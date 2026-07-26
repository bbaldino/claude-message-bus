use claude_bus::proto::{FromBus, ReplyResult, Target, ToBus};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn start_bus() -> (tempfile::TempDir, u16) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        // Rate limit disabled: these tests send bursts deliberately. The
        // exchange cap stays at its default so the runaway test exercises it.
        let guards =
            claude_bus::bus::delivery::Guards::new(claude_bus::bus::delivery::DEFAULT_CAP, 0);
        claude_bus::bus::serve_on_with(listener, path, guards)
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    (dir, port)
}

/// Same as `start_bus`, but with an injectable keepalive cadence so the
/// "vanished peer" tests don't have to sleep for the production 30s/90s
/// timeout.
async fn start_bus_with_keepalive(
    ping_interval: std::time::Duration,
    pong_timeout: std::time::Duration,
) -> (tempfile::TempDir, u16) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let guards =
            claude_bus::bus::delivery::Guards::new(claude_bus::bus::delivery::DEFAULT_CAP, 0);
        let keepalive = claude_bus::bus::Keepalive::new(ping_interval, pong_timeout);
        claude_bus::bus::serve_on_with_keepalive(listener, path, guards, keepalive)
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    (dir, port)
}

/// Same as `start_bus`, but the caller supplies (and keeps a clone of) the
/// `Registry`, so a test can reach in and call `Registry::send_to` directly
/// against a connection the running bus already has live. `cap` sets the
/// exchange guard's cap directly (rate limit stays disabled), so a test that
/// needs to trip `Paused` repeatedly and cheaply can use a cap of 1 instead
/// of burning through the production default of 20 each time.
async fn start_bus_with_registry(
    registry: claude_bus::bus::registry::Registry,
    cap: u32,
) -> (tempfile::TempDir, u16) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let guards = claude_bus::bus::delivery::Guards::new(cap, 0);
        claude_bus::bus::serve_on_full(
            listener,
            path,
            guards,
            claude_bus::bus::Keepalive::default(),
            registry,
        )
        .await
        .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    (dir, port)
}

async fn connect(port: u16, name: &str) -> Ws {
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

async fn next_event(ws: &mut Ws) -> FromBus {
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
async fn next_non_flood_event(ws: &mut Ws) -> FromBus {
    loop {
        let ev = next_event(ws).await;
        if matches!(&ev, FromBus::Message { from, .. } if from == "attacker") {
            continue;
        }
        return ev;
    }
}

async fn send(ws: &mut Ws, cmd: &ToBus) {
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
async fn pump_for(ws: &mut Ws, duration: std::time::Duration) {
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

#[tokio::test]
async fn registering_confirms_the_effective_name() {
    let (_d, port) = start_bus().await;
    let mut ws = connect(port, "caas").await;
    match next_event(&mut ws).await {
        FromBus::Registered { name } => assert_eq!(name, "caas"),
        other => panic!("expected Registered, got {other:?}"),
    }
}

#[tokio::test]
async fn a_second_register_on_one_connection_is_refused_and_identity_is_unchanged() {
    // If a second Register were accepted, `attach` would mint a fresh
    // effective name (e.g. "caas#2") for the same socket, and only that
    // second name would ever be detached on disconnect: "caas" would stay
    // registered online forever, silently absorbing anything addressed to
    // it. The connection's identity must be fixed at the first Register.
    let (_d, port) = start_bus().await;
    let mut ws = connect(port, "caas").await;
    match next_event(&mut ws).await {
        FromBus::Registered { name } => assert_eq!(name, "caas"),
        other => panic!("expected Registered, got {other:?}"),
    }

    send(
        &mut ws,
        &ToBus::Register {
            name: "caas".into(),
            host: "testhost".into(),
            cwd: "/w/caas".into(),
            session_id: None,
        },
    )
    .await;
    match next_event(&mut ws).await {
        FromBus::Error { req_id, message } => {
            assert_eq!(req_id, None);
            assert!(
                message.contains("caas"),
                "error should name the existing identity: {message}"
            );
        }
        other => panic!("expected Error refusing the second Register, got {other:?}"),
    }

    // The connection is still "caas": commands still work, and the identity
    // that appears in a subsequent message is still the original one.
    let mut other = connect(port, "dashboard").await;
    next_event(&mut other).await; // Registered

    send(
        &mut ws,
        &ToBus::Send {
            req_id: 1,
            target: Target::Agent {
                name: "dashboard".into(),
            },
            text: "still caas".into(),
            done: false,
        },
    )
    .await;
    next_event(&mut ws).await; // Reply to the sender

    match next_event(&mut other).await {
        FromBus::Message { from, .. } => assert_eq!(from, "caas"),
        other => panic!("expected Message from caas, got {other:?}"),
    }
}

#[tokio::test]
async fn a_dm_reaches_a_connected_agent() {
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    let mut b = connect(port, "dashboard").await;
    next_event(&mut a).await; // Registered
    next_event(&mut b).await; // Registered

    send(
        &mut a,
        &ToBus::Send {
            req_id: 1,
            target: Target::Agent {
                name: "dashboard".into(),
            },
            text: "hello".into(),
            done: false,
        },
    )
    .await;

    // The sender is told it was delivered, not merely queued.
    match next_event(&mut a).await {
        FromBus::Reply {
            req_id,
            result:
                ReplyResult::Sent {
                    delivered_to,
                    queued_for,
                    room,
                    ..
                },
        } => {
            assert_eq!(req_id, 1);
            assert_eq!(room, "dm:caas|dashboard");
            assert_eq!(delivered_to, vec!["dashboard".to_string()]);
            assert!(queued_for.is_empty());
        }
        other => panic!("expected Sent, got {other:?}"),
    }

    match next_event(&mut b).await {
        FromBus::Message {
            from, text, room, ..
        } => {
            assert_eq!(from, "caas");
            assert_eq!(text, "hello");
            assert_eq!(room, "dm:caas|dashboard");
        }
        other => panic!("expected Message, got {other:?}"),
    }
}

#[tokio::test]
async fn a_message_to_an_offline_agent_reports_queued_not_delivered() {
    // This is the POC 3 correction: never tell the model "delivered" when it wasn't.
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;

    send(
        &mut a,
        &ToBus::Send {
            req_id: 9,
            target: Target::Agent {
                name: "ghost".into(),
            },
            text: "anyone there".into(),
            done: false,
        },
    )
    .await;

    match next_event(&mut a).await {
        FromBus::Reply {
            result:
                ReplyResult::Sent {
                    delivered_to,
                    queued_for,
                    ..
                },
            ..
        } => {
            assert!(delivered_to.is_empty(), "nobody was online");
            assert_eq!(queued_for, vec!["ghost".to_string()]);
        }
        other => panic!("expected Sent, got {other:?}"),
    }
}

#[tokio::test]
async fn a_room_message_fans_out_to_all_other_members() {
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    let mut b = connect(port, "dashboard").await;
    next_event(&mut a).await;
    next_event(&mut b).await;

    send(
        &mut a,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut a).await;
    send(
        &mut b,
        &ToBus::Join {
            req_id: 2,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut b).await;

    send(
        &mut a,
        &ToBus::Send {
            req_id: 3,
            target: Target::Room {
                room: "protocol".into(),
            },
            text: "proposal".into(),
            done: false,
        },
    )
    .await;

    match next_event(&mut a).await {
        FromBus::Reply {
            result: ReplyResult::Sent { delivered_to, .. },
            ..
        } => {
            assert_eq!(
                delivered_to,
                vec!["dashboard".to_string()],
                "sender must not receive its own message"
            );
        }
        other => panic!("expected Sent, got {other:?}"),
    }
    match next_event(&mut b).await {
        FromBus::Message { text, .. } => assert_eq!(text, "proposal"),
        other => panic!("expected Message, got {other:?}"),
    }
}

#[tokio::test]
async fn history_returns_what_was_said() {
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;

    send(
        &mut a,
        &ToBus::Send {
            req_id: 1,
            target: Target::Room {
                room: "protocol".into(),
            },
            text: "first".into(),
            done: false,
        },
    )
    .await;
    next_event(&mut a).await;

    send(
        &mut a,
        &ToBus::History {
            req_id: 2,
            room: "protocol".into(),
            limit: 10,
        },
    )
    .await;
    match next_event(&mut a).await {
        FromBus::Reply {
            result: ReplyResult::History { messages },
            ..
        } => {
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].text, "first");
            assert_eq!(messages[0].from, "caas");
        }
        other => panic!("expected History, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unknown_room_in_history_lists_valid_rooms() {
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    send(
        &mut a,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut a).await;

    send(
        &mut a,
        &ToBus::History {
            req_id: 2,
            room: "nope".into(),
            limit: 10,
        },
    )
    .await;
    match next_event(&mut a).await {
        FromBus::Error { message, req_id } => {
            assert_eq!(req_id, Some(2));
            assert!(
                message.contains("protocol"),
                "error must name valid rooms: {message}"
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn reconnecting_gets_an_unread_summary_not_the_backlog() {
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    let mut b = connect(port, "dashboard").await;
    next_event(&mut a).await;
    next_event(&mut b).await;

    send(
        &mut a,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut a).await;
    send(
        &mut b,
        &ToBus::Join {
            req_id: 2,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut b).await;

    drop(b); // dashboard goes away
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    for i in 0..3 {
        send(
            &mut a,
            &ToBus::Send {
                req_id: 10 + i,
                target: Target::Room {
                    room: "protocol".into(),
                },
                text: format!("while you were out {i}"),
                done: false,
            },
        )
        .await;
        next_event(&mut a).await;
    }

    let mut b2 = connect(port, "dashboard").await;
    next_event(&mut b2).await; // Registered
    match next_event(&mut b2).await {
        FromBus::Unread { room, count } => {
            assert_eq!(room, "protocol");
            assert_eq!(count, 3, "summary, not replay");
        }
        other => panic!("expected Unread, got {other:?}"),
    }
}

#[tokio::test]
async fn files_round_trip_through_the_bus() {
    use base64::Engine;
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    send(
        &mut a,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut a).await;

    let content = base64::engine::general_purpose::STANDARD.encode(b"schema goes here");
    send(
        &mut a,
        &ToBus::PutFile {
            req_id: 2,
            room: "protocol".into(),
            key: "schema.txt".into(),
            content_b64: content,
            content_type: Some("text/plain".into()),
        },
    )
    .await;
    match next_event(&mut a).await {
        FromBus::Reply {
            result: ReplyResult::FileStored { key, size, .. },
            ..
        } => {
            assert_eq!(key, "schema.txt");
            assert_eq!(size, 16);
        }
        other => panic!("expected FileStored, got {other:?}"),
    }

    send(
        &mut a,
        &ToBus::GetFile {
            req_id: 3,
            room: "protocol".into(),
            key: "schema.txt".into(),
        },
    )
    .await;
    match next_event(&mut a).await {
        FromBus::Reply {
            result: ReplyResult::FileContent { content_b64, .. },
            ..
        } => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(content_b64)
                .unwrap();
            assert_eq!(bytes, b"schema goes here");
        }
        other => panic!("expected FileContent, got {other:?}"),
    }
}

#[tokio::test]
async fn getting_a_missing_file_names_the_files_that_do_exist() {
    // The task's error-message constraint ("name valid alternatives, not a
    // bare not-found") applies here just as much as to unknown rooms. This
    // is bespoke formatting logic in the GetFile arm, not a pass-through
    // over an already-tested Store call, so it needs its own assertion.
    use base64::Engine;
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    send(
        &mut a,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut a).await;

    let content = base64::engine::general_purpose::STANDARD.encode(b"schema goes here");
    send(
        &mut a,
        &ToBus::PutFile {
            req_id: 2,
            room: "protocol".into(),
            key: "schema.txt".into(),
            content_b64: content,
            content_type: Some("text/plain".into()),
        },
    )
    .await;
    next_event(&mut a).await; // FileStored

    send(
        &mut a,
        &ToBus::GetFile {
            req_id: 3,
            room: "protocol".into(),
            key: "missing.txt".into(),
        },
    )
    .await;
    match next_event(&mut a).await {
        FromBus::Error { req_id, message } => {
            assert_eq!(req_id, Some(3));
            assert!(
                message.contains("schema.txt"),
                "error must name the files that do exist: {message}"
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn the_exchange_cap_pauses_a_runaway_room() {
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;

    // Default cap is 20; the 21st send must be refused.
    for i in 0..20 {
        send(
            &mut a,
            &ToBus::Send {
                req_id: 100 + i,
                target: Target::Room {
                    room: "loop".into(),
                },
                text: format!("m{i}"),
                done: false,
            },
        )
        .await;
        match next_event(&mut a).await {
            FromBus::Reply { .. } => {}
            other => panic!("message {i} should have been accepted, got {other:?}"),
        }
    }

    send(
        &mut a,
        &ToBus::Send {
            req_id: 999,
            target: Target::Room {
                room: "loop".into(),
            },
            text: "one too many".into(),
            done: false,
        },
    )
    .await;
    match next_event(&mut a).await {
        FromBus::Paused { room, .. } => assert_eq!(room, "loop"),
        other => panic!("expected Paused, got {other:?}"),
    }

    // The outstanding request must also resolve promptly — via a truthful
    // Error naming the pause — rather than leaving the sender to block for
    // the full request timeout and conclude the bus is unreachable.
    match next_event(&mut a).await {
        FromBus::Error { req_id, message } => {
            assert_eq!(req_id, Some(999));
            assert!(
                message.to_lowercase().contains("paused"),
                "must name the pause: {message}"
            );
            assert!(
                message.to_lowercase().contains("resume"),
                "must point at the resume path: {message}"
            );
            assert!(
                !message.to_lowercase().contains("unreachable"),
                "must not suggest the bus is down: {message}"
            );
        }
        other => panic!("expected Error resolving the paused send, got {other:?}"),
    }

    send(
        &mut a,
        &ToBus::Resume {
            req_id: 1000,
            room: "loop".into(),
        },
    )
    .await;
    match next_event(&mut a).await {
        FromBus::Reply {
            result: ReplyResult::Resumed { room },
            ..
        } => assert_eq!(room, "loop"),
        other => panic!("expected Resumed, got {other:?}"),
    }
}

// Regression for the defect this fix round targets: nothing pinged
// connections, so a peer whose host vanished stayed "online" forever and
// `send` kept reporting `delivered` for messages nobody would ever receive.
//
// This is not a socket-close test — closing a socket cleanly was already
// detected before this fix, via `stream.next()` returning `None`. The
// failure mode here is a peer that is silent but whose TCP connection is
// still open (laptop lid closed, cable pulled, NAT black-holing): `victim`
// below never drops its connection and never sends a pong, simulating
// exactly that.
#[tokio::test]
async fn a_peer_that_stops_answering_pings_is_detached_and_reads_as_queued() {
    let (_d, port) = start_bus_with_keepalive(
        std::time::Duration::from_millis(50),
        std::time::Duration::from_millis(150),
    )
    .await;

    let mut a = connect(port, "caas").await;
    next_event(&mut a).await; // Registered

    // `victim` registers and is read from exactly once, for its own
    // Registered reply. After that its stream is never polled again, so
    // tokio-tungstenite never gets the chance to auto-reply to the bus's
    // Pings with a Pong (that reply is only flushed on the next read/write/
    // flush call — see tungstenite's `WebSocket::write` docs). The
    // connection itself is kept alive in scope for the whole test: this
    // must be detected by ping/pong silence, not by the socket closing.
    let mut victim = connect(port, "victim").await;
    next_event(&mut victim).await; // Registered

    // A few ping/timeout cycles at 50ms/150ms — comfortably short of any
    // reasonable test timeout, comfortably long enough for the bus to have
    // pinged, waited, and given up at least once. `a` is pumped (not merely
    // slept past) so *it* keeps answering pings and stays online; only
    // `victim` goes silent.
    pump_for(&mut a, std::time::Duration::from_millis(600)).await;

    send(&mut a, &ToBus::ListAgents { req_id: 1 }).await;
    match next_event(&mut a).await {
        FromBus::Reply {
            result: ReplyResult::Agents { agents },
            ..
        } => {
            let victim_agent = agents
                .iter()
                .find(|ag| ag.name == "victim")
                .expect("victim should still be a known agent, just not an online one");
            assert!(
                !victim_agent.online,
                "victim stopped answering pings and must no longer read as online"
            );
        }
        other => panic!("expected Agents, got {other:?}"),
    }

    send(
        &mut a,
        &ToBus::Send {
            req_id: 2,
            target: Target::Agent {
                name: "victim".into(),
            },
            text: "are you there".into(),
            done: false,
        },
    )
    .await;
    match next_event(&mut a).await {
        FromBus::Reply {
            result:
                ReplyResult::Sent {
                    delivered_to,
                    queued_for,
                    ..
                },
            ..
        } => {
            assert!(
                delivered_to.is_empty(),
                "a vanished peer must never be reported as delivered"
            );
            assert_eq!(queued_for, vec!["victim".to_string()]);
        }
        other => panic!("expected Sent, got {other:?}"),
    }

    // Keep the never-polled connection alive until the assertions above are
    // done, so its silence — not a dropped socket — is what the bus reacted
    // to.
    drop(victim);
}

fn flood_message() -> FromBus {
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
async fn flood_continuously(
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

// Regression for fix round 1's follow-up defect: the connection's own
// command replies used to share one queue with inbound fan-out routed to it
// as a recipient. An agent that is a member of busy rooms could have its
// *inbound* queue filled by *other* agents' traffic, and if that coincided
// with a reply to one of its *own* requests, the reply got dropped —
// `Handler::request`'s oneshot never resolved, and the tool call eventually
// reported the bus as unreachable on a connection that was never
// unreachable. The fix splits the control (own replies) and routing
// (inbound fan-out) queues so pressure on one can never starve the other.
#[tokio::test]
async fn a_full_routing_queue_does_not_starve_the_connections_own_replies() {
    let registry = claude_bus::bus::registry::Registry::new();
    let (_d, port) =
        start_bus_with_registry(registry.clone(), claude_bus::bus::delivery::DEFAULT_CAP).await;

    let mut victim = connect(port, "victim").await;
    let name = match next_event(&mut victim).await {
        FromBus::Registered { name } => name,
        other => panic!("expected Registered, got {other:?}"),
    };

    // Sanity check: `Registry::send_to` does fill and then report full at
    // exactly the routing queue's capacity, directly against a live
    // connection (this is the same contract `registry::tests::
    // send_to_a_full_channel_reports_failure_not_delivery` covers in
    // isolation, re-checked here against the real `connection()` wiring).
    let mut queued = 0usize;
    while registry.send_to(&name, flood_message()).await {
        queued += 1;
        assert!(
            queued <= claude_bus::bus::registry::CHANNEL_CAPACITY * 2,
            "send_to never reported the queue full; it should have stopped \
             accepting after CHANNEL_CAPACITY sends"
        );
    }
    assert_eq!(
        queued,
        claude_bus::bus::registry::CHANNEL_CAPACITY,
        "the routing queue should fill at exactly its capacity"
    );

    // Keep re-filling it for the duration of the actual request/reply
    // round trips below — a one-shot fill drains away as soon as the
    // writer task gets scheduled, which happens before `victim`'s own
    // reply is even sent; sustained pressure is what the review's finding
    // describes (an agent that is a member of *several concurrently busy
    // rooms*).
    let active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flooder = tokio::spawn(flood_continuously(
        registry.clone(),
        name.clone(),
        active.clone(),
    ));

    // With the routing queue kept saturated, victim's own requests — sent
    // over its own live connection — must still get replies. They travel
    // the separate control channel, so routing pressure must not touch
    // them. This is repeated rather than sent once: exactly when a reply's
    // `try_send` lands relative to the flooder's and writer's own
    // scheduling is not something a test can pin down turn-by-turn, so many
    // independent attempts under sustained pressure is what makes this
    // reliably distinguish "never drops" from "usually doesn't drop".
    let attempts = 300u64;
    for req_id in 0..attempts {
        send(&mut victim, &ToBus::ListRooms { req_id }).await;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            next_non_flood_event(&mut victim),
        )
        .await;
        match result {
            Ok(FromBus::Reply {
                req_id: got_id,
                result: ReplyResult::Rooms { .. },
            }) => assert_eq!(got_id, req_id),
            Ok(other) => panic!("expected a Reply to ListRooms {req_id}, got {other:?}"),
            Err(_) => {
                active.store(false, std::sync::atomic::Ordering::Relaxed);
                let _ = flooder.await;
                panic!(
                    "victim's own Reply to request {req_id} never arrived while its \
                     routing queue was kept full — the control and routing channels \
                     are not actually independent"
                );
            }
        }
    }

    active.store(false, std::sync::atomic::Ordering::Relaxed);
    let _ = flooder.await;
}

// Same pressure, but against the specific two-events-back-to-back path the
// review called out as the worst instance: `FromBus::Paused` and its
// resolving `FromBus::Error` are two sequential `try_send`s with no `.await`
// (and so no drain opportunity) between them. If the first drops for
// capacity, the second almost certainly does too — collapsing the fallback
// that exists so a paused room never produces a false "bus unreachable"
// timeout. Both must still arrive even with the routing queue completely
// full.
#[tokio::test]
async fn a_full_routing_queue_does_not_swallow_the_pause_notification_or_its_resolving_error() {
    let registry = claude_bus::bus::registry::Registry::new();
    // Cap of 1: every room pauses on its second message, so one iteration
    // below reaches the Paused+Error pair in two round trips instead of the
    // production default's twenty-one. As with the sibling test above, the
    // exact instant a `try_send` lands relative to the flooder's and
    // writer's own scheduling isn't something a test can pin down
    // turn-by-turn, so this repeats the trip many times, across fresh rooms
    // (the guard's pause state is per-room), rather than tripping it once.
    let (_d, port) = start_bus_with_registry(registry.clone(), 1).await;

    let mut a = connect(port, "caas").await;
    let name = match next_event(&mut a).await {
        FromBus::Registered { name } => name,
        other => panic!("expected Registered, got {other:?}"),
    };

    let active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flooder = tokio::spawn(flood_continuously(
        registry.clone(),
        name.clone(),
        active.clone(),
    ));

    let fail = |active: &std::sync::Arc<std::sync::atomic::AtomicBool>, msg: String| -> ! {
        active.store(false, std::sync::atomic::Ordering::Relaxed);
        panic!("{msg}");
    };

    let iterations = 150u64;
    for i in 0..iterations {
        let room = format!("loop{i}");

        send(
            &mut a,
            &ToBus::Send {
                req_id: i * 10,
                target: Target::Room { room: room.clone() },
                text: "one".into(),
                done: false,
            },
        )
        .await;
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            next_non_flood_event(&mut a),
        )
        .await
        {
            Ok(FromBus::Reply { .. }) => {}
            Ok(other) => fail(
                &active,
                format!("iteration {i}: expected Reply, got {other:?}"),
            ),
            Err(_) => fail(
                &active,
                format!("iteration {i}: first message's Reply never arrived under pressure"),
            ),
        }

        send(
            &mut a,
            &ToBus::Send {
                req_id: i * 10 + 1,
                target: Target::Room { room: room.clone() },
                text: "two, over the cap of 1".into(),
                done: false,
            },
        )
        .await;
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            next_non_flood_event(&mut a),
        )
        .await
        {
            Ok(FromBus::Paused { room: r, .. }) => assert_eq!(r, room),
            Ok(other) => fail(
                &active,
                format!("iteration {i}: expected Paused, got {other:?}"),
            ),
            Err(_) => fail(
                &active,
                format!("iteration {i}: Paused never arrived under routing-queue pressure"),
            ),
        }
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            next_non_flood_event(&mut a),
        )
        .await
        {
            Ok(FromBus::Error { req_id, message }) => {
                assert_eq!(req_id, Some(i * 10 + 1));
                assert!(message.to_lowercase().contains("paused"));
            }
            Ok(other) => fail(
                &active,
                format!("iteration {i}: expected the resolving Error, got {other:?}"),
            ),
            Err(_) => fail(
                &active,
                format!(
                    "iteration {i}: the resolving Error never arrived under routing-queue \
                     pressure — Paused and Error are sent back to back with nothing \
                     draining between them, so this is the case most likely to drop the \
                     second"
                ),
            ),
        }
    }

    active.store(false, std::sync::atomic::Ordering::Relaxed);
    let _ = flooder.await;
}
