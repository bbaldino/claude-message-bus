//! Shared test scaffolding for driving a real bus over WebSocket. Extracted
//! from `tests/bus.rs` so `tests/events.rs` (and anything else that wants to
//! drive the bus end-to-end) doesn't have to duplicate it. This file lives in
//! a subdirectory of `tests/`, so cargo does not treat it as its own test
//! binary — every test file that wants these helpers does `mod common;`.

#![allow(dead_code)] // not every helper is used by every test binary that includes this module.

use claude_bus::proto::{FromBus, ToBus};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

pub type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Common plumbing behind all the `start_bus*` variants below: bind an
/// ephemeral port, spawn the bus on it with the given `Guards`/`Keepalive`/
/// `Registry`, and hand back the temp data dir (so a test can open a second
/// `Store` against the same database), the port, and the dir's path.
async fn start_bus_full(
    guards: claude_bus::bus::delivery::Guards,
    keepalive: claude_bus::bus::Keepalive,
    registry: claude_bus::bus::registry::Registry,
) -> (tempfile::TempDir, u16, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let serve_path = path.clone();
    tokio::spawn(async move {
        claude_bus::bus::serve_on_full(listener, serve_path, guards, keepalive, registry)
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    (dir, port, path)
}

/// Same as `start_bus`, but also hands back the bus's data directory so a
/// test can open a second `Store` against the same database and read what
/// the bus wrote.
pub async fn start_bus_with_dir() -> (tempfile::TempDir, u16, std::path::PathBuf) {
    // Rate limit disabled: these tests send bursts deliberately. The
    // exchange cap stays at its default so the runaway test exercises it.
    let guards = claude_bus::bus::delivery::Guards::new(claude_bus::bus::delivery::DEFAULT_CAP, 0);
    start_bus_full(
        guards,
        claude_bus::bus::Keepalive::default(),
        claude_bus::bus::registry::Registry::new(),
    )
    .await
}

pub async fn start_bus() -> (tempfile::TempDir, u16) {
    let (dir, port, _path) = start_bus_with_dir().await;
    (dir, port)
}

/// Same as `start_bus_with_dir`, but the caller supplies `Guards` directly —
/// for tests that need the rate limit *enabled* (`start_bus_with_dir`
/// deliberately disables it, since most callers send bursts on purpose).
pub async fn start_bus_with_guards_dir(
    guards: claude_bus::bus::delivery::Guards,
) -> (tempfile::TempDir, u16, std::path::PathBuf) {
    start_bus_full(
        guards,
        claude_bus::bus::Keepalive::default(),
        claude_bus::bus::registry::Registry::new(),
    )
    .await
}

/// Same as `start_bus_with_dir`, but with an injectable keepalive cadence so
/// the "vanished peer" / keepalive-timeout tests don't have to sleep for the
/// production 30s/90s timeout.
pub async fn start_bus_with_keepalive_dir(
    ping_interval: std::time::Duration,
    pong_timeout: std::time::Duration,
) -> (tempfile::TempDir, u16, std::path::PathBuf) {
    let guards = claude_bus::bus::delivery::Guards::new(claude_bus::bus::delivery::DEFAULT_CAP, 0);
    let keepalive = claude_bus::bus::Keepalive::new(ping_interval, pong_timeout);
    start_bus_full(
        guards,
        keepalive,
        claude_bus::bus::registry::Registry::new(),
    )
    .await
}

pub async fn start_bus_with_keepalive(
    ping_interval: std::time::Duration,
    pong_timeout: std::time::Duration,
) -> (tempfile::TempDir, u16) {
    let (dir, port, _path) = start_bus_with_keepalive_dir(ping_interval, pong_timeout).await;
    (dir, port)
}

/// Same as `start_bus`, but the caller supplies (and keeps a clone of) the
/// `Registry`, so a test can reach in and call `Registry::send_to` directly
/// against a connection the running bus already has live. `cap` sets the
/// exchange guard's cap directly (rate limit stays disabled), so a test that
/// needs to trip `Paused` repeatedly and cheaply can use a cap of 1 instead
/// of burning through the production default of 20 each time.
pub async fn start_bus_with_registry(
    registry: claude_bus::bus::registry::Registry,
    cap: u32,
) -> (tempfile::TempDir, u16) {
    let (dir, port, _path) = start_bus_full(
        claude_bus::bus::delivery::Guards::new(cap, 0),
        claude_bus::bus::Keepalive::default(),
        registry,
    )
    .await;
    (dir, port)
}

pub async fn connect(port: u16, name: &str) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();
    let reg = ToBus::Register {
        name: name.into(),
        host: "testhost".into(),
        cwd: format!("/w/{name}"),
        session_id: Some(format!("sess-{name}")),
    };
    ws.send(Message::text(serde_json::to_string(&reg).unwrap()))
        .await
        .unwrap();
    ws
}

/// Like `connect`, but identifies via `Observe` instead of `Register` — a
/// viewer, not a participant. See `ToBus::Observe`.
pub async fn connect_observer(port: u16, name: &str) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();
    let obs = ToBus::Observe { name: name.into() };
    ws.send(Message::text(serde_json::to_string(&obs).unwrap()))
        .await
        .unwrap();
    ws
}

pub async fn next_event(ws: &mut Ws) -> FromBus {
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for a bus event")
            .expect("stream ended")
            .expect("ws error");
        if let Message::Text(t) = msg {
            return serde_json::from_str(&t).expect("parse FromBus");
        }
    }
}

/// Like `next_event`, but skips over the flood of `FromBus::Message` events
/// from "attacker" that the routing-queue-pressure tests push through
/// `Registry::send_to` directly. Those exist only to occupy the queue while
/// it's being filled; once the writer task gets scheduled it drains and
/// forwards them like any other routed message, so a test waiting for a
/// specific reply afterward has to look past them.
pub async fn next_non_flood_event(ws: &mut Ws) -> FromBus {
    loop {
        let ev = next_event(ws).await;
        if matches!(&ev, FromBus::Message { from, .. } if from == "attacker") {
            continue;
        }
        return ev;
    }
}

pub async fn send(ws: &mut Ws, cmd: &ToBus) {
    ws.send(Message::text(serde_json::to_string(cmd).unwrap()))
        .await
        .unwrap();
}

/// Keeps polling `ws` for `duration` without expecting any particular event.
///
/// `tokio-tungstenite` only flushes its automatic `Pong` reply the next time
/// something reads (or writes) the stream — see `WebSocket::write`'s docs.
/// A connection that is simply waiting (e.g. via a bare `sleep`) is
/// therefore indistinguishable, from the bus's point of view, from a
/// genuinely vanished peer: nobody is pumping it, so no pong goes out. Tests
/// that want a *live* connection to survive an idle period need to pump it
/// like this instead of sleeping past it.
pub async fn pump_for(ws: &mut Ws, duration: std::time::Duration) {
    let deadline = tokio::time::Instant::now() + duration;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let _ = tokio::time::timeout(
            remaining.min(std::time::Duration::from_millis(20)),
            ws.next(),
        )
        .await;
    }
}

pub fn flood_message() -> FromBus {
    FromBus::Message {
        id: 0,
        room: "flood".into(),
        from: "attacker".into(),
        text: "x".into(),
        done: false,
    }
}

/// Keeps a connection's routing queue saturated for as long as `active`
/// stays `true`, by calling `Registry::send_to` directly (the same path
/// other connections' room/DM fan-out goes through) in a tight loop.
///
/// A one-shot fill isn't enough to prove control traffic is immune to
/// routing pressure: the writer task drains the queue as fast as it can
/// forward messages onto the socket, so by the time a test's own
/// request/reply round trip actually reaches the point of contention, a
/// single burst has usually already drained away. Continuously refilling —
/// racing the writer's drain rate — is what reproduces sustained pressure,
/// which is the scenario the review's finding actually describes (a
/// connection that is a member of *several concurrently busy rooms*, not
/// one that received one burst and then went quiet).
///
/// `yield_now` between sends is required, not cosmetic: `tokio::sync::Mutex`
/// resolves synchronously when uncontended, so a bare `while active.load()
/// { send_to(...).await }` never actually yields to the scheduler and starves
/// every other task on a single-threaded runtime — including the writer task
/// this is racing against, and the test's own main task.
pub async fn flood_continuously(
    registry: claude_bus::bus::registry::Registry,
    name: String,
    active: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    while active.load(Ordering::Relaxed) {
        // Re-saturate the queue in one uninterrupted burst (no `.await`
        // yield point inside this inner loop — `try_send` and an
        // uncontended `tokio::sync::Mutex` both resolve synchronously) so
        // that whenever another task — the writer draining it, or this
        // connection's own read loop trying to enqueue a reply — actually
        // gets to run, it is as likely as possible to see the queue at
        // capacity, not mid-drain.
        while registry.send_to(&name, flood_message()).await {}
        tokio::task::yield_now().await;
    }
}
