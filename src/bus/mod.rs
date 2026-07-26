//! The bus server. Owns the registry, rooms, message log, and file store.
//! Knows nothing about MCP — it speaks only the `proto` wire types.

pub mod delivery;
pub mod registry;
pub mod rooms;

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::{Router, routing::get};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::proto::{
    AgentInfo, FileInfo, FromBus, HistoryItem, ReplyResult, RoomInfo, Target, ToBus,
};
use crate::store::{Store, now_ms};
use delivery::{GuardVerdict, Guards};
use registry::Registry;

#[derive(Clone)]
struct App {
    store: Arc<Store>,
    registry: Registry,
    guards: Guards,
}

pub async fn serve(port: u16, data_dir: PathBuf) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    eprintln!("claude-bus listening on 0.0.0.0:{port}");
    serve_on(listener, data_dir).await
}

/// Split out so tests can bind port 0 and learn the assigned port.
pub async fn serve_on(listener: tokio::net::TcpListener, data_dir: PathBuf) -> anyhow::Result<()> {
    serve_on_with(listener, data_dir, Guards::default()).await
}

/// Guards are injected rather than read from configuration so tests can disable
/// the rate limit without the production path branching on a test-only signal.
pub async fn serve_on_with(
    listener: tokio::net::TcpListener,
    data_dir: PathBuf,
    guards: Guards,
) -> anyhow::Result<()> {
    let app = App {
        store: Arc::new(Store::open(&data_dir).await?),
        registry: Registry::new(),
        guards,
    };
    let router = Router::new()
        .route("/ws", get(upgrade))
        .route("/human-active", axum::routing::post(human_active))
        .with_state(app);
    axum::serve(listener, router).await?;
    Ok(())
}

#[derive(serde::Deserialize)]
struct HumanActiveQuery {
    agent: String,
}

/// Called by the optional UserPromptSubmit hook. The human typing is the only
/// accurate signal that a conversation is still supervised.
async fn human_active(
    State(app): State<App>,
    axum::extract::Query(q): axum::extract::Query<HumanActiveQuery>,
) -> &'static str {
    let rooms: Vec<String> = app
        .store
        .rooms()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.members.contains(&q.agent))
        .map(|r| r.name)
        .collect();
    app.guards.reset_all_for(&rooms).await;
    "ok"
}

async fn upgrade(ws: WebSocketUpgrade, State(app): State<App>) -> Response {
    ws.on_upgrade(move |socket| connection(socket, app))
}

async fn connection(socket: WebSocket, app: App) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<FromBus>();

    let writer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let Ok(json) = serde_json::to_string(&event) else {
                continue;
            };
            if sink.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    let mut me: Option<String> = None;

    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else { continue };
        let cmd: ToBus = match serde_json::from_str(&text) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(FromBus::Error {
                    req_id: None,
                    message: format!("unparseable command: {e}"),
                });
                continue;
            }
        };

        if let ToBus::Register {
            name,
            host,
            cwd,
            session_id,
        } = &cmd
        {
            // A connection registers exactly once. Accepting a second
            // Register would mint a fresh effective name via `attach` (e.g.
            // `caas#2`) while leaving the original identity's connection
            // entry in the registry untouched — that first name would never
            // be detached, so it would stay "online" forever, silently
            // swallowing anything addressed to it after this socket closes.
            if let Some(existing) = &me {
                let _ = tx.send(FromBus::Error {
                    req_id: None,
                    message: format!(
                        "already registered as {existing}; a connection may only register once"
                    ),
                });
                continue;
            }

            let effective = app.registry.attach(name, host, tx.clone()).await;
            let _ = app
                .store
                .upsert_agent(&effective, host, cwd, session_id.as_deref())
                .await;
            me = Some(effective.clone());
            let _ = tx.send(FromBus::Registered {
                name: effective.clone(),
            });
            send_unread_summaries(&app, &effective, &tx).await;
            continue;
        }

        let Some(name) = me.clone() else {
            let _ = tx.send(FromBus::Error {
                req_id: None,
                message: "register before sending commands".into(),
            });
            continue;
        };

        handle(&app, &name, cmd, &tx).await;
    }

    if let Some(name) = me {
        app.registry.detach(&name).await;
        let _ = app.store.set_online(&name, false).await;
        eprintln!("disconnected: {name}");
    }
    writer.abort();
}

/// On reconnect an agent gets counts, never the backlog: replaying yesterday's
/// conversation into a fresh session wastes context and derails whatever the
/// human actually sat down to do.
async fn send_unread_summaries(app: &App, name: &str, tx: &registry::Sender) {
    let Ok(rooms) = app.store.rooms().await else {
        return;
    };
    for room in rooms {
        if !room.members.iter().any(|m| m == name) {
            continue;
        }
        if let Ok(count) = app.store.unread_count(&room.name, name).await
            && count > 0
        {
            let _ = tx.send(FromBus::Unread {
                room: room.name.clone(),
                count,
            });
        }
    }
}

async fn known_rooms(app: &App) -> String {
    match app.store.rooms().await {
        Ok(rooms) if !rooms.is_empty() => rooms
            .into_iter()
            .map(|r| r.name)
            .collect::<Vec<_>>()
            .join(", "),
        _ => "(none yet)".to_string(),
    }
}

async fn handle(app: &App, me: &str, cmd: ToBus, tx: &registry::Sender) {
    match cmd {
        ToBus::Register { .. } => {}

        ToBus::Join { req_id, room } => {
            if let Err(e) = app.store.join_room(&room, me).await {
                let _ = tx.send(FromBus::Error {
                    req_id: Some(req_id),
                    message: e.to_string(),
                });
                return;
            }
            let members = app.store.room_members(&room).await.unwrap_or_default();
            let _ = tx.send(FromBus::Reply {
                req_id,
                result: ReplyResult::Joined { room, members },
            });
        }

        ToBus::Send {
            req_id,
            target,
            text,
            done,
        } => {
            let room = rooms::resolve(&target, me);

            match app.guards.check(&room, me, now_ms()).await {
                GuardVerdict::Allow => {}
                GuardVerdict::RateLimited { retry_in_ms } => {
                    let _ = tx.send(FromBus::Error {
                        req_id: Some(req_id),
                        message: format!("rate limited; retry in {retry_in_ms} ms"),
                    });
                    return;
                }
                GuardVerdict::Paused { count } => {
                    let pause_reason = format!(
                        "{count} messages in this room with no human input. \
                         Tell your human, and call resume once they say to continue."
                    );
                    // The channel event is what informs the model
                    // conversationally; the Error below is what resolves the
                    // outstanding `send` request so it doesn't sit blocked
                    // for the full 10s timeout and get misreported as the
                    // bus being unreachable.
                    let _ = tx.send(FromBus::Paused {
                        room: room.clone(),
                        reason: pause_reason.clone(),
                    });
                    let _ = tx.send(FromBus::Error {
                        req_id: Some(req_id),
                        message: format!(
                            "send blocked: room \"{room}\" is paused ({pause_reason}) \
                             The bus itself is reachable — this is the exchange-cap pause, \
                             not an outage. Call resume once your human says to continue."
                        ),
                    });
                    return;
                }
            }

            // A DM auto-creates its room and enrolls both sides.
            let _ = app.store.join_room(&room, me).await;
            if let Target::Agent { name } = &target {
                let _ = app.store.join_room(&room, name).await;
            }

            let msg_id = match app.store.append_message(&room, me, &text, done).await {
                Ok(id) => id,
                Err(e) => {
                    let _ = tx.send(FromBus::Error {
                        req_id: Some(req_id),
                        message: e.to_string(),
                    });
                    return;
                }
            };

            let members = app.store.room_members(&room).await.unwrap_or_default();
            let mut delivered_to = Vec::new();
            let mut queued_for = Vec::new();
            for member in members.iter().filter(|m| m.as_str() != me) {
                let event = FromBus::Message {
                    id: msg_id,
                    room: room.clone(),
                    from: me.to_string(),
                    text: text.clone(),
                    done,
                };
                if app.registry.send_to(member, event).await {
                    delivered_to.push(member.clone());
                } else {
                    queued_for.push(member.clone());
                }
            }

            let _ = tx.send(FromBus::Reply {
                req_id,
                result: ReplyResult::Sent {
                    room,
                    msg_id,
                    delivered_to,
                    queued_for,
                },
            });
        }

        ToBus::History {
            req_id,
            room,
            limit,
        } => {
            let members = app.store.room_members(&room).await.unwrap_or_default();
            if members.is_empty() {
                let _ = tx.send(FromBus::Error {
                    req_id: Some(req_id),
                    message: format!(
                        "no room named {room}. Known rooms: {}",
                        known_rooms(app).await
                    ),
                });
                return;
            }
            let messages = app
                .store
                .history(&room, limit)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|m| HistoryItem {
                    id: m.id,
                    from: m.from_agent,
                    text: m.body,
                    done: m.done,
                    created_at: m.created_at,
                })
                .collect();
            let _ = tx.send(FromBus::Reply {
                req_id,
                result: ReplyResult::History { messages },
            });
        }

        ToBus::ListRooms { req_id } => {
            let rooms = app
                .store
                .rooms()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|r| RoomInfo {
                    name: r.name,
                    mode: r.mode,
                    members: r.members,
                })
                .collect();
            let _ = tx.send(FromBus::Reply {
                req_id,
                result: ReplyResult::Rooms { rooms },
            });
        }

        ToBus::ListAgents { req_id } => {
            let online = app.registry.online().await;
            let agents = app
                .store
                .agents()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|a| AgentInfo {
                    online: online.contains(&a.name),
                    name: a.name,
                    host: a.host,
                })
                .collect();
            let _ = tx.send(FromBus::Reply {
                req_id,
                result: ReplyResult::Agents { agents },
            });
        }

        ToBus::PutFile {
            req_id,
            room,
            key,
            content_b64,
            content_type,
        } => {
            let bytes = match base64::engine::general_purpose::STANDARD.decode(&content_b64) {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx.send(FromBus::Error {
                        req_id: Some(req_id),
                        message: format!("content is not valid base64: {e}"),
                    });
                    return;
                }
            };
            match app
                .store
                .put_file(&room, &key, &bytes, content_type.as_deref(), me)
                .await
            {
                Ok(f) => {
                    let _ = tx.send(FromBus::Reply {
                        req_id,
                        result: ReplyResult::FileStored {
                            key: f.key,
                            size: f.size,
                            sha256: f.sha256,
                        },
                    });
                }
                Err(e) => {
                    let _ = tx.send(FromBus::Error {
                        req_id: Some(req_id),
                        message: e.to_string(),
                    });
                }
            }
        }

        ToBus::GetFile { req_id, room, key } => match app.store.get_file(&room, &key).await {
            Ok(Some((meta, bytes))) => {
                let _ = tx.send(FromBus::Reply {
                    req_id,
                    result: ReplyResult::FileContent {
                        key: meta.key,
                        content_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
                        content_type: meta.content_type,
                    },
                });
            }
            Ok(None) => {
                let available = app
                    .store
                    .list_files(&room)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|f| f.key)
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = tx.send(FromBus::Error {
                    req_id: Some(req_id),
                    message: format!(
                        "no file {key} in {room}. Available: {}",
                        if available.is_empty() {
                            "(none)".into()
                        } else {
                            available
                        }
                    ),
                });
            }
            Err(e) => {
                let _ = tx.send(FromBus::Error {
                    req_id: Some(req_id),
                    message: e.to_string(),
                });
            }
        },

        ToBus::ListFiles { req_id, room } => {
            let files = app
                .store
                .list_files(&room)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|f| FileInfo {
                    key: f.key,
                    size: f.size,
                    content_type: f.content_type,
                    updated_by: f.updated_by,
                })
                .collect();
            let _ = tx.send(FromBus::Reply {
                req_id,
                result: ReplyResult::Files { files },
            });
        }

        ToBus::Resume { req_id, room } => {
            app.guards.reset(&room).await;
            let _ = tx.send(FromBus::Reply {
                req_id,
                result: ReplyResult::Resumed { room },
            });
        }

        ToBus::Ack {
            room,
            last_delivered_id,
        } => {
            let _ = app.store.set_cursor(&room, me, last_delivered_id).await;
        }
    }
}
