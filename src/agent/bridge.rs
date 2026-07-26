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

pub async fn run(
    cfg: BridgeConfig,
    mut rx: mpsc::UnboundedReceiver<ToBus>,
    peer: Peer<RoleServer>,
    pending: Pending,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        match connect_once(&cfg, &mut rx, &peer, &pending).await {
            Ok(()) => eprintln!("[agent] bus connection closed"),
            Err(e) => eprintln!("[agent] bus error: {e}"),
        }
        eprintln!("[agent] reconnecting in {:?}", backoff);
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

async fn connect_once(
    cfg: &BridgeConfig,
    rx: &mut mpsc::UnboundedReceiver<ToBus>,
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
                dispatch(event, peer, pending, rx).await;
            }
        }
    }
}

async fn dispatch(
    event: FromBus,
    peer: &Peer<RoleServer>,
    pending: &Pending,
    _rx: &mut mpsc::UnboundedReceiver<ToBus>,
) {
    match event {
        FromBus::Message {
            id,
            room,
            from,
            text,
            done,
        } => {
            eprintln!("[agent] recv ← {from} in {room} (msg {id})");
            inject(
                peer,
                &text,
                json!({
                    // meta keys must be identifiers: letters, digits, underscores
                    "room": room,
                    "from": from,
                    "msg_id": id.to_string(),
                    "done": done.to_string(),
                }),
            )
            .await;
        }
        FromBus::Unread { room, count } => {
            eprintln!("[agent] {count} unread in {room}");
            inject(
                peer,
                &format!(
                    "{count} message(s) arrived in room \"{room}\" while you were away. \
                     Call history with room=\"{room}\" if you want to catch up."
                ),
                json!({ "room": room, "kind": "unread" }),
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
    }
}

async fn inject(peer: &Peer<RoleServer>, content: &str, meta: serde_json::Value) {
    let notification = CustomNotification::new(
        "notifications/claude/channel",
        Some(json!({ "content": content, "meta": meta })),
    );
    match peer
        .send_notification(ServerNotification::CustomNotification(notification))
        .await
    {
        // Resolves when written to the transport, not when Claude processes it.
        Ok(()) => eprintln!("[agent] injected into session: {content:.80}"),
        Err(e) => eprintln!("[agent] FAILED to inject: {e}"),
    }
}
