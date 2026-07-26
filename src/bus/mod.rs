//! The bus server. Owns the registry, rooms, message log, and file store.
//! Knows nothing about MCP — it speaks only the `proto` wire types.

pub mod delivery;
pub mod registry;
pub mod rooms;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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

/// How often the bus pings each connected client, and how long it waits for
/// a pong before deciding the peer is gone.
///
/// Nothing on either side sent a `Ping` before this fix, so a peer whose
/// host vanished hard (lid closed, cable pulled, NAT black-holing) stayed
/// "online" — and kept being reported as `delivered` — until TCP itself
/// noticed, which is minutes away by default and can be indefinite behind a
/// black-holing NAT. This is what closes that gap.
///
/// `ping_interval` = 30s / `pong_timeout` = 90s (3 missed pings) for the
/// production default: frequent enough that a vanished peer is caught well
/// within a human's patience for "is this thing on", generous enough
/// (3 full cycles, not 1) that a busy-but-alive agent — one whose task is
/// momentarily starving the executor that would otherwise answer a ping —
/// is never mistaken for a dead one. `tokio-tungstenite` (both the axum
/// server side and the `tokio-tungstenite` client side used by
/// `agent::bridge` and `tail`) answers `Ping` with `Pong` automatically as
/// long as something keeps polling the stream, which both clients already
/// do in their main read loops — no client-side change was needed for pongs
/// to happen.
#[derive(Clone, Copy)]
pub struct Keepalive {
    pub ping_interval: Duration,
    pub pong_timeout: Duration,
}

impl Default for Keepalive {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            pong_timeout: Duration::from_secs(90),
        }
    }
}

impl Keepalive {
    pub fn new(ping_interval: Duration, pong_timeout: Duration) -> Self {
        Self {
            ping_interval,
            pong_timeout,
        }
    }
}

#[derive(Clone)]
struct App {
    store: Arc<Store>,
    registry: Registry,
    guards: Guards,
    keepalive: Keepalive,
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
    serve_on_with_keepalive(listener, data_dir, guards, Keepalive::default()).await
}

/// Keepalive is injected the same way `Guards` is: production gets the real
/// 30s/90s cadence, tests get millisecond-scale intervals so the "a vanished
/// peer stops being reported as online" behavior is testable without
/// sleeping for the production timeout.
pub async fn serve_on_with_keepalive(
    listener: tokio::net::TcpListener,
    data_dir: PathBuf,
    guards: Guards,
    keepalive: Keepalive,
) -> anyhow::Result<()> {
    let app = App {
        store: Arc::new(Store::open(&data_dir).await?),
        registry: Registry::new(),
        guards,
        keepalive,
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
    let (tx, mut rx) = mpsc::channel::<FromBus>(registry::CHANNEL_CAPACITY);

    // The writer task owns the sink for the connection's whole lifetime, so
    // it — not the read loop below — is the one that actually emits `Ping`
    // frames on a timer, interleaved with real `FromBus` events.
    let ping_interval = app.keepalive.ping_interval;
    let writer = tokio::spawn(async move {
        let mut ping_ticker = tokio::time::interval(ping_interval);
        ping_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping_ticker.tick().await; // the first tick fires immediately; skip it
        loop {
            tokio::select! {
                event = rx.recv() => {
                    let Some(event) = event else { break };
                    let Ok(json) = serde_json::to_string(&event) else {
                        continue;
                    };
                    if sink.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                _ = ping_ticker.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut me: Option<String> = None;

    // A peer that stops answering pings must not be allowed to keep the read
    // loop parked in `stream.next().await` forever — on a black-holed
    // connection that read may never return on its own, TCP alone can take
    // ~15 minutes to notice by default, and indefinitely behind a
    // black-holing NAT. `timeout_ticker` fires independently of whatever
    // `stream.next()` is doing, so the peer is judged solely on whether a
    // pong actually arrived, not on whether the socket happens to wake up.
    let mut last_pong = tokio::time::Instant::now();
    let mut timeout_ticker = tokio::time::interval(app.keepalive.ping_interval);
    timeout_ticker.tick().await; // skip the immediate first tick

    loop {
        tokio::select! {
            incoming = stream.next() => {
                let Some(Ok(msg)) = incoming else { break };
                let text = match msg {
                    Message::Pong(_) => {
                        last_pong = tokio::time::Instant::now();
                        continue;
                    }
                    Message::Text(text) => text,
                    _ => continue,
                };

                let cmd: ToBus = match serde_json::from_str(&text) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.try_send(FromBus::Error {
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
                        let _ = tx.try_send(FromBus::Error {
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
                    let _ = tx.try_send(FromBus::Registered {
                        name: effective.clone(),
                    });
                    send_unread_summaries(&app, &effective, &tx).await;
                    continue;
                }

                let Some(name) = me.clone() else {
                    let _ = tx.try_send(FromBus::Error {
                        req_id: None,
                        message: "register before sending commands".into(),
                    });
                    continue;
                };

                handle(&app, &name, cmd, &tx).await;
            }
            _ = timeout_ticker.tick() => {
                if last_pong.elapsed() > app.keepalive.pong_timeout {
                    if let Some(name) = &me {
                        eprintln!(
                            "keepalive timeout: no pong from {name} within {:?}, closing",
                            app.keepalive.pong_timeout
                        );
                    }
                    break;
                }
            }
        }
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
            let _ = tx.try_send(FromBus::Unread {
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
                let _ = tx.try_send(FromBus::Error {
                    req_id: Some(req_id),
                    message: e.to_string(),
                });
                return;
            }
            let members = app.store.room_members(&room).await.unwrap_or_default();
            let _ = tx.try_send(FromBus::Reply {
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
                    let _ = tx.try_send(FromBus::Error {
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
                    let _ = tx.try_send(FromBus::Paused {
                        room: room.clone(),
                        reason: pause_reason.clone(),
                    });
                    let _ = tx.try_send(FromBus::Error {
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
                    let _ = tx.try_send(FromBus::Error {
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

            let _ = tx.try_send(FromBus::Reply {
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
                let _ = tx.try_send(FromBus::Error {
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
            let _ = tx.try_send(FromBus::Reply {
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
            let _ = tx.try_send(FromBus::Reply {
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
            let _ = tx.try_send(FromBus::Reply {
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
                    let _ = tx.try_send(FromBus::Error {
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
                    let _ = tx.try_send(FromBus::Reply {
                        req_id,
                        result: ReplyResult::FileStored {
                            key: f.key,
                            size: f.size,
                            sha256: f.sha256,
                        },
                    });
                }
                Err(e) => {
                    let _ = tx.try_send(FromBus::Error {
                        req_id: Some(req_id),
                        message: e.to_string(),
                    });
                }
            }
        }

        ToBus::GetFile { req_id, room, key } => match app.store.get_file(&room, &key).await {
            Ok(Some((meta, bytes))) => {
                let _ = tx.try_send(FromBus::Reply {
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
                let _ = tx.try_send(FromBus::Error {
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
                let _ = tx.try_send(FromBus::Error {
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
            let _ = tx.try_send(FromBus::Reply {
                req_id,
                result: ReplyResult::Files { files },
            });
        }

        ToBus::Resume { req_id, room } => {
            app.guards.reset(&room).await;
            let _ = tx.try_send(FromBus::Reply {
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
