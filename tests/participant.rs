//! End-to-end tests for the HTTP participant surface (`/api/participants`).
//! Drives a real bus over raw HTTP/1.1, matching the no-HTTP-client style of
//! `tests/web.rs`, and a real WS agent (via `common::InProcessAgent`) when a
//! send needs a live recipient.

#![allow(dead_code)] // helpers land a task ahead of the tests that use them.

mod common;

use claude_bus::bus::participant::ParticipantConfig;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Minimal HTTP/1.1 POST with a JSON body and arbitrary extra headers. Returns
/// `(status, body)`. No HTTP-client dependency, like `tests/web.rs`.
async fn post_json_headers(
    port: u16,
    path: &str,
    body: Value,
    headers: &[(&str, &str)],
) -> (u16, String) {
    let body = body.to_string();
    let mut req = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(&body);
    send_raw(port, &req).await
}

/// Minimal HTTP/1.1 GET with arbitrary extra headers. Returns `(status, body)`.
async fn get_headers(port: u16, path: &str, headers: &[(&str, &str)]) -> (u16, String) {
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    send_raw(port, &req).await
}

async fn send_raw(port: u16, req: &str) -> (u16, String) {
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).await.unwrap();
    let status = raw
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = raw.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    (status, body.trim().to_string())
}

fn token_of(body: &str) -> String {
    serde_json::from_str::<Value>(body).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn register_without_secret_is_a_plain_participant() {
    let (_d, port, _p) =
        common::start_bus_with_participants_dir(ParticipantConfig::default()).await;
    let (status, body) =
        post_json_headers(port, "/api/participants", json!({"name":"raven"}), &[]).await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["name"], "raven");
    assert_eq!(v["relayer"], false);
    assert!(v["token"].as_str().is_some_and(|t| !t.is_empty()));
}

#[tokio::test]
async fn register_with_correct_secret_is_a_relayer() {
    let cfg = ParticipantConfig {
        relayer_secret: Some("s3cret".into()),
        ..Default::default()
    };
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    let (status, body) = post_json_headers(
        port,
        "/api/participants",
        json!({"name":"raven"}),
        &[("X-Relayer-Secret", "s3cret")],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["relayer"],
        true
    );
}

#[tokio::test]
async fn a_wrong_secret_is_refused_not_downgraded() {
    let cfg = ParticipantConfig {
        relayer_secret: Some("s3cret".into()),
        ..Default::default()
    };
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    let (status, _body) = post_json_headers(
        port,
        "/api/participants",
        json!({"name":"raven"}),
        &[("X-Relayer-Secret", "wrong")],
    )
    .await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn a_cross_origin_register_is_refused() {
    let (_d, port, _p) =
        common::start_bus_with_participants_dir(ParticipantConfig::default()).await;
    let (status, _b) = post_json_headers(
        port,
        "/api/participants",
        json!({"name":"raven"}),
        &[("Origin", "http://evil.example")],
    )
    .await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn a_participant_send_reaches_a_connected_agent_and_reports_delivered() {
    let (_d, port, _p) =
        common::start_bus_with_participants_dir(ParticipantConfig::default()).await;
    let bus = format!("ws://127.0.0.1:{port}/ws");
    let mut caas = common::InProcessAgent::start(&bus, "caas");
    common::initialize(&mut caas).await;
    assert!(
        common::wait_until(|| common::agent_is_online(port, "caas")).await,
        "caas should be online before the send"
    );

    let (_s, body) =
        post_json_headers(port, "/api/participants", json!({"name":"raven"}), &[]).await;
    let token = token_of(&body);

    let (status, sbody) = post_json_headers(
        port,
        "/api/participants/send",
        json!({"target":{"kind":"agent","name":"caas"}, "text":"hi", "done":false}),
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(status, 200, "{sbody}");
    let v: Value = serde_json::from_str(&sbody).unwrap();
    assert_eq!(v["outcome"], "sent");
    assert_eq!(v["deliveredTo"], json!(["caas"]));
}

#[tokio::test]
async fn a_participant_send_to_an_unknown_agent_is_refused() {
    let (_d, port, _p) =
        common::start_bus_with_participants_dir(ParticipantConfig::default()).await;
    let (_s, body) =
        post_json_headers(port, "/api/participants", json!({"name":"raven"}), &[]).await;
    let token = token_of(&body);
    let (status, _b) = post_json_headers(
        port,
        "/api/participants/send",
        json!({"target":{"kind":"agent","name":"ghost"}, "text":"hi"}),
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(status, 422);
}

#[tokio::test]
async fn a_send_without_a_token_is_unauthorized() {
    let (_d, port, _p) =
        common::start_bus_with_participants_dir(ParticipantConfig::default()).await;
    let (status, _b) = post_json_headers(
        port,
        "/api/participants/send",
        json!({"target":{"kind":"room","room":"x"}, "text":"hi"}),
        &[],
    )
    .await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn receive_returns_a_dm_and_advances_the_cursor() {
    use claude_bus::proto::{Target, ToBus};
    let (_d, port, _p) =
        common::start_bus_with_participants_dir(ParticipantConfig::default()).await;

    // raven registers over HTTP.
    let (_s, rb) = post_json_headers(port, "/api/participants", json!({"name":"raven"}), &[]).await;
    let token = token_of(&rb);

    // caas connects over WS and DMs raven; wait for its Sent reply so the
    // message is durably stored before raven polls.
    let mut caas = common::connect(port, "caas").await;
    common::send(
        &mut caas,
        &ToBus::Send {
            req_id: 1,
            target: Target::Agent {
                name: "raven".into(),
            },
            text: "ping".into(),
            done: false,
            files: vec![],
        },
    )
    .await;
    common::next_event(&mut caas).await; // Reply::Sent

    let (status, body) = get_headers(
        port,
        "/api/participants/receive?after=0&timeout=2",
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    let msgs = v["messages"].as_array().unwrap();
    assert!(
        msgs.iter()
            .any(|m| m["from"] == "caas" && m["body"] == "ping"),
        "raven should receive caas's DM: {body}"
    );
    let cursor = v["cursor"].as_i64().unwrap();
    assert!(cursor > 0, "cursor advances to the delivered id: {body}");

    // A second poll acking `cursor` returns nothing new before timeout.
    let (_s2, body2) = get_headers(
        port,
        &format!("/api/participants/receive?after={cursor}&timeout=1"),
        &[("X-Participant-Token", &token)],
    )
    .await;
    let v2: Value = serde_json::from_str(&body2).unwrap();
    assert!(
        v2["messages"].as_array().unwrap().is_empty(),
        "acked messages are not redelivered: {body2}"
    );
}

#[tokio::test]
async fn a_plain_participant_cannot_resume() {
    let (_d, port, _p) =
        common::start_bus_with_participants_dir(ParticipantConfig::default()).await;
    let (_s, body) =
        post_json_headers(port, "/api/participants", json!({"name":"raven"}), &[]).await;
    let token = token_of(&body);
    let (status, _b) = post_json_headers(
        port,
        "/api/participants/resume",
        json!({"room":"x"}),
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn a_relayer_can_resume() {
    let cfg = ParticipantConfig {
        relayer_secret: Some("s".into()),
        ..Default::default()
    };
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    let (_s, body) = post_json_headers(
        port,
        "/api/participants",
        json!({"name":"raven"}),
        &[("X-Relayer-Secret", "s")],
    )
    .await;
    let token = token_of(&body);
    let (status, _b) = post_json_headers(
        port,
        "/api/participants/resume",
        json!({"room":"x"}),
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(status, 204);
}

#[tokio::test]
async fn a_lease_that_stops_polling_goes_offline() {
    let cfg = ParticipantConfig {
        lease_ttl: std::time::Duration::from_millis(150),
        ..Default::default()
    };
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    post_json_headers(port, "/api/participants", json!({"name":"raven"}), &[]).await;
    assert!(
        common::agent_is_online(port, "raven").await,
        "raven is online right after registering"
    );
    assert!(
        common::wait_until(|| async { !common::agent_is_online(port, "raven").await }).await,
        "raven should go offline once its lease expires with no polling"
    );
}

#[tokio::test]
async fn an_expired_lease_leaves_an_agent_that_can_be_deleted() {
    // The sweeper detached an expired lease from the registry but never cleared
    // the persisted `online` column, so the delete — whose store guard deletes only
    // `online = 0` rows — refused with 409 forever, though every view showed the
    // participant offline. (Found live with a throwaway "raven-dev".)
    let cfg = ParticipantConfig {
        lease_ttl: std::time::Duration::from_millis(150),
        ..Default::default()
    };
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    post_json_headers(port, "/api/participants", json!({"name":"raven-dev"}), &[]).await;
    assert!(
        common::wait_until(|| async { !common::agent_is_online(port, "raven-dev").await }).await,
        "the lease should expire"
    );
    assert_eq!(
        common::delete_same_origin(port, "/api/agents/raven-dev").await,
        204,
        "an expired HTTP participant must be deletable"
    );
}

#[tokio::test]
async fn a_reserved_name_takeover_is_not_undone_when_the_old_lease_expires() {
    // An authorized registration of a reserved name takes it over from the prior
    // holder. The prior lease used to stay in the lease table, so when it lapsed
    // the sweeper — which detaches by name — detached the *new*, actively polling
    // holder: it went offline (and, with the online-flag fix, deletable) mid-session.
    let cfg = ParticipantConfig {
        relayer_secret: Some("s".into()),
        reserved_names: ["raven".to_string()].into_iter().collect(),
        lease_ttl: std::time::Duration::from_millis(1500),
    };
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    let auth = [("X-Relayer-Secret", "s")];
    let reg = || async {
        let (status, body) =
            post_json_headers(port, "/api/participants", json!({"name":"raven"}), &auth).await;
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["name"], "raven", "the reserved name is kept: {body}");
        v["token"].as_str().unwrap().to_string()
    };
    let _old = reg().await; // the old client, which then goes silent
    let new = reg().await; // a reconnect takes the name over

    // The new holder keeps polling well past the old lease's expiry.
    let poller = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(3500);
        while tokio::time::Instant::now() < deadline {
            get_headers(
                port,
                "/api/participants/receive?timeout=1",
                &[("X-Participant-Token", &new)],
            )
            .await;
        }
    });
    poller.await.unwrap();

    assert!(
        common::agent_is_online(port, "raven").await,
        "the live holder must still be online after the old lease expired"
    );
    assert_eq!(
        common::delete_same_origin(port, "/api/agents/raven").await,
        409,
        "a live holder must not be deletable"
    );
}

#[tokio::test]
async fn human_mode_with_the_secret_is_a_full_human_proxy() {
    let cfg = ParticipantConfig {
        relayer_secret: Some("s".into()),
        ..Default::default()
    };
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    let (status, body) = post_json_headers(
        port,
        "/api/participants",
        json!({"name":"raven","mode":"human"}),
        &[("X-Relayer-Secret", "s")],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["human"], true, "{body}");
    assert_eq!(v["relayer"], false, "{body}");
}

#[tokio::test]
async fn human_mode_without_the_secret_is_not_honored() {
    let cfg = ParticipantConfig {
        relayer_secret: Some("s".into()),
        ..Default::default()
    };
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    let (status, body) = post_json_headers(
        port,
        "/api/participants",
        json!({"name":"raven","mode":"human"}),
        &[],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["human"], false, "no secret => plain, not human: {body}");
    assert_eq!(v["relayer"], false, "{body}");
}

#[tokio::test]
async fn an_unknown_mode_is_rejected() {
    let cfg = ParticipantConfig {
        relayer_secret: Some("s".into()),
        ..Default::default()
    };
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    let (status, _b) = post_json_headers(
        port,
        "/api/participants",
        json!({"name":"raven","mode":"bogus"}),
        &[("X-Relayer-Secret", "s")],
    )
    .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn a_human_proxy_is_cap_exempt_while_a_relayer_is_not() {
    let cfg = ParticipantConfig {
        relayer_secret: Some("s".into()),
        ..Default::default()
    };
    let guards = claude_bus::bus::delivery::Guards::new(2, 0);
    let (_d, port, _p) = common::start_bus_with_participants_and_guards_dir(guards, cfg).await;

    // Human proxy: four sends to its own room, none pause (cap=2, but exempt).
    let (_s, hb) = post_json_headers(
        port,
        "/api/participants",
        json!({"name":"raven","mode":"human"}),
        &[("X-Relayer-Secret", "s")],
    )
    .await;
    let htok = token_of(&hb);
    for i in 0..4 {
        let (st, b) = post_json_headers(
            port,
            "/api/participants/send",
            json!({"target":{"kind":"room","room":"hroom"}, "text":format!("h{i}")}),
            &[("X-Participant-Token", &htok)],
        )
        .await;
        assert_eq!(st, 200, "{b}");
        assert_eq!(
            serde_json::from_str::<Value>(&b).unwrap()["outcome"],
            "sent",
            "a human proxy is cap-exempt: {b}"
        );
    }

    // Relayer: same bus, its own room, pauses once past the cap.
    let (_s2, rb) = post_json_headers(
        port,
        "/api/participants",
        json!({"name":"hubby","mode":"relayer"}),
        &[("X-Relayer-Secret", "s")],
    )
    .await;
    let rtok = token_of(&rb);
    let mut outcomes = vec![];
    for i in 0..3 {
        let (_st, b) = post_json_headers(
            port,
            "/api/participants/send",
            json!({"target":{"kind":"room","room":"rroom"}, "text":format!("r{i}")}),
            &[("X-Participant-Token", &rtok)],
        )
        .await;
        outcomes.push(
            serde_json::from_str::<Value>(&b).unwrap()["outcome"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    assert_eq!(
        outcomes,
        vec!["sent", "sent", "paused"],
        "a relayer's sends count against the exchange cap"
    );
}

#[tokio::test]
async fn an_http_participant_does_not_inherit_a_previous_holders_status() {
    let (_d, port, _p) =
        common::start_bus_with_participants_dir(ParticipantConfig::default()).await;
    let mut ws = common::connect(port, "raven").await;
    common::next_event(&mut ws).await; // Registered
    common::send(
        &mut ws,
        &claude_bus::proto::ToBus::Status {
            state: claude_bus::proto::AgentState::Idle,
            state_age_ms: Some(0),
            heartbeat_age_ms: Some(0),
            reason: None,
            text: Some("the websocket session's words".into()),
            text_age_ms: Some(0),
            waiting_on: None,
        },
    )
    .await;
    assert!(
        common::wait_until(|| async {
            common::get_json(port, "/api/agents/raven").await["status"]["state"] == "idle"
        })
        .await
    );
    drop(ws);
    assert!(common::wait_until(|| async { !common::agent_is_online(port, "raven").await }).await);
    let (status, body) =
        post_json_headers(port, "/api/participants", json!({"name":"raven"}), &[]).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["name"],
        "raven"
    );
    let detail = common::get_json(port, "/api/agents/raven").await;
    assert!(detail["status"].is_null(), "{detail}");
}

/// A tiny but genuine PNG: the 8-byte signature plus padding. Enough for the store
/// (any bytes) and for get_file's magic-byte sniffing.
fn png_bytes(len: usize) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.resize(len, 7);
    v
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

async fn register_token(port: u16, name: &str) -> String {
    let (status, body) =
        post_json_headers(port, "/api/participants", json!({ "name": name }), &[]).await;
    assert_eq!(status, 200, "{body}");
    serde_json::from_str::<Value>(&body).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_participant_can_store_a_file_in_a_room() {
    let (_d, port, _p) = common::start_bus_with_participants_dir(Default::default()).await;
    let token = register_token(port, "raven").await;
    let png = png_bytes(1_000);
    let (status, body) = post_json_headers(
        port,
        "/api/participants/files",
        json!({ "room": "shots", "key": "a.png", "content_b64": b64(&png), "content_type": "image/png" }),
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["key"], "a.png");
    assert_eq!(v["size"], 1_000);
    assert_eq!(v["sha256"].as_str().unwrap().len(), 64, "{body}");

    let files = common::get_json(port, "/api/rooms/shots/files").await;
    let f = files
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["key"] == "a.png")
        .unwrap_or_else(|| panic!("not listed: {files}"));
    assert_eq!(f["size"], 1_000);
    assert_eq!(f["updatedBy"], "raven");
    assert_eq!(f["contentType"], "image/png");
}

#[tokio::test]
async fn a_screenshot_larger_than_the_default_body_limit_is_accepted() {
    // A 3 MB PNG is ~4 MB of base64: well past axum's 2 MB default request limit.
    let (_d, port, _p) = common::start_bus_with_participants_dir(Default::default()).await;
    let token = register_token(port, "raven").await;
    let (status, body) = post_json_headers(
        port,
        "/api/participants/files",
        json!({ "room": "shots", "key": "big.png", "content_b64": b64(&png_bytes(3 * 1024 * 1024)), "content_type": "image/png" }),
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn storing_a_file_needs_a_live_lease_same_origin_and_valid_base64() {
    let (_d, port, _p) = common::start_bus_with_participants_dir(Default::default()).await;
    let token = register_token(port, "raven").await;
    let body = json!({ "room": "shots", "key": "a.png", "content_b64": b64(&png_bytes(10)) });
    let (no_token, _) = post_json_headers(port, "/api/participants/files", body.clone(), &[]).await;
    assert_eq!(no_token, 401);
    let (cross, _) = post_json_headers(
        port,
        "/api/participants/files",
        body,
        &[
            ("X-Participant-Token", &token),
            ("Origin", "http://evil.example"),
        ],
    )
    .await;
    assert_eq!(cross, 403);
    let (bad, msg) = post_json_headers(
        port,
        "/api/participants/files",
        json!({ "room": "shots", "key": "a.png", "content_b64": "not base64!!" }),
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(bad, 400, "{msg}");
    assert!(msg.contains("base64"), "{msg}");
}

/// Like `get_headers`, but keeps the body as bytes and returns the headers,
/// for endpoints that answer with a file rather than text.
async fn get_bytes(port: u16, path: &str, headers: &[(&str, &str)]) -> (u16, String, Vec<u8>) {
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a header terminator");
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (status, head, raw[split + 4..].to_vec())
}

/// caas (WS) DMs raven (HTTP) a message carrying `shot.png`. Returns raven's
/// token, the attachment as raven received it, and the bytes caas stored.
async fn dm_raven_an_attachment(port: u16) -> (String, Value, Vec<u8>) {
    use claude_bus::proto::{Target, ToBus};
    let token = register_token(port, "raven").await;
    let png = png_bytes(500);
    let mut caas = common::connect(port, "caas").await;
    common::next_event(&mut caas).await; // Registered
    common::send(
        &mut caas,
        &ToBus::PutFile {
            req_id: 1,
            room: "dm:caas|raven".into(),
            key: "shot.png".into(),
            content_b64: b64(&png),
            content_type: Some("image/png".into()),
        },
    )
    .await;
    common::next_event(&mut caas).await; // FileStored
    common::send(
        &mut caas,
        &ToBus::Send {
            req_id: 2,
            target: Target::Agent {
                name: "raven".into(),
            },
            text: "the mockup".into(),
            done: false,
            files: vec!["shot.png".into()],
        },
    )
    .await;
    common::next_event(&mut caas).await; // Sent

    let (status, body) = get_headers(
        port,
        "/api/participants/receive?after=0&timeout=2",
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    let msg = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["body"] == "the mockup")
        .unwrap_or_else(|| panic!("raven did not receive the DM: {body}"))
        .clone();
    let files = msg["files"].as_array().expect("a files array").clone();
    assert_eq!(files.len(), 1, "{msg}");
    (token, files[0].clone(), png)
}

#[tokio::test]
async fn a_received_message_lists_its_attachments() {
    let (_d, port, _p) = common::start_bus_with_participants_dir(Default::default()).await;
    let (_token, file, _png) = dm_raven_an_attachment(port).await;
    assert_eq!(file["key"], "shot.png");
    assert_eq!(file["contentType"], "image/png");
    assert_eq!(file["size"], 500);
    assert_eq!(file["sha256"].as_str().unwrap().len(), 64, "{file}");
}

#[tokio::test]
async fn a_member_fetches_an_attachments_bytes() {
    let (_d, port, _p) = common::start_bus_with_participants_dir(Default::default()).await;
    let (token, file, png) = dm_raven_an_attachment(port).await;
    let sha = file["sha256"].as_str().unwrap();
    let (status, head, body) = get_bytes(
        port,
        &format!("/api/participants/files?room=dm:caas%7Craven&key=shot.png&sha256={sha}"),
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(status, 200, "{head}");
    assert_eq!(body, png, "the exact stored bytes");
    assert!(
        head.to_ascii_lowercase()
            .contains("content-type: image/png"),
        "{head}"
    );
    // Without sha256 it serves the current version.
    let (status, _, body) = get_bytes(
        port,
        "/api/participants/files?room=dm:caas%7Craven&key=shot.png",
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body, png);
}

#[tokio::test]
async fn fetching_a_file_is_refused_outside_the_rules() {
    let (_d, port, _p) = common::start_bus_with_participants_dir(Default::default()).await;
    let (token, _file, _png) = dm_raven_an_attachment(port).await;
    let path = "/api/participants/files?room=dm:caas%7Craven&key=shot.png";

    let (no_token, _, _) = get_bytes(port, path, &[]).await;
    assert_eq!(no_token, 401);
    let (cross, _, _) = get_bytes(
        port,
        path,
        &[
            ("X-Participant-Token", &token),
            ("Origin", "http://evil.example"),
        ],
    )
    .await;
    assert_eq!(cross, 403);

    let other = register_token(port, "outsider").await;
    let (not_member, _, _) = get_bytes(port, path, &[("X-Participant-Token", &other)]).await;
    assert_eq!(not_member, 403, "only rooms the lease is a member of");

    let (unknown, _, _) = get_bytes(
        port,
        "/api/participants/files?room=dm:caas%7Craven&key=nope.png",
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(unknown, 404);

    let (stale, _, _) = get_bytes(
        port,
        &format!("{path}&sha256={}", "0".repeat(64)),
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(stale, 409, "the key no longer holds the attached version");
}

#[tokio::test]
async fn a_participant_send_attaches_a_file_it_stored() {
    use claude_bus::proto::FromBus;
    let (_d, port, _p) = common::start_bus_with_participants_dir(Default::default()).await;
    let token = register_token(port, "raven").await;
    let mut caas = common::connect(port, "caas").await;
    common::next_event(&mut caas).await; // Registered

    let (status, body) = post_json_headers(
        port,
        "/api/participants/files",
        json!({ "room": "dm:caas|raven", "key": "photo.png", "content_b64": b64(&png_bytes(20)), "content_type": "image/png" }),
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(status, 200, "{body}");

    let (bad, msg) = post_json_headers(
        port,
        "/api/participants/send",
        json!({ "target": {"kind":"agent","name":"caas"}, "text": "see", "files": ["missing.png"] }),
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(bad, 422, "{msg}");
    assert!(msg.contains("missing.png"), "{msg}");

    let (status, body) = post_json_headers(
        port,
        "/api/participants/send",
        json!({ "target": {"kind":"agent","name":"caas"}, "text": "see", "files": ["photo.png"] }),
        &[("X-Participant-Token", &token)],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    match common::next_message(&mut caas).await {
        FromBus::Message { text, files, .. } => {
            assert_eq!(text, "see", "the refused send must not have been delivered");
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].key, "photo.png");
        }
        other => panic!("expected Message, got {other:?}"),
    }
}
