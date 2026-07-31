//! The command dispatcher: turns a `ToBus` from a connected agent into a
//! store/registry operation and a reply on the caller's control channel.
//! Split out of `bus::mod` so the connection loop (socket lifecycle, writer
//! task, keepalive tickers) doesn't share a file with the command handling
//! this module adds to over time.

use base64::Engine;
use serde_json::json;

use super::App;
use super::delivery::GuardVerdict;
use super::registry;
use super::rooms;
use crate::proto::{
    AgentInfo, FileInfo, FromBus, HistoryItem, ReplyResult, RoomInfo, Target, ToBus,
};
use crate::store::now_ms;

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

pub(crate) async fn handle(
    app: &App,
    me: &str,
    cmd: ToBus,
    control_tx: &registry::Sender,
    is_human: bool,
) {
    match cmd {
        ToBus::Register { .. } => {}

        // Both are observer-only, rejected here for a registered agent the
        // same way `handle_observer` rejects agent-only commands for an
        // observer — the two roles are disjoint by construction, not just by
        // convention.
        ToBus::Observe { .. } => {
            let _ = control_tx.try_send(FromBus::Error {
                req_id: None,
                message: "already registered as an agent; a connection may only identify once"
                    .into(),
            });
        }
        ToBus::Watch { req_id, .. } => {
            let _ = control_tx.try_send(FromBus::Error {
                req_id: Some(req_id),
                message: "watch is for observers; a registered agent should use join".into(),
            });
        }

        ToBus::Join { req_id, room } => {
            if let Err(e) = app.store.join_room(&room, me).await {
                let _ = control_tx.try_send(FromBus::Error {
                    req_id: Some(req_id),
                    message: e.to_string(),
                });
                return;
            }
            let members = app.store.room_members(&room).await.unwrap_or_default();
            let _ = app
                .store
                .append_event("room_joined", Some(me), Some(&room), json!({}))
                .await;
            let _ = control_tx.try_send(FromBus::Reply {
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

            match app.guards.check(&room, me, now_ms(), is_human).await {
                GuardVerdict::Allow => {}
                GuardVerdict::RateLimited { retry_in_ms } => {
                    let _ = app
                        .store
                        .append_event(
                            "rate_limited",
                            Some(me),
                            Some(&room),
                            json!({ "retry_in_ms": retry_in_ms }),
                        )
                        .await;
                    let _ = control_tx.try_send(FromBus::Error {
                        req_id: Some(req_id),
                        message: format!("rate limited; retry in {retry_in_ms} ms"),
                    });
                    return;
                }
                GuardVerdict::Paused { count } => {
                    let _ = app
                        .store
                        .append_event(
                            "room_paused",
                            Some(me),
                            Some(&room),
                            json!({ "count": count }),
                        )
                        .await;
                    let pause_reason = format!(
                        "{count} messages in this room with no human input. \
                         Tell your human, and call resume once they say to continue."
                    );
                    // The channel event is what informs the model
                    // conversationally; the Error below is what resolves the
                    // outstanding `send` request so it doesn't sit blocked
                    // for the full 10s timeout and get misreported as the
                    // bus being unreachable.
                    let _ = control_tx.try_send(FromBus::Paused {
                        room: room.clone(),
                        reason: pause_reason.clone(),
                    });
                    let _ = control_tx.try_send(FromBus::Error {
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

            let msg_id = match app
                .store
                .append_message(&room, me, &text, done, false)
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    let _ = control_tx.try_send(FromBus::Error {
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

            // Observers watching this room get the same event, but they are
            // spectators: this is a separate fan-out from the member loop
            // above and never touches `delivered_to`/`queued_for` — an
            // observer was never a party to the `send`, so it must not be
            // able to influence what the sender is told was delivered vs.
            // queued.
            app.registry
                .notify_watchers(
                    &room,
                    FromBus::Message {
                        id: msg_id,
                        room: room.clone(),
                        from: me.to_string(),
                        text,
                        done,
                    },
                )
                .await;

            let _ = app
                .store
                .append_event(
                    "message_sent",
                    Some(me),
                    Some(&room),
                    json!({
                        "msg_id": msg_id,
                        "delivered_to": &delivered_to,
                        "queued_for": &queued_for,
                        "done": done,
                    }),
                )
                .await;

            let _ = control_tx.try_send(FromBus::Reply {
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
            // The cursor means "delivered to this agent"; the unread summary
            // on reconnect is just a count, not delivery, but `history`'s
            // reply genuinely hands the messages to the model — so this is
            // the moment delivery happens for the catch-up path. Deliberately
            // *not* inside `reply_history`: that function is shared with
            // `handle_observer`'s `ToBus::History` arm, which has no `me` and
            // must never move a real agent's cursor.
            if let Some(max_id) = reply_history(app, control_tx, req_id, &room, limit).await {
                let _ = app.store.set_cursor(&room, me, max_id).await;
                let _ = app
                    .store
                    .append_event(
                        "ack",
                        Some(me),
                        Some(&room),
                        json!({ "last_delivered_id": max_id }),
                    )
                    .await;
            }
        }

        ToBus::ListRooms { req_id } => reply_list_rooms(app, control_tx, req_id).await,

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
            let _ = control_tx.try_send(FromBus::Reply {
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
                    let _ = control_tx.try_send(FromBus::Error {
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
                    let _ = app
                        .store
                        .append_event(
                            "file_stored",
                            Some(me),
                            Some(&room),
                            json!({ "key": &f.key, "size": f.size, "sha256": &f.sha256 }),
                        )
                        .await;
                    let _ = control_tx.try_send(FromBus::Reply {
                        req_id,
                        result: ReplyResult::FileStored {
                            key: f.key,
                            size: f.size,
                            sha256: f.sha256,
                        },
                    });
                }
                Err(e) => {
                    let _ = control_tx.try_send(FromBus::Error {
                        req_id: Some(req_id),
                        message: e.to_string(),
                    });
                }
            }
        }

        ToBus::GetFile { req_id, room, key } => match app.store.get_file(&room, &key).await {
            Ok(Some((meta, bytes))) => {
                let _ = app
                    .store
                    .append_event(
                        "file_fetched",
                        Some(me),
                        Some(&room),
                        json!({ "key": &key }),
                    )
                    .await;
                let _ = control_tx.try_send(FromBus::Reply {
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
                let _ = control_tx.try_send(FromBus::Error {
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
                let _ = control_tx.try_send(FromBus::Error {
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
            let _ = control_tx.try_send(FromBus::Reply {
                req_id,
                result: ReplyResult::Files { files },
            });
        }

        ToBus::Resume { req_id, room } => {
            app.guards.reset(&room).await;
            let _ = app
                .store
                .append_event("resumed", Some(me), Some(&room), json!({}))
                .await;
            let _ = control_tx.try_send(FromBus::Reply {
                req_id,
                result: ReplyResult::Resumed { room },
            });
        }

        ToBus::Ack {
            room,
            last_delivered_id,
        } => {
            let _ = app.store.set_cursor(&room, me, last_delivered_id).await;
            let _ = app
                .store
                .append_event(
                    "ack",
                    Some(me),
                    Some(&room),
                    json!({ "last_delivered_id": last_delivered_id }),
                )
                .await;
        }
    }
}

/// Shared by a registered agent's `History` and an observer's — read paths
/// carry no membership check today (that is deliberately separate work), so
/// there is nothing agent-specific about this beyond who is allowed to call
/// it.
///
/// Returns the highest message id actually sent in the reply (`None` if no
/// reply with messages went out, e.g. the room doesn't exist or has no
/// messages), so a caller that knows a delivery-cursor-owning `me` — i.e.
/// the `ToBus::History` arm in `handle`, not `handle_observer` — can advance
/// that agent's cursor to exactly what it was shown. `limit` may be smaller
/// than the number of unread messages, in which case the caller only ever
/// saw the most recent `limit` of them; returning the true max of *those*
/// keeps the cursor honest rather than jumping to the room's newest message.
/// This function deliberately never touches the cursor itself: it is called
/// from `handle_observer` too, for a `tail` watcher that has no cursor at
/// all, and moving one from here would move a real agent's cursor just by
/// virtue of someone watching the room.
pub(crate) async fn reply_history(
    app: &App,
    control_tx: &registry::Sender,
    req_id: u64,
    room: &str,
    limit: i64,
) -> Option<i64> {
    // Existence, not membership: a room whose only participant was a human holds no
    // members once they disconnect, but its transcript is still there and still worth
    // reading — not least by that same human reconnecting. See `Store::room_exists`.
    if !app.store.room_exists(room).await.unwrap_or(false) {
        let _ = control_tx.try_send(FromBus::Error {
            req_id: Some(req_id),
            message: format!(
                "no room named {room}. Known rooms: {}",
                known_rooms(app).await
            ),
        });
        return None;
    }
    let rows = app.store.history(room, limit).await.unwrap_or_default();
    let max_id = rows.iter().map(|m| m.id).max();
    let messages = rows
        .into_iter()
        .map(|m| HistoryItem {
            id: m.id,
            from: m.from_agent,
            text: m.body,
            done: m.done,
            created_at: m.created_at,
            human: m.human,
        })
        .collect();
    let _ = control_tx.try_send(FromBus::Reply {
        req_id,
        result: ReplyResult::History { messages },
    });
    max_id
}

/// Shared by a registered agent's `ListRooms` and an observer's — see
/// `reply_history`.
pub(crate) async fn reply_list_rooms(app: &App, control_tx: &registry::Sender, req_id: u64) {
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
    let _ = control_tx.try_send(FromBus::Reply {
        req_id,
        result: ReplyResult::Rooms { rooms },
    });
}
