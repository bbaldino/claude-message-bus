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

/// What authority a connection's command carries.
///
/// `human_present` means a person is actually at the keyboard — it, and only it,
/// exempts the exchange guard and clears a pause. `relayer` means the connection
/// speaks with a human's authority (label only): its messages are stamped human,
/// but the guards still apply. Keeping the two apart is the whole point (see the
/// long note in the `Send` arm): authority is delegable by configuration,
/// attendance is not.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Authority {
    pub human_present: bool,
    pub relayer: bool,
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

/// The typed result of a send, shared by the WS command arm and the HTTP
/// participant endpoint. Each transport formats it its own way (control-channel
/// frames vs. a JSON `outcome`), but the decision — and every store write behind
/// it — is made once, here.
pub(crate) enum SendOutcome {
    Sent {
        room: String,
        msg_id: i64,
        delivered_to: Vec<String>,
        queued_for: Vec<String>,
    },
    RateLimited {
        retry_in_ms: i64,
    },
    Paused {
        room: String,
        count: u32,
        reason: String,
    },
    /// A `Target::Agent` with no such agent. Carries the caller-facing message
    /// (including a "did you mean" suggestion); the `send_refused` event is
    /// already recorded.
    UnknownAgent {
        message: String,
    },
    /// A storage or verification failure — nothing was sent.
    Error {
        message: String,
    },
}

/// Perform a send: resolve the room, refuse an unknown DM target, run the
/// delivery guards, persist, fan out to members and observers, and record the
/// audit events. Returns a typed outcome; writes no wire frames of its own, so
/// both the WS arm and the HTTP endpoint can render it.
///
/// `authority.human_present` is what the guard consults — deliberately NOT the
/// relayer-expanded `has_human_authority` used as the message *label*. The two
/// look like one question and are not: the gate asks whether a person is present
/// (which zeroes the exchange counter and un-pauses the room), while the label
/// asks whose authority the message carries. Authority is delegable by
/// configuration (a relayer, or a secret-proven HTTP lease); attendance is not,
/// because the bus cannot tell whether a relayer is passing on a human's words
/// or composing its own. Collapsing the two would silently delete the exchange
/// cap for relayed conversations. A relayer un-pauses through `Resume` instead.
pub(crate) async fn do_send(
    app: &App,
    me: &str,
    target: Target,
    text: String,
    done: bool,
    authority: Authority,
) -> SendOutcome {
    let room = rooms::resolve(&target, me);

    // An unknown DM target is refused before the guards: a malformed request must
    // not consume exchange-cap budget or come back as `RateLimited`, which would
    // send the caller off to retry something that can never work. Existence, not
    // liveness — an offline agent still has its row, and queuing for it is the
    // point of the bus. Only `Target::Agent` is checked; rooms auto-create.
    if let Target::Agent { name } = &target {
        let known = match app.store.agent_exists(name).await {
            Ok(known) => known,
            Err(e) => {
                eprintln!("could not check whether agent {name} exists: {e}");
                return SendOutcome::Error {
                    message: "could not verify the target agent".to_string(),
                };
            }
        };
        if !known {
            let suggestion = match name.split_once('@') {
                Some((bare, _)) if app.store.agent_exists(bare).await.unwrap_or(false) => {
                    format!("; did you mean {bare:?}?")
                }
                _ => "; call `agents` for the list".to_string(),
            };
            let _ = app
                .store
                .append_event(
                    "send_refused",
                    Some(me),
                    None,
                    json!({ "target": name, "reason": "unknown_agent" }),
                )
                .await;
            return SendOutcome::UnknownAgent {
                message: format!("no agent named {name:?}{suggestion}"),
            };
        }
    }

    let cleared_pause = match app
        .guards
        .check(&room, me, now_ms(), authority.human_present)
        .await
    {
        GuardVerdict::Allow { cleared_pause } => cleared_pause,
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
            return SendOutcome::RateLimited { retry_in_ms };
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
            let reason = format!(
                "{count} messages in this room with no human input. \
                 Tell your human, and call resume once they say to continue."
            );
            return SendOutcome::Paused {
                room,
                count,
                reason,
            };
        }
    };

    // The label the message carries: the connection's own presence, a relay
    // grant (config), or a secret-proven HTTP lease. Never anything in the
    // payload. Wider than the gate above, on purpose — see this function's doc.
    let has_human_authority =
        authority.human_present || authority.relayer || app.relayers.contains(me);

    // A DM auto-creates its room and enrolls both sides.
    let _ = app.store.join_room(&room, me).await;
    if let Target::Agent { name } = &target {
        let _ = app.store.join_room(&room, name).await;
    }

    let msg_id = match app
        .store
        .append_message(&room, me, &text, done, has_human_authority)
        .await
    {
        Ok(id) => id,
        Err(e) => {
            return SendOutcome::Error {
                message: e.to_string(),
            };
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
            human: has_human_authority,
        };
        if app.registry.send_to(member, event).await {
            delivered_to.push(member.clone());
        } else {
            queued_for.push(member.clone());
        }
    }

    // Observers are spectators: a separate fan-out that never touches
    // delivered/queued, since an observer was never a party to the send.
    app.registry
        .notify_watchers(
            &room,
            FromBus::Message {
                id: msg_id,
                room: room.clone(),
                from: me.to_string(),
                text,
                done,
                human: has_human_authority,
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

    // A human message that lifted a pause is recorded so `Store::room_flag`
    // stops deriving "needs you". Appended after the message stores, so it can
    // never claim a resume for a send that failed.
    if cleared_pause {
        let _ = app
            .store
            .append_event(
                "resumed",
                Some(me),
                Some(&room),
                json!({ "via": "human_message", "msg_id": msg_id }),
            )
            .await;
    }

    SendOutcome::Sent {
        room,
        msg_id,
        delivered_to,
        queued_for,
    }
}

pub(crate) async fn handle(
    app: &App,
    me: &str,
    cmd: ToBus,
    control_tx: &registry::Sender,
    authority: Authority,
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
        ToBus::Unwatch { req_id, .. } => {
            let _ = control_tx.try_send(FromBus::Error {
                req_id: Some(req_id),
                message: "unwatch is for observers; a registered agent should use join".into(),
            });
        }
        ToBus::WatchPresence { req_id } => {
            let _ = control_tx.try_send(FromBus::Error {
                req_id: Some(req_id),
                message: "watch_presence is for observers; a registered agent has no use for it"
                    .into(),
            });
        }
        ToBus::WatchEvents { req_id, .. } => {
            let _ = control_tx.try_send(FromBus::Error {
                req_id: Some(req_id),
                message: "watch_events is for observers; a registered agent has no use for it"
                    .into(),
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
            // The send core is shared with the HTTP participant path via
            // `do_send`; this arm only translates the typed outcome onto the WS
            // wire (control-channel frames). Every store write — the message
            // itself and the `message_sent`/`rate_limited`/`room_paused`/
            // `send_refused`/`resumed` events — happens inside `do_send`, so both
            // transports produce an identical audit trail.
            match do_send(app, me, target, text, done, authority).await {
                SendOutcome::Sent {
                    room,
                    msg_id,
                    delivered_to,
                    queued_for,
                } => {
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
                SendOutcome::RateLimited { retry_in_ms } => {
                    let _ = control_tx.try_send(FromBus::Error {
                        req_id: Some(req_id),
                        message: format!("rate limited; retry in {retry_in_ms} ms"),
                    });
                }
                SendOutcome::Paused {
                    room,
                    count: _,
                    reason,
                } => {
                    // The channel event informs the model conversationally; the
                    // Error resolves the outstanding `send` so it doesn't block
                    // for the full 10s timeout and get misreported as the bus
                    // being unreachable.
                    let _ = control_tx.try_send(FromBus::Paused {
                        room: room.clone(),
                        reason: reason.clone(),
                    });
                    let _ = control_tx.try_send(FromBus::Error {
                        req_id: Some(req_id),
                        message: format!(
                            "send blocked: room \"{room}\" is paused ({reason}) \
                             The bus itself is reachable — this is the exchange-cap pause, \
                             not an outage. Call resume once your human says to continue."
                        ),
                    });
                }
                SendOutcome::UnknownAgent { message } | SendOutcome::Error { message } => {
                    let _ = control_tx.try_send(FromBus::Error {
                        req_id: Some(req_id),
                        message,
                    });
                }
            }
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
                    version: a.version,
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
