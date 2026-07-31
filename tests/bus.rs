mod common;

use claude_bus::proto::{FromBus, ReplyResult, Target, ToBus};
use claude_bus::store::Store;
use common::{
    agent_is_online, connect, connect_human, connect_observer, flood_continuously, flood_message,
    next_event, next_non_flood_event, pump_for, send, start_bus, start_bus_with_dir,
    start_bus_with_guards_dir, start_bus_with_keepalive, start_bus_with_registry, wait_until,
};

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
            human: false,
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
    assert!(
        wait_until(|| async { !agent_is_online(port, "dashboard").await }).await,
        "dashboard never went offline after its connection was dropped"
    );

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
        FromBus::Unread { rooms } => {
            assert_eq!(rooms.len(), 1);
            assert_eq!(rooms[0].room, "protocol");
            assert_eq!(rooms[0].count, 3, "summary, not replay");
        }
        other => panic!("expected Unread, got {other:?}"),
    }
}

// Regression for fix round 3's second finding: `send_unread_summaries` used
// to `try_send` one `FromBus::Unread` per room with unread messages. An
// agent reconnecting into enough such rooms (CONTROL_CHANNEL_CAPACITY = 16)
// could exceed the control channel's capacity purely from its own
// Register-time burst — dropping a Registered, an Unread, or a Reply to
// whatever command got pipelined right after registering. Same failure
// family as fix round 2 (dropped control-plane event → stuck oneshot or
// false unreachability), reached through a different door. The fix
// coalesces all rooms into a single `FromBus::Unread { rooms }` event, which
// is O(1) regardless of room count — bounded by construction, not estimate.
#[tokio::test]
async fn reconnect_unread_summary_is_one_event_regardless_of_room_count() {
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    let mut b = connect(port, "dashboard").await;
    next_event(&mut a).await;
    next_event(&mut b).await;

    // Comfortably over CONTROL_CHANNEL_CAPACITY (16): the old per-room
    // implementation would `try_send` this many separate `Unread` events
    // (plus the `Registered` already ahead of them) into a 16-slot channel.
    const ROOM_COUNT: usize = 20;
    let rooms: Vec<String> = (0..ROOM_COUNT).map(|i| format!("room{i}")).collect();

    for (i, room) in rooms.iter().enumerate() {
        send(
            &mut a,
            &ToBus::Join {
                req_id: i as u64,
                room: room.clone(),
            },
        )
        .await;
        next_event(&mut a).await;
        send(
            &mut b,
            &ToBus::Join {
                req_id: 1000 + i as u64,
                room: room.clone(),
            },
        )
        .await;
        next_event(&mut b).await;
    }

    drop(b); // dashboard goes away
    assert!(
        wait_until(|| async { !agent_is_online(port, "dashboard").await }).await,
        "dashboard never went offline after its connection was dropped"
    );

    for (i, room) in rooms.iter().enumerate() {
        send(
            &mut a,
            &ToBus::Send {
                req_id: 2000 + i as u64,
                target: Target::Room { room: room.clone() },
                text: "while you were out".into(),
                done: false,
            },
        )
        .await;
        next_event(&mut a).await;
    }

    let mut b2 = connect(port, "dashboard").await;
    next_event(&mut b2).await; // Registered

    // The whole summary must be exactly one event, with every room
    // represented — not the first of twenty separate ones.
    match next_event(&mut b2).await {
        FromBus::Unread { rooms: got } => {
            assert_eq!(
                got.len(),
                ROOM_COUNT,
                "expected one combined event listing all {ROOM_COUNT} rooms, got {} rooms: {got:?}",
                got.len()
            );
            let mut seen: std::collections::HashMap<String, i64> =
                got.into_iter().map(|r| (r.room, r.count)).collect();
            for room in &rooms {
                assert_eq!(
                    seen.remove(room),
                    Some(1),
                    "room {room} should be represented with exactly 1 unread message"
                );
            }
            assert!(
                seen.is_empty(),
                "unexpected extra rooms in summary: {seen:?}"
            );
        }
        other => panic!("expected a single Unread event, got {other:?}"),
    }

    // A command pipelined right after registering — sharing the same
    // control channel the Unread summary just went through — must still get
    // its own reply.
    send(&mut b2, &ToBus::ListRooms { req_id: 9999 }).await;
    match tokio::time::timeout(std::time::Duration::from_secs(5), next_event(&mut b2)).await {
        Ok(FromBus::Reply {
            req_id,
            result: ReplyResult::Rooms { .. },
        }) => assert_eq!(req_id, 9999),
        Ok(other) => panic!("expected a Reply to ListRooms, got {other:?}"),
        Err(_) => panic!(
            "the Reply to a command issued right after registering never arrived — \
             the Register-time Unread burst must have exhausted the control channel"
        ),
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

// --- observer mode: a viewer is not a participant ---
//
// `claude-bus tail` used to register as a genuine agent and `Join` the room
// it watched, which permanently polluted `agents`/`room_members` with every
// dead `tail-<pid>` that ever ran, and made every future `send` to that room
// report the (probably long-gone) tail process as `queued_for`. These tests
// pin the fix: an observer identifies via `Observe`/watches via `Watch`,
// never touches `Store`, and is invisible to every place a real member would
// show up.

#[tokio::test]
async fn an_observer_receives_room_traffic_without_being_a_recipient() {
    let (_d, port) = start_bus().await;

    let mut sender = connect(port, "caas").await;
    next_event(&mut sender).await; // Registered
    send(
        &mut sender,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut sender).await; // Joined

    let mut watcher = connect_observer(port, "tail-1").await;
    next_event(&mut watcher).await; // Observing
    send(
        &mut watcher,
        &ToBus::Watch {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    match next_event(&mut watcher).await {
        FromBus::Reply {
            result: ReplyResult::Watching { room },
            ..
        } => assert_eq!(room, "protocol"),
        other => panic!("expected a Reply to Watch, got {other:?}"),
    }

    send(
        &mut sender,
        &ToBus::Send {
            req_id: 2,
            target: Target::Room {
                room: "protocol".into(),
            },
            text: "hello".into(),
            done: false,
        },
    )
    .await;

    // `caas` is the only *member* of the room — the observer must not be
    // counted as a recipient at all, in either column. Sabotage: with the
    // old tail.rs (which sent `Register` + `Join`), the observer would be a
    // real member and would show up in `delivered_to`, failing this.
    match next_event(&mut sender).await {
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
                "the observer must never appear in delivered_to: {delivered_to:?}"
            );
            assert!(
                queued_for.is_empty(),
                "the observer must never appear in queued_for: {queued_for:?}"
            );
        }
        other => panic!("expected a Reply to Send, got {other:?}"),
    }

    // The observer still gets the live traffic, despite not being a member.
    match next_event(&mut watcher).await {
        FromBus::Message {
            room, from, text, ..
        } => {
            assert_eq!(room, "protocol");
            assert_eq!(from, "caas");
            assert_eq!(text, "hello");
        }
        other => panic!("expected a live Message, got {other:?}"),
    }
}

#[tokio::test]
async fn an_observer_leaves_no_trace_after_disconnecting() {
    let (_d, port) = start_bus().await;

    let mut caas = connect(port, "caas").await;
    next_event(&mut caas).await; // Registered
    send(
        &mut caas,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut caas).await; // Joined

    let mut watcher = connect_observer(port, "tail-99").await;
    next_event(&mut watcher).await; // Observing
    send(
        &mut watcher,
        &ToBus::Watch {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut watcher).await; // Reply

    drop(watcher);
    // Give the bus's connection-teardown path time to run before asserting.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Sabotage: with the old implementation, `tail-99` would still be in
    // `agents` (an `upsert_agent` row is never removed, only marked
    // offline) and in `protocol`'s member list (never removed at all).
    send(&mut caas, &ToBus::ListAgents { req_id: 2 }).await;
    match next_event(&mut caas).await {
        FromBus::Reply {
            result: ReplyResult::Agents { agents },
            ..
        } => {
            assert_eq!(
                agents.len(),
                1,
                "the observer must never appear in agents(): {agents:?}"
            );
            assert_eq!(agents[0].name, "caas");
        }
        other => panic!("expected a Reply to ListAgents, got {other:?}"),
    }

    send(&mut caas, &ToBus::ListRooms { req_id: 3 }).await;
    match next_event(&mut caas).await {
        FromBus::Reply {
            result: ReplyResult::Rooms { rooms },
            ..
        } => {
            let protocol = rooms
                .iter()
                .find(|r| r.name == "protocol")
                .expect("protocol room exists");
            assert_eq!(
                protocol.members,
                vec!["caas".to_string()],
                "the observer must leave no membership trace: {:?}",
                protocol.members
            );
        }
        other => panic!("expected a Reply to ListRooms, got {other:?}"),
    }
}

#[tokio::test]
async fn an_observer_can_read_history_and_list_rooms_without_membership() {
    let (_d, port) = start_bus().await;

    let mut caas = connect(port, "caas").await;
    next_event(&mut caas).await; // Registered
    send(
        &mut caas,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut caas).await; // Joined
    send(
        &mut caas,
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
    next_event(&mut caas).await; // Reply to Send

    let mut watcher = connect_observer(port, "tail-2").await;
    next_event(&mut watcher).await; // Observing

    send(
        &mut watcher,
        &ToBus::History {
            req_id: 5,
            room: "protocol".into(),
            limit: 10,
        },
    )
    .await;
    match next_event(&mut watcher).await {
        FromBus::Reply {
            result: ReplyResult::History { messages },
            ..
        } => {
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].text, "hi");
        }
        other => panic!("expected a Reply to History, got {other:?}"),
    }

    send(&mut watcher, &ToBus::ListRooms { req_id: 6 }).await;
    match next_event(&mut watcher).await {
        FromBus::Reply {
            result: ReplyResult::Rooms { rooms },
            ..
        } => {
            assert!(rooms.iter().any(|r| r.name == "protocol"));
        }
        other => panic!("expected a Reply to ListRooms, got {other:?}"),
    }
}

#[tokio::test]
async fn an_observer_cannot_join_or_send() {
    let (_d, port) = start_bus().await;
    let mut watcher = connect_observer(port, "tail-3").await;
    next_event(&mut watcher).await; // Observing

    send(
        &mut watcher,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    match next_event(&mut watcher).await {
        FromBus::Error { .. } => {}
        other => panic!("expected an Error rejecting join, got {other:?}"),
    }

    send(
        &mut watcher,
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
    match next_event(&mut watcher).await {
        FromBus::Error { .. } => {}
        other => panic!("expected an Error rejecting send, got {other:?}"),
    }
}

#[tokio::test]
async fn a_real_agents_registration_and_membership_are_unaffected_by_a_concurrent_observer() {
    let (_d, port) = start_bus().await;

    let mut watcher = connect_observer(port, "tail-4").await;
    next_event(&mut watcher).await; // Observing
    send(
        &mut watcher,
        &ToBus::Watch {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut watcher).await; // Reply

    let mut caas = connect(port, "caas").await;
    match next_event(&mut caas).await {
        FromBus::Registered { name } => assert_eq!(name, "caas"),
        other => panic!("expected Registered, got {other:?}"),
    }
    send(
        &mut caas,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    match next_event(&mut caas).await {
        FromBus::Reply {
            result: ReplyResult::Joined { room, members },
            ..
        } => {
            assert_eq!(room, "protocol");
            assert_eq!(members, vec!["caas".to_string()]);
        }
        other => panic!("expected a Reply to Join, got {other:?}"),
    }

    send(&mut caas, &ToBus::ListAgents { req_id: 2 }).await;
    match next_event(&mut caas).await {
        FromBus::Reply {
            result: ReplyResult::Agents { agents },
            ..
        } => {
            assert_eq!(agents.len(), 1, "unexpected agents: {agents:?}");
            assert_eq!(agents[0].name, "caas");
        }
        other => panic!("expected a Reply to ListAgents, got {other:?}"),
    }
}

// --- Bug 2: cursor advance on history ---

// The real regression: a message that arrived while `beta` was offline is
// queued_for beta, beta reconnects and reads it via `history`, but nothing
// ever advanced beta's cursor — so the message stays "unread" forever on
// every future reconnect. This drives that exact sequence end to end over
// the WebSocket and confirms `history` is what closes the gap.
#[tokio::test]
async fn history_after_reconnect_advances_the_cursor_past_what_it_returned() {
    let (_d, port, store_dir) = start_bus_with_dir().await;

    let mut alpha = connect(port, "alpha").await;
    next_event(&mut alpha).await; // Registered
    let mut beta = connect(port, "beta").await;
    next_event(&mut beta).await; // Registered

    send(
        &mut alpha,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut alpha).await; // Joined
    send(
        &mut beta,
        &ToBus::Join {
            req_id: 2,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut beta).await; // Joined

    drop(beta); // beta goes offline
    assert!(
        wait_until(|| async { !agent_is_online(port, "beta").await }).await,
        "beta never went offline after its connection was dropped"
    );

    send(
        &mut alpha,
        &ToBus::Send {
            req_id: 3,
            target: Target::Room {
                room: "protocol".into(),
            },
            text: "while you were out".into(),
            done: false,
        },
    )
    .await;
    match next_event(&mut alpha).await {
        FromBus::Reply {
            result: ReplyResult::Sent { queued_for, .. },
            ..
        } => assert_eq!(queued_for, vec!["beta".to_string()]),
        other => panic!("expected a Reply to Send, got {other:?}"),
    }

    let mut beta2 = connect(port, "beta").await;
    next_event(&mut beta2).await; // Registered
    match next_event(&mut beta2).await {
        FromBus::Unread { rooms } => assert_eq!(rooms[0].count, 1),
        other => panic!("expected an Unread summary, got {other:?}"),
    }

    send(
        &mut beta2,
        &ToBus::History {
            req_id: 10,
            room: "protocol".into(),
            limit: 10,
        },
    )
    .await;
    match next_event(&mut beta2).await {
        FromBus::Reply {
            result: ReplyResult::History { messages },
            ..
        } => {
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].text, "while you were out");
        }
        other => panic!("expected a Reply to History, got {other:?}"),
    }

    // The cursor advance is asynchronous and fire-and-forget; poll for it to
    // land rather than guessing with a fixed sleep.
    let store = Store::open(&store_dir).await.unwrap();
    assert!(
        wait_until(|| async { store.unread_count("protocol", "beta").await.unwrap() == 0 }).await,
        "history must advance beta's cursor past the message it was just shown, \
         or that message will be reported unread forever"
    );
}

// Regression constraint: `reply_history` is shared with the observer
// (`tail`) path, which has no cursor of its own. The cursor advance must
// live in the `ToBus::History` arm that has `me`, not inside
// `reply_history` itself — otherwise a `tail` watcher reading a room would
// move a real agent's cursor. This test would pass even with no cursor
// advance implemented at all, which is why `history_after_reconnect_...`
// above exists to prove the advance actually happens; this one exists to
// prove it never happens for the wrong caller.
#[tokio::test]
async fn an_observers_history_call_does_not_move_any_agents_cursor() {
    let (_d, port, store_dir) = start_bus_with_dir().await;

    let mut alpha = connect(port, "alpha").await;
    next_event(&mut alpha).await;
    let mut beta = connect(port, "beta").await;
    next_event(&mut beta).await;

    send(
        &mut alpha,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut alpha).await;
    send(
        &mut beta,
        &ToBus::Join {
            req_id: 2,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut beta).await;

    send(
        &mut alpha,
        &ToBus::Send {
            req_id: 3,
            target: Target::Room {
                room: "protocol".into(),
            },
            text: "hello".into(),
            done: false,
        },
    )
    .await;
    next_event(&mut alpha).await; // Sent reply
    next_event(&mut beta).await; // routed Message, delivered live (not via history)

    let store = Store::open(&store_dir).await.unwrap();
    let alpha_before = store.cursor("protocol", "alpha").await.unwrap();
    let beta_before = store.cursor("protocol", "beta").await.unwrap();

    let mut watcher = connect_observer(port, "tail-watcher").await;
    next_event(&mut watcher).await; // Observing

    send(
        &mut watcher,
        &ToBus::History {
            req_id: 99,
            room: "protocol".into(),
            limit: 10,
        },
    )
    .await;
    match next_event(&mut watcher).await {
        FromBus::Reply {
            result: ReplyResult::History { messages },
            ..
        } => assert_eq!(messages.len(), 1),
        other => panic!("expected a Reply to History, got {other:?}"),
    }

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    assert_eq!(
        store.cursor("protocol", "alpha").await.unwrap(),
        alpha_before,
        "an observer's history call must not move alpha's cursor"
    );
    assert_eq!(
        store.cursor("protocol", "beta").await.unwrap(),
        beta_before,
        "an observer's history call must not move beta's cursor"
    );
}

#[tokio::test]
async fn a_human_registration_is_recorded_as_human() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await; // Registered

    let store = Store::open(&store_dir).await.unwrap();
    let agents = store.agents().await.unwrap();
    let me = agents.iter().find(|a| a.name == "bbaldino").unwrap();
    assert!(me.is_human, "the flag must reach the store");

    let regs = store.events_of_kind("agent_registered", 10).await.unwrap();
    let mine = regs
        .iter()
        .find(|e| e.agent.as_deref() == Some("bbaldino"))
        .expect("a registration event");
    assert_eq!(
        mine.detail["is_human"], true,
        "the event log must distinguish a person joining from a bot: {:?}",
        mine.detail
    );
}

#[tokio::test]
async fn an_ordinary_agent_is_not_recorded_as_human() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;

    let store = Store::open(&store_dir).await.unwrap();
    let agents = store.agents().await.unwrap();
    assert!(!agents.iter().find(|a| a.name == "caas").unwrap().is_human);
}

#[tokio::test]
async fn a_humans_room_membership_ends_when_they_disconnect() {
    let (_d, port, store_dir) = start_bus_with_dir().await;

    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    send(
        &mut a,
        &ToBus::Join {
            req_id: 1,
            room: "demo".into(),
        },
    )
    .await;
    next_event(&mut a).await;

    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await;
    send(
        &mut h,
        &ToBus::Join {
            req_id: 2,
            room: "demo".into(),
        },
    )
    .await;
    next_event(&mut h).await;

    let store = Store::open(&store_dir).await.unwrap();
    assert!(
        store
            .room_members("demo")
            .await
            .unwrap()
            .contains(&"bbaldino".to_string()),
        "precondition: the human is a member while connected"
    );

    drop(h); // the human closes their terminal
    assert!(
        wait_until(|| async {
            let s = Store::open(&store_dir).await.unwrap();
            !s.room_members("demo")
                .await
                .unwrap()
                .contains(&"bbaldino".to_string())
        })
        .await,
        "the human's membership must not outlive their connection"
    );

    // And the agent must not be told a departed human is a pending recipient.
    send(
        &mut a,
        &ToBus::Send {
            req_id: 3,
            target: Target::Room {
                room: "demo".into(),
            },
            text: "anyone there?".into(),
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
                !queued_for.contains(&"bbaldino".to_string()),
                "queued_for: {queued_for:?}"
            );
            assert!(
                !delivered_to.contains(&"bbaldino".to_string()),
                "delivered_to: {delivered_to:?}"
            );
        }
        other => panic!("expected a Sent reply, got {other:?}"),
    }
}

#[tokio::test]
async fn an_agents_room_membership_survives_disconnection() {
    // The contrast that makes the human case meaningful: agents stay members so
    // messages queue for them while they are away.
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    send(
        &mut a,
        &ToBus::Join {
            req_id: 1,
            room: "demo".into(),
        },
    )
    .await;
    next_event(&mut a).await;

    drop(a);
    assert!(
        wait_until(|| async { !agent_is_online(port, "caas").await }).await,
        "caas never went offline"
    );

    let store = Store::open(&store_dir).await.unwrap();
    assert!(
        store
            .room_members("demo")
            .await
            .unwrap()
            .contains(&"caas".to_string()),
        "an agent stays a member after disconnecting"
    );
}

#[tokio::test]
async fn a_human_send_resets_the_exchange_counter() {
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await;
    send(
        &mut h,
        &ToBus::Join {
            req_id: 1,
            room: "loop".into(),
        },
    )
    .await;
    next_event(&mut h).await;

    // Nineteen agent sends: one short of the cap.
    for i in 0..19 {
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

    // The human speaks, which is the signal the cap was built to detect.
    send(
        &mut h,
        &ToBus::Send {
            req_id: 2,
            target: Target::Room {
                room: "loop".into(),
            },
            text: "still here".into(),
            done: false,
        },
    )
    .await;
    next_event(&mut h).await;
    // `a` is a room member too, so the human's broadcast fans out to it just
    // like any other room message — drain that notification before treating
    // `a`'s own queue as carrying only replies to its own sends.
    match next_event(&mut a).await {
        FromBus::Message { from, .. } if from == "bbaldino" => {}
        other => panic!("expected the human's broadcast fanned out to caas, got {other:?}"),
    }

    // The counter is back to zero, so the agent gets a full cap's worth again.
    for i in 0..19 {
        send(
            &mut a,
            &ToBus::Send {
                req_id: 200 + i,
                target: Target::Room {
                    room: "loop".into(),
                },
                text: format!("n{i}"),
                done: false,
            },
        )
        .await;
        match next_event(&mut a).await {
            FromBus::Reply {
                result: ReplyResult::Sent { .. },
                ..
            } => {}
            other => panic!("send {i} after the human spoke should have been allowed: {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_human_can_send_into_a_paused_room_and_unpauses_it() {
    // The important one. A pause exists because no human was present; a human
    // arriving is exactly the condition that should clear it. If the human's send
    // bounced, they could not rescue the conversation.
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;

    // Drive the room past the cap.
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
        // The last of these 21 sends is itself the one that trips the cap, so
        // (like the explicit "still going" check below) it enqueues a second,
        // resolving `Error` behind its `Paused` notice. Drain that too so it
        // isn't mistaken later for the reply to an unrelated request.
        if let FromBus::Paused { .. } = next_event(&mut a).await {
            next_event(&mut a).await;
        }
    }
    // Confirm it really is paused for an agent.
    send(
        &mut a,
        &ToBus::Send {
            req_id: 300,
            target: Target::Room {
                room: "loop".into(),
            },
            text: "still going".into(),
            done: false,
        },
    )
    .await;
    // A `Paused` verdict enqueues two events for this one request: the
    // conversational `Paused` notice, then the resolving `Error` that
    // unblocks the caller's outstanding request (see `commands::handle`'s
    // `GuardVerdict::Paused` arm). Drain both, not just whichever arrives
    // first, so a leftover doesn't get mistaken later for a reply to a
    // different request.
    match next_event(&mut a).await {
        FromBus::Paused { .. } => match next_event(&mut a).await {
            FromBus::Error { .. } => {}
            other => panic!("expected the resolving Error after Paused, got {other:?}"),
        },
        FromBus::Error { .. } => {}
        other => panic!("precondition: the room should be paused for an agent, got {other:?}"),
    }

    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await;
    send(
        &mut h,
        &ToBus::Join {
            req_id: 1,
            room: "loop".into(),
        },
    )
    .await;
    next_event(&mut h).await;
    send(
        &mut h,
        &ToBus::Send {
            req_id: 2,
            target: Target::Room {
                room: "loop".into(),
            },
            text: "hold on, let me look".into(),
            done: false,
        },
    )
    .await;
    match next_event(&mut h).await {
        FromBus::Reply {
            result: ReplyResult::Sent { .. },
            ..
        } => {}
        other => panic!("a human must be able to speak into a paused room: {other:?}"),
    }
    // `a` is a room member too, so the human's broadcast fans out to it just
    // like any other room message — drain that notification before treating
    // `a`'s own queue as carrying only replies to its own sends.
    match next_event(&mut a).await {
        FromBus::Message { from, .. } if from == "bbaldino" => {}
        other => panic!("expected the human's broadcast fanned out to caas, got {other:?}"),
    }

    // And the room is open again for the agent.
    send(
        &mut a,
        &ToBus::Send {
            req_id: 301,
            target: Target::Room {
                room: "loop".into(),
            },
            text: "ok".into(),
            done: false,
        },
    )
    .await;
    match next_event(&mut a).await {
        FromBus::Reply {
            result: ReplyResult::Sent { .. },
            ..
        } => {}
        other => panic!("the room should have un-paused: {other:?}"),
    }
}

#[tokio::test]
async fn a_human_is_not_rate_limited() {
    // start_bus_with_keepalive is not what we want here; use the rate-limited variant.
    let guards =
        claude_bus::bus::delivery::Guards::new(claude_bus::bus::delivery::DEFAULT_CAP, 5_000);
    let (_d, port, _path) = start_bus_with_guards_dir(guards).await;

    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await;
    send(
        &mut h,
        &ToBus::Join {
            req_id: 1,
            room: "demo".into(),
        },
    )
    .await;
    next_event(&mut h).await;

    for i in 0..3 {
        send(
            &mut h,
            &ToBus::Send {
                req_id: 10 + i,
                target: Target::Room {
                    room: "demo".into(),
                },
                text: format!("typing fast {i}"),
                done: false,
            },
        )
        .await;
        match next_event(&mut h).await {
            FromBus::Reply {
                result: ReplyResult::Sent { .. },
                ..
            } => {}
            other => panic!("a person typing is not a runaway loop: {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_human_and_two_agents_can_all_talk_in_one_room() {
    // The whole point of the feature: three participants, and every message reaches
    // the other two.
    let (_d, port) = start_bus().await;

    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    send(
        &mut a,
        &ToBus::Join {
            req_id: 1,
            room: "design".into(),
        },
    )
    .await;
    next_event(&mut a).await;

    let mut b = connect(port, "dashboard").await;
    next_event(&mut b).await;
    send(
        &mut b,
        &ToBus::Join {
            req_id: 2,
            room: "design".into(),
        },
    )
    .await;
    next_event(&mut b).await;

    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await;
    send(
        &mut h,
        &ToBus::Join {
            req_id: 3,
            room: "design".into(),
        },
    )
    .await;
    next_event(&mut h).await;

    send(
        &mut h,
        &ToBus::Send {
            req_id: 4,
            target: Target::Room {
                room: "design".into(),
            },
            text: "what do you two think?".into(),
            done: false,
        },
    )
    .await;
    match next_event(&mut h).await {
        FromBus::Reply {
            result: ReplyResult::Sent { delivered_to, .. },
            ..
        } => {
            assert!(
                delivered_to.contains(&"caas".to_string()),
                "{delivered_to:?}"
            );
            assert!(
                delivered_to.contains(&"dashboard".to_string()),
                "{delivered_to:?}"
            );
            assert!(
                !delivered_to.contains(&"bbaldino".to_string()),
                "not to the sender"
            );
        }
        other => panic!("expected Sent, got {other:?}"),
    }

    for (who, ws) in [("caas", &mut a), ("dashboard", &mut b)] {
        match next_event(ws).await {
            FromBus::Message { from, text, .. } => {
                assert_eq!(from, "bbaldino", "{who} should see the human as the sender");
                assert_eq!(text, "what do you two think?");
            }
            other => panic!("{who} should have received the human's message: {other:?}"),
        }
    }
}

/// Reads events off `ws` until the reply to `req_id` arrives, so a test that
/// asks a question right after connecting isn't derailed by the `Unread`
/// summary (or anything else) the bus volunteers first.
async fn reply_to(ws: &mut common::Ws, req_id: u64) -> FromBus {
    for _ in 0..10 {
        let ev = next_event(ws).await;
        match &ev {
            FromBus::Reply { req_id: got, .. } if *got == req_id => return ev,
            FromBus::Error {
                req_id: Some(got), ..
            } if *got == req_id => return ev,
            _ => {}
        }
    }
    panic!("never saw a reply to req_id {req_id}");
}

#[tokio::test]
async fn a_room_whose_only_member_was_a_human_still_serves_its_history() {
    // Task 4 made a human's membership ephemeral, which creates a state that could
    // not previously exist: a room with real messages and zero members. History must
    // key off whether the room exists, not off who happens to be in it right now —
    // otherwise a human reconnecting to the room they were just typing in is told it
    // does not exist.
    let (_d, port) = start_bus().await;

    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await; // Registered
    send(
        &mut h,
        &ToBus::Join {
            req_id: 1,
            room: "solo".into(),
        },
    )
    .await;
    next_event(&mut h).await; // Joined
    send(
        &mut h,
        &ToBus::Send {
            req_id: 2,
            target: Target::Room {
                room: "solo".into(),
            },
            text: "note to self".into(),
            done: false,
        },
    )
    .await;
    next_event(&mut h).await; // Sent

    drop(h); // they close the terminal
    assert!(
        wait_until(|| async { !agent_is_online(port, "bbaldino").await }).await,
        "the human never went offline"
    );

    let mut again = connect_human(port, "bbaldino").await;
    next_event(&mut again).await; // Registered
    send(
        &mut again,
        &ToBus::History {
            req_id: 3,
            room: "solo".into(),
            limit: 20,
        },
    )
    .await;
    match reply_to(&mut again, 3).await {
        FromBus::Reply {
            result: ReplyResult::History { messages },
            ..
        } => {
            assert_eq!(messages.len(), 1, "{messages:?}");
            assert_eq!(messages[0].text, "note to self");
        }
        other => panic!("a room with messages but no members must stay readable: {other:?}"),
    }
}

#[tokio::test]
async fn history_for_a_room_that_never_existed_is_still_an_error() {
    // The counterpart to the test above: relaxing the membership check must not turn
    // a typo'd room name into a silently empty transcript.
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    send(
        &mut a,
        &ToBus::History {
            req_id: 1,
            room: "nonesuch".into(),
            limit: 20,
        },
    )
    .await;
    match reply_to(&mut a, 1).await {
        FromBus::Error { message, .. } => {
            assert!(
                message.contains("nonesuch"),
                "the error should name the room asked for: {message}"
            );
        }
        other => panic!("expected an error for an unknown room, got {other:?}"),
    }
}

#[tokio::test]
async fn a_humans_message_reaches_an_agent_marked_as_human() {
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

    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await;
    send(
        &mut h,
        &ToBus::Send {
            req_id: 2,
            target: Target::Room {
                room: "protocol".into(),
            },
            text: "please refactor this".into(),
            done: false,
        },
    )
    .await;
    next_event(&mut h).await; // Sent

    match next_event(&mut a).await {
        FromBus::Message { from, human, .. } => {
            assert_eq!(from, "bbaldino");
            assert!(human, "the worker must be able to tell a person asked");
        }
        other => panic!("expected a Message, got {other:?}"),
    }
}

#[tokio::test]
async fn an_agents_message_is_not_marked_as_human() {
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

    let mut b = connect(port, "dashboard").await;
    next_event(&mut b).await;
    send(
        &mut b,
        &ToBus::Send {
            req_id: 2,
            target: Target::Room {
                room: "protocol".into(),
            },
            text: "could you refactor this".into(),
            done: false,
        },
    )
    .await;
    next_event(&mut b).await;

    match next_event(&mut a).await {
        FromBus::Message { human, .. } => assert!(!human, "an agent is not a human"),
        other => panic!("expected a Message, got {other:?}"),
    }
}

#[tokio::test]
async fn history_reports_the_origin_of_each_message() {
    // The catch-up path: a worker that was offline learns origin from `history`.
    let (_d, port) = start_bus().await;
    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await;
    send(
        &mut h,
        &ToBus::Join {
            req_id: 1,
            room: "protocol".into(),
        },
    )
    .await;
    next_event(&mut h).await;
    send(
        &mut h,
        &ToBus::Send {
            req_id: 2,
            target: Target::Room {
                room: "protocol".into(),
            },
            text: "please refactor this".into(),
            done: false,
        },
    )
    .await;
    next_event(&mut h).await;

    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    send(
        &mut a,
        &ToBus::History {
            req_id: 3,
            room: "protocol".into(),
            limit: 10,
        },
    )
    .await;
    match reply_to(&mut a, 3).await {
        FromBus::Reply {
            result: ReplyResult::History { messages },
            ..
        } => {
            let m = messages
                .iter()
                .find(|m| m.from == "bbaldino")
                .expect("the human's message");
            assert!(m.human, "catch-up must preserve origin: {messages:?}");
        }
        other => panic!("expected History, got {other:?}"),
    }
}
