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

async fn send(ws: &mut Ws, cmd: &ToBus) {
    ws.send(Message::text(serde_json::to_string(cmd).unwrap()))
        .await
        .unwrap();
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
