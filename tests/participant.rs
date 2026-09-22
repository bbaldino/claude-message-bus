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
