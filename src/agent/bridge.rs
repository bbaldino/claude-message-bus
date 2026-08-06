//! Bridges the bus WebSocket to the live Claude Code session.
//!
//! Inbound bus messages become `notifications/claude/channel` events, which is
//! the one mechanism that reaches a session sitting idle. Notifications are
//! unacknowledged: if the session was not launched with the channel registered,
//! the event is discarded with no error, so every emission is logged to stderr.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rmcp::model::{CustomNotification, ServerNotification};
use rmcp::service::{Peer, RoleServer};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::agent::handler::Pending;
use crate::proto::{FromBus, ToBus};

pub struct BridgeConfig {
    pub bus_url: String,
    pub name: String,
    pub host: String,
    pub cwd: String,
    pub session_id: Option<String>,
}

/// A connection that stayed up at least this long resets the backoff to its
/// floor on the next drop, so a bus restart after hours of uptime does not
/// wait up to 30s to reconnect just because of a stale prior outage.
const STABLE_CONNECTION_THRESHOLD: Duration = Duration::from_secs(60);
const BACKOFF_FLOOR: Duration = Duration::from_secs(1);

pub async fn run(
    cfg: BridgeConfig,
    mut rx: mpsc::UnboundedReceiver<ToBus>,
    tx: mpsc::UnboundedSender<ToBus>,
    peer: Peer<RoleServer>,
    pending: Pending,
) {
    let mut backoff = BACKOFF_FLOOR;
    loop {
        let connected_at = std::time::Instant::now();
        match connect_once(&cfg, &mut rx, &tx, &peer, &pending).await {
            Ok(()) => eprintln!("[agent] bus connection closed"),
            Err(e) => eprintln!("[agent] bus error: {e}"),
        }
        if connected_at.elapsed() >= STABLE_CONNECTION_THRESHOLD {
            backoff = BACKOFF_FLOOR;
        }
        eprintln!("[agent] reconnecting in {:?}", backoff);
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

async fn connect_once(
    cfg: &BridgeConfig,
    rx: &mut mpsc::UnboundedReceiver<ToBus>,
    tx: &mpsc::UnboundedSender<ToBus>,
    peer: &Peer<RoleServer>,
    pending: &Pending,
) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(&cfg.bus_url).await?;
    let (mut sink, mut stream) = ws.split();
    eprintln!("[agent] connected to {}", cfg.bus_url);

    let register = ToBus::Register {
        name: cfg.name.clone(),
        host: cfg.host.clone(),
        cwd: cfg.cwd.clone(),
        session_id: cfg.session_id.clone(),
        human: false,
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
    };
    sink.send(Message::text(serde_json::to_string(&register)?))
        .await?;

    loop {
        tokio::select! {
            outbound = rx.recv() => {
                let Some(cmd) = outbound else { return Ok(()) };
                sink.send(Message::text(serde_json::to_string(&cmd)?)).await?;
            }
            inbound = stream.next() => {
                let Some(msg) = inbound else { return Ok(()) };
                let Ok(text) = msg?.into_text() else { continue };
                if text.trim().is_empty() { continue }
                let event: FromBus = match serde_json::from_str(&text) {
                    Ok(e) => e,
                    Err(e) => { eprintln!("[agent] unparseable from bus: {e}: {text}"); continue }
                };
                dispatch(event, peer, pending, tx).await;
            }
        }
    }
}

async fn dispatch(
    event: FromBus,
    peer: &Peer<RoleServer>,
    pending: &Pending,
    tx: &mpsc::UnboundedSender<ToBus>,
) {
    match event {
        FromBus::Message {
            id,
            room,
            from,
            text,
            done,
            human,
        } => {
            eprintln!("[agent] recv ← {from} in {room} (msg {id})");
            let injected = inject(
                peer,
                &text,
                json!({
                    // meta keys must be identifiers: letters, digits, underscores
                    "room": room,
                    "from": from,
                    "msg_id": id.to_string(),
                    "done": done.to_string(),
                    // The one signal that tells the model whether its human asked, or
                    // another agent did. `instructions` splits its restraint on this.
                    "human": human.to_string(),
                }),
            )
            .await;
            // Only advance the delivery cursor for messages that actually
            // reached the transport; if injection failed, the model never
            // saw it, so it must still show up as unread on the next
            // reconnect rather than being silently skipped forever.
            if injected {
                if tx
                    .send(ToBus::Ack {
                        room: room.clone(),
                        last_delivered_id: id,
                    })
                    .is_err()
                {
                    eprintln!(
                        "[agent] failed to queue ack for msg {id} in {room}: bridge channel closed"
                    );
                }
            } else {
                eprintln!("[agent] not acking msg {id} in {room}: injection failed");
            }
        }
        FromBus::Unread { rooms } => {
            // One event for the whole reconnect, not one per room: both so
            // the model isn't hit with a separate injection per room, and
            // because that's what lets the bus keep this bounded on its
            // side (see `RoomUnread` / `send_unread_summaries`).
            let total: i64 = rooms.iter().map(|r| r.count).sum();
            let room_names: Vec<&str> = rooms.iter().map(|r| r.room.as_str()).collect();
            eprintln!(
                "[agent] {total} unread across {} room(s): {}",
                rooms.len(),
                room_names.join(", ")
            );
            let detail = rooms
                .iter()
                .map(|r| format!("- {}: {}", r.room, r.count))
                .collect::<Vec<_>>()
                .join("\n");
            inject(
                peer,
                &format!(
                    "{total} message(s) arrived while you were away, across {} room(s):\n\
                     {detail}\n\
                     Call history with a room name if you want to catch up.",
                    rooms.len()
                ),
                json!({ "kind": "unread", "rooms": room_names.join(",") }),
            )
            .await;
        }
        FromBus::Paused { room, reason } => {
            eprintln!("[agent] room {room} paused: {reason}");
            inject(
                peer,
                &format!("Room \"{room}\" is paused: {reason}"),
                json!({ "room": room, "kind": "paused" }),
            )
            .await;
        }
        FromBus::Registered { name } => eprintln!("[agent] registered as {name}"),
        // The agent bridge always registers via `ToBus::Register`, never
        // `ToBus::Observe` (that's `tail`'s path), so this never actually
        // arrives here — kept only because `FromBus` must be matched
        // exhaustively.
        FromBus::Observing { .. } => {}
        FromBus::Reply { req_id, result } => {
            if let Some(tx) = pending.lock().await.remove(&req_id) {
                let _ = tx.send(FromBus::Reply { req_id, result });
            }
        }
        FromBus::Error { req_id, message } => {
            eprintln!("[agent] bus error: {message}");
            if let Some(id) = req_id
                && let Some(tx) = pending.lock().await.remove(&id)
            {
                let _ = tx.send(FromBus::Error { req_id, message });
            }
        }
        // The agent bridge never sends `WatchPresence`/`WatchEvents` (those
        // are observer-only, issued by `claude-bus tail`/the console), so
        // these never actually arrive here — kept only because `FromBus`
        // must be matched exhaustively.
        FromBus::Presence { .. } | FromBus::Event { .. } => {}
    }
}

/// Returns whether the notification reached the transport — resolves when
/// written, not when Claude processes it, but that's the only signal
/// available, and it's what callers use to decide whether to ack.
async fn inject(peer: &Peer<RoleServer>, content: &str, meta: serde_json::Value) -> bool {
    let notification = CustomNotification::new(
        "notifications/claude/channel",
        Some(json!({ "content": content, "meta": meta })),
    );
    match peer
        .send_notification(ServerNotification::CustomNotification(notification))
        .await
    {
        Ok(()) => {
            eprintln!("[agent] injected into session: {content:.80}");
            true
        }
        Err(e) => {
            eprintln!("[agent] FAILED to inject: {e}");
            false
        }
    }
}
