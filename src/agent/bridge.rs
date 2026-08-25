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
    pub liveness: Liveness,
}

/// A connection that stayed up at least this long resets the backoff to its
/// floor on the next drop, so a bus restart after hours of uptime does not
/// wait up to 30s to reconnect just because of a stale prior outage.
const STABLE_CONNECTION_THRESHOLD: Duration = Duration::from_secs(60);
const BACKOFF_FLOOR: Duration = Duration::from_secs(1);

/// How often the bridge pings the bus, and how long it waits for any inbound
/// frame before deciding the connection is dead.
///
/// Injected the way the bus injects `Keepalive`, and for the same reason: the
/// production cadence is minutes, so a test that used it would have to sleep
/// for minutes.
///
/// The client pings rather than only listening. Relying on the bus's pings
/// alone would need no ticker at all, but it would couple this timeout to the
/// peer's configured cadence — and that cadence is configurable
/// (`Keepalive::new`). Anyone lengthening the bus's ping interval past this
/// timeout would turn every idle connection into a reconnect loop, with
/// nothing in either file to warn them.
///
/// Detection is bounded by `idle_timeout + ping_interval + longest_dispatch`,
/// not just the first two terms. `last_inbound` is set before the frame is
/// parsed and dispatched, so a slow `dispatch` (it awaits
/// `peer.send_notification()`, an unbounded write to the MCP transport if the
/// session has stopped reading) can only delay detection, never cause a false
/// positive — the clock for the *next* idle window doesn't start until
/// dispatch returns and the loop is back in `select!`.
///
/// Neither the ping in the ticker arm nor the register/outbound writes in the
/// other `select!` arms are wrapped in a timeout. A peer that advertises a
/// zero receive window can park any of those sends indefinitely; the loop
/// never re-enters `select!`, so the idle check never runs and the give-up
/// path never fires — the one way this mechanism can defeat itself. Not the
/// bug this branch was built to catch (a lost FIN leaves the send buffer
/// drainable, and a 2-byte ping fits for a very long time), and not a
/// regression: the outbound arm always had this shape. Left unwrapped
/// anyway — the fix is a timeout on every send, which is a bigger change
/// than this doc comment. `connect_async` has the same unbounded-wait
/// property and is out of scope for the same reason.
#[derive(Clone, Copy, Debug)]
pub struct Liveness {
    pub ping_interval: Duration,
    pub idle_timeout: Duration,
}

impl Default for Liveness {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(90),
        }
    }
}

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

    // Checked on a ticker rather than by racing a timer against the read, so
    // the granularity is one interval: detection lands within
    // `idle_timeout + ping_interval` (see `Liveness` for the fuller bound).
    //
    // Left at the default `MissedTickBehavior::Burst` rather than set to
    // `Delay`. A burst can genuinely happen: the loop busy for longer than
    // `ping_interval` while inbound frames keep resetting `last_inbound`
    // produces a run of queued ticks once it comes up for air. That's
    // harmless here — the bus's read loop drops `Ping` on its catch-all arm
    // and counts nothing toward its own timeout, tungstenite answers each
    // one with a pong, and a burst can't cause a false teardown because only
    // the *first* firing checks the deadline; it bails only if the
    // connection is genuinely stale, and every firing after that finds
    // `last_inbound` already fresh. `Delay` would instead push detection out
    // by a full extra interval after any busy period — the wrong trade for a
    // ticker whose entire reason to exist is detecting a stale connection.
    // The bus makes the same call but splits the two duties across separate
    // tickers (`Delay` on its pinger, `Burst` on its timeout checker,
    // `src/bus/mod.rs`); the bridge merges ping and check into this one
    // ticker, so it keeps `Burst`.
    let mut liveness_ticker = tokio::time::interval(cfg.liveness.ping_interval);
    liveness_ticker.tick().await; // skip the immediate first tick
    let mut last_inbound = tokio::time::Instant::now();

    loop {
        tokio::select! {
            outbound = rx.recv() => {
                let Some(cmd) = outbound else { return Ok(()) };
                sink.send(Message::text(serde_json::to_string(&cmd)?)).await?;
            }
            inbound = stream.next() => {
                let Some(msg) = inbound else { return Ok(()) };
                // Any frame, not specifically a pong: it is strictly more
                // information, and a busy connection must not trip the timer
                // just because a pong queued behind a burst of messages.
                last_inbound = tokio::time::Instant::now();
                let Ok(text) = msg?.into_text() else { continue };
                if text.trim().is_empty() { continue }
                let event: FromBus = match serde_json::from_str(&text) {
                    Ok(e) => e,
                    Err(e) => { eprintln!("[agent] unparseable from bus: {e}: {text}"); continue }
                };
                dispatch(event, peer, pending, tx).await;
            }
            _ = liveness_ticker.tick() => {
                // Checked before pinging, so a connection already known to be
                // dead is not written into first.
                if last_inbound.elapsed() > cfg.liveness.idle_timeout {
                    eprintln!(
                        "[agent] no traffic from the bus in {:?}, assuming the connection is dead",
                        cfg.liveness.idle_timeout
                    );
                    anyhow::bail!("no traffic from the bus in {:?}", cfg.liveness.idle_timeout);
                }
                // The ping is also a write, so a dead socket eventually fails
                // here on its own — a second detection path that does not
                // depend on the timer above.
                sink.send(Message::Ping(Vec::new().into())).await?;
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
        FromBus::Registered { name, relayer } => {
            eprintln!(
                "[agent] registered as {name}{}",
                if relayer { " (relayer)" } else { "" }
            );
            // Only a relayer is told. The failure is asymmetric: an agent that wrongly
            // assumes it has no grant behaves correctly, while a relayer that assumes
            // the same defers on its own human's instructions and stalls their work.
            //
            // Per registration rather than once per process, because the grant is
            // recomputed per registration too — a renamed `hub#2` holds none.
            if relayer {
                inject(
                    peer,
                    "You hold a relayer grant on this bus. Your messages are stamped \
                     human=\"true\" and reach other agents carrying your human's \
                     authority, not as agent-to-agent chatter — so they are instructions \
                     to act on, and a recipient asking you to confirm separately is a \
                     round trip your grant exists to remove.\n\n\
                     Because of that, a recipient cannot tell your own words from your \
                     human's by the attribute alone. Attribute explicitly: quote your \
                     human when relaying them, and mark your own reasoning as yours.",
                    json!({ "kind": "relayer_grant" }),
                )
                .await;
            }
        }
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
