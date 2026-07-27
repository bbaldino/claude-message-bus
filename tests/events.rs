use claude_bus::store::Store;
use serde_json::json;

async fn temp_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).await.expect("open store");
    (dir, store)
}

#[tokio::test]
async fn appends_and_reads_back_an_event() {
    let (_d, store) = temp_store().await;
    store
        .append_event(
            "message_sent",
            Some("caas"),
            Some("protocol"),
            json!({"msg_id": 7, "delivered_to": ["dashboard"]}),
        )
        .await
        .unwrap();

    let evs = store.events(10).await.unwrap();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].kind, "message_sent");
    assert_eq!(evs[0].agent.as_deref(), Some("caas"));
    assert_eq!(evs[0].room.as_deref(), Some("protocol"));
    assert_eq!(evs[0].detail["msg_id"], 7);
    assert_eq!(evs[0].detail["delivered_to"][0], "dashboard");
}

#[tokio::test]
async fn nullable_agent_and_room_round_trip_as_none() {
    let (_d, store) = temp_store().await;
    store
        .append_event("bus_started", None, None, json!({}))
        .await
        .unwrap();

    let evs = store.events(10).await.unwrap();
    assert!(evs[0].agent.is_none());
    assert!(evs[0].room.is_none());
}

#[tokio::test]
async fn events_returns_most_recent_first() {
    let (_d, store) = temp_store().await;
    for i in 0..3 {
        store
            .append_event("ack", Some("caas"), Some("r"), json!({"n": i}))
            .await
            .unwrap();
    }
    let evs = store.events(10).await.unwrap();
    assert_eq!(evs[0].detail["n"], 2, "newest first");
    assert_eq!(evs[2].detail["n"], 0);
}

#[tokio::test]
async fn events_for_room_returns_oldest_first_for_interleaving() {
    // The transcript view merges these with messages in chronological order, so this
    // query must hand them over in the order they happened, unlike the others.
    let (_d, store) = temp_store().await;
    for i in 0..3 {
        store
            .append_event(
                "message_sent",
                Some("caas"),
                Some("protocol"),
                json!({"n": i}),
            )
            .await
            .unwrap();
    }
    store
        .append_event(
            "message_sent",
            Some("caas"),
            Some("other"),
            json!({"n": 99}),
        )
        .await
        .unwrap();

    let evs = store.events_for_room("protocol", 10).await.unwrap();
    assert_eq!(evs.len(), 3, "only this room's events");
    assert_eq!(evs[0].detail["n"], 0, "oldest first");
    assert_eq!(evs[2].detail["n"], 2);
}

#[tokio::test]
async fn events_filter_by_agent_and_by_kind() {
    let (_d, store) = temp_store().await;
    store
        .append_event("ack", Some("caas"), Some("r"), json!({}))
        .await
        .unwrap();
    store
        .append_event("room_paused", Some("caas"), Some("r"), json!({}))
        .await
        .unwrap();
    store
        .append_event("ack", Some("dashboard"), Some("r"), json!({}))
        .await
        .unwrap();

    assert_eq!(store.events_for_agent("caas", 10).await.unwrap().len(), 2);
    assert_eq!(store.events_of_kind("ack", 10).await.unwrap().len(), 2);
}

#[tokio::test]
async fn limit_is_respected() {
    let (_d, store) = temp_store().await;
    for i in 0..5 {
        store
            .append_event("ack", Some("caas"), Some("r"), json!({"n": i}))
            .await
            .unwrap();
    }
    assert_eq!(store.events(2).await.unwrap().len(), 2);
}

#[tokio::test]
async fn malformed_detail_json_does_not_poison_reads() {
    // detail_json is TEXT; a bad row should degrade to Null rather than failing the
    // whole query and hiding every other event on the page.
    let (_d, store) = temp_store().await;
    store
        .append_event("ack", Some("caas"), Some("r"), json!({"ok": true}))
        .await
        .unwrap();
    sqlx::query("INSERT INTO events (created_at, kind, agent, room, detail_json) VALUES (1, 'bad', 'x', 'r', 'not json')")
        .execute(store.pool_for_test())
        .await
        .unwrap();

    let evs = store.events(10).await.unwrap();
    assert_eq!(evs.len(), 2, "the bad row must not sink the query");
}

// --- Task 3: events written by the running bus, driven over WebSocket ---

mod common;

use claude_bus::proto::{FromBus, ReplyResult, Target, ToBus};
use common::{
    connect, next_event, send, start_bus_with_dir, start_bus_with_guards_dir,
    start_bus_with_keepalive_dir,
};

#[tokio::test]
async fn a_send_records_delivery_outcome_per_recipient() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await; // Registered

    send(
        &mut a,
        &ToBus::Send {
            req_id: 1,
            target: Target::Agent {
                name: "ghost".into(),
            },
            text: "hello".into(),
            done: false,
        },
    )
    .await;
    next_event(&mut a).await; // Sent reply

    let store = Store::open(&store_dir).await.unwrap();
    let sent: Vec<_> = store.events_of_kind("message_sent", 10).await.unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].detail["queued_for"][0], "ghost",
        "recipient was offline"
    );
    assert_eq!(sent[0].detail["delivered_to"].as_array().unwrap().len(), 0);
}

// The write discipline's first, non-negotiable rule: a logging failure must never fail
// the operation being logged. `append_event`'s result is discarded with `let _ =` at
// every one of its ten call sites specifically so that a broken event log can never
// take the bus operation it accompanies down with it — but nothing asserted that until
// now. This drops the `events` table out from under a live bus, then drives a real
// `Send`, and confirms the message still lands in `messages` and the sender still gets
// its normal `Sent` reply.
#[tokio::test]
async fn a_broken_event_log_does_not_break_the_send_it_would_have_logged() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await; // Registered

    // Open a second `Store` against the same database and drop `events` out from under
    // the running bus. `Store::open` re-applies the (idempotent) schema first, which is
    // harmless here since every table but the one we're about to drop already exists.
    let store = Store::open(&store_dir).await.unwrap();
    sqlx::query("DROP TABLE events")
        .execute(store.pool_for_test())
        .await
        .unwrap();

    send(
        &mut a,
        &ToBus::Send {
            req_id: 1,
            target: Target::Room {
                room: "protocol".into(),
            },
            text: "still works".into(),
            done: false,
        },
    )
    .await;

    match next_event(&mut a).await {
        FromBus::Reply {
            result: ReplyResult::Sent { room, .. },
            ..
        } => assert_eq!(room, "protocol"),
        other => panic!("expected a normal Sent reply despite the broken event log, got {other:?}"),
    }

    let history = store.history("protocol", 10).await.unwrap();
    assert_eq!(history.len(), 1, "the message must still be recorded");
    assert_eq!(history[0].body, "still works");
}

#[tokio::test]
async fn registration_records_both_requested_and_effective_name() {
    // The caas -> caas#2 collision must be visible in the log rather than inferred.
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    let mut b = connect(port, "caas").await; // same name, same host
    next_event(&mut b).await;

    let store = Store::open(&store_dir).await.unwrap();
    let regs = store.events_of_kind("agent_registered", 10).await.unwrap();
    assert_eq!(regs.len(), 2);
    let collided = regs
        .iter()
        .find(|e| e.detail["effective_name"] != e.detail["requested_name"]);
    let collided = collided.expect("the second registration should differ");
    assert_eq!(collided.detail["requested_name"], "caas");
    assert_eq!(collided.detail["effective_name"], "caas#2");
}

#[tokio::test]
async fn an_ack_is_recorded() {
    // The Ack defect was invisible precisely because nothing recorded acks.
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    send(
        &mut a,
        &ToBus::Ack {
            room: "protocol".into(),
            last_delivered_id: 5,
        },
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let store = Store::open(&store_dir).await.unwrap();
    let acks = store.events_of_kind("ack", 10).await.unwrap();
    assert_eq!(acks.len(), 1);
    assert_eq!(acks[0].detail["last_delivered_id"], 5);
}

// The cursor-advance-on-history fix must be as visible in the log as a
// regular Ack: a cursor that moves with no event would reintroduce the
// invisible-state bug this project was built to catch, just through a
// different door than the explicit ToBus::Ack the other `an_ack_is_recorded`
// test covers.
#[tokio::test]
async fn history_driven_cursor_advance_is_recorded_as_an_ack() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await; // Registered
    send(
        &mut a,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut a).await; // Joined

    send(
        &mut a,
        &ToBus::Send {
            req_id: 2,
            target: Target::Room {
                room: "protocol".into(),
            },
            text: "hi".into(),
            done: false,
        },
    )
    .await;
    let msg_id = match next_event(&mut a).await {
        FromBus::Reply {
            result: ReplyResult::Sent { msg_id, .. },
            ..
        } => msg_id,
        other => panic!("expected a Reply to Send, got {other:?}"),
    };

    send(
        &mut a,
        &ToBus::History {
            req_id: 3,
            room: "protocol".into(),
            limit: 10,
        },
    )
    .await;
    next_event(&mut a).await; // Reply to History

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let store = Store::open(&store_dir).await.unwrap();
    let acks = store.events_of_kind("ack", 10).await.unwrap();
    assert_eq!(acks.len(), 1);
    assert_eq!(acks[0].agent.as_deref(), Some("caas"));
    assert_eq!(acks[0].room.as_deref(), Some("protocol"));
    assert_eq!(acks[0].detail["last_delivered_id"], msg_id);
}

#[tokio::test]
async fn a_paused_room_is_recorded() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;

    for i in 0..21 {
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
        next_event(&mut a).await;
    }

    let store = Store::open(&store_dir).await.unwrap();
    let paused = store.events_of_kind("room_paused", 10).await.unwrap();
    assert_eq!(paused.len(), 1);
    assert_eq!(paused[0].room.as_deref(), Some("loop"));
}

#[tokio::test]
async fn a_join_is_recorded() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
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
    next_event(&mut a).await; // Joined reply

    let store = Store::open(&store_dir).await.unwrap();
    let joined = store.events_of_kind("room_joined", 10).await.unwrap();
    assert_eq!(joined.len(), 1);
    assert_eq!(joined[0].agent.as_deref(), Some("caas"));
    assert_eq!(joined[0].room.as_deref(), Some("protocol"));
}

#[tokio::test]
async fn a_resume_is_recorded() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;

    send(
        &mut a,
        &ToBus::Resume {
            req_id: 1,
            room: "loop".into(),
        },
    )
    .await;
    next_event(&mut a).await; // Resumed reply

    let store = Store::open(&store_dir).await.unwrap();
    let resumed = store.events_of_kind("resumed", 10).await.unwrap();
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0].room.as_deref(), Some("loop"));
}

#[tokio::test]
async fn a_rate_limited_send_is_recorded() {
    // `start_bus_with_dir` deliberately disables the rate limit (other tests
    // send bursts on purpose), so this test brings its own `Guards` with the
    // rate limit enabled.
    let guards = claude_bus::bus::delivery::Guards::new(
        claude_bus::bus::delivery::DEFAULT_CAP,
        claude_bus::bus::delivery::DEFAULT_MIN_INTERVAL_MS,
    );
    let (_d, port, store_dir) = start_bus_with_guards_dir(guards).await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;

    // The rate limit rejects a second send to the same room from the same
    // agent that arrives too soon after the first.
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
    next_event(&mut a).await; // Sent reply

    send(
        &mut a,
        &ToBus::Send {
            req_id: 2,
            target: Target::Room {
                room: "protocol".into(),
            },
            text: "immediately after".into(),
            done: false,
        },
    )
    .await;
    match next_event(&mut a).await {
        FromBus::Error { .. } => {}
        other => panic!("expected the second send to be rate limited, got {other:?}"),
    }

    let store = Store::open(&store_dir).await.unwrap();
    let limited = store.events_of_kind("rate_limited", 10).await.unwrap();
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].room.as_deref(), Some("protocol"));
    assert!(limited[0].detail["retry_in_ms"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn a_stored_and_fetched_file_are_recorded() {
    use base64::Engine;
    let (_d, port, store_dir) = start_bus_with_dir().await;
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
    next_event(&mut a).await; // FileStored reply

    send(
        &mut a,
        &ToBus::GetFile {
            req_id: 3,
            room: "protocol".into(),
            key: "schema.txt".into(),
        },
    )
    .await;
    next_event(&mut a).await; // FileContent reply

    let store = Store::open(&store_dir).await.unwrap();
    let stored = store.events_of_kind("file_stored", 10).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].detail["key"], "schema.txt");

    let fetched = store.events_of_kind("file_fetched", 10).await.unwrap();
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].detail["key"], "schema.txt");
}

#[tokio::test]
async fn a_closed_socket_is_recorded_as_socket_closed() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await; // Registered

    drop(a);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let store = Store::open(&store_dir).await.unwrap();
    let disconnects = store
        .events_of_kind("agent_disconnected", 10)
        .await
        .unwrap();
    assert_eq!(disconnects.len(), 1);
    assert_eq!(disconnects[0].agent.as_deref(), Some("caas"));
    assert_eq!(disconnects[0].detail["reason"], "socket_closed");
}

// Regression for the specific defect this event exists to diagnose: a
// hardcoded single reason would make a lost socket indistinguishable from a
// keepalive timeout, and a ghost agent is only diagnosable if the two are
// told apart. Uses the same millisecond-scale `Keepalive` knobs as
// `tests/bus.rs`'s `a_peer_that_stops_answering_pings_is_detached_and_reads_as_queued`
// so this doesn't have to sleep for the production 30s/90s cadence.
#[tokio::test]
async fn a_keepalive_timeout_is_recorded_as_keepalive_timeout() {
    let (_d, port, store_dir) = start_bus_with_keepalive_dir(
        std::time::Duration::from_millis(50),
        std::time::Duration::from_millis(150),
    )
    .await;

    // `victim` registers and is then never read from again, so
    // tokio-tungstenite never gets the chance to auto-reply to the bus's
    // Pings with a Pong (that reply is only flushed on the next read/write
    // call). The connection is kept alive (not dropped) for the whole test:
    // this must be detected by ping/pong silence, not by the socket closing.
    let mut victim = connect(port, "victim").await;
    next_event(&mut victim).await; // Registered

    // A few ping/timeout cycles at 50ms/150ms, comfortably long enough for
    // the bus to have pinged, waited, and given up at least once.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;

    let store = Store::open(&store_dir).await.unwrap();
    let disconnects = store
        .events_of_kind("agent_disconnected", 10)
        .await
        .unwrap();
    assert_eq!(disconnects.len(), 1);
    assert_eq!(disconnects[0].agent.as_deref(), Some("victim"));
    assert_eq!(disconnects[0].detail["reason"], "keepalive_timeout");

    drop(victim);
}
