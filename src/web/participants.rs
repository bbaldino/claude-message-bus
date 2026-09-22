//! HTTP participant endpoints, under `/api/participants`.
//!
//! A participant that can only make outbound HTTP requests (a WASM/`wasi:http`
//! plugin) joins here instead of over `/ws`: register to open a lease, long-poll
//! to receive, POST to send/resume. The heavy lifting is reused — sends go
//! through `bus::commands`, presence and delivered-vs-queued through `Registry`,
//! the guards through `Guards` — so this module is transport, not a second bus.
//!
//! Every endpoint carries the same-origin guard the WS handshake and the
//! hide/delete writes do: the bus binds `0.0.0.0` with no auth, so a
//! state-changing request a browser can forge must at least come from a page
//! this bus served. Non-browser callers (raven's bridge, curl) send no `Origin`
//! and are allowed, exactly as elsewhere.

use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::bus::App;
use crate::bus::commands::{Authority, SendOutcome, do_send};
use crate::bus::participant::{LeaseHandle, RelayerDecision};
use crate::proto::Target;
use crate::store::MessageRow;

/// The lease behind an `X-Participant-Token`, renewing it, or `None` when the
/// header is missing or the token is unknown/expired.
pub(crate) async fn token_lease(app: &App, headers: &HeaderMap) -> Option<LeaseHandle> {
    let token = headers
        .get("x-participant-token")
        .and_then(|v| v.to_str().ok())?;
    app.participants.touch(token).await
}

/// No `Origin` (a non-browser client) is allowed; a browser `Origin` must name
/// this bus. Mirrors `bus::origin_permitted` and `web::api`'s delete guard.
pub(crate) fn origin_ok(headers: &HeaderMap) -> bool {
    let get = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
    match get("origin") {
        None => true,
        Some(origin) => crate::web::origin_matches_host(origin, get("host").unwrap_or_default()),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct RegisterRequest {
    name: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RegisterResponse {
    name: String,
    token: String,
    relayer: bool,
    lease_ttl_ms: u64,
}

/// `POST /api/participants` — open a lease.
///
/// A correct `X-Relayer-Secret` makes the whole lease a relayer (session-level
/// authority); a wrong one is refused rather than downgraded. A reserved name
/// may be claimed only by a relayer, and an existing holder of it is detached
/// first so the returned name is the bare reserved one, not a `#2` suffix.
pub(crate) async fn register(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<RegisterRequest>,
) -> Response {
    if !origin_ok(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let secret = headers
        .get("x-relayer-secret")
        .and_then(|v| v.to_str().ok());
    let relayer = match app.participants.decide_relayer(secret) {
        RelayerDecision::Plain => false,
        RelayerDecision::Relayer => true,
        RelayerDecision::Refused => return StatusCode::FORBIDDEN.into_response(),
    };
    if app.participants.is_reserved(&body.name) && !relayer {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Reserved + authorised: take over any existing holder so the bare reserved
    // name is kept rather than suffixed. `detach` here drops the prior lease's
    // routing entry; its lease row is swept later (or is this same identity
    // reconnecting).
    if app.participants.is_reserved(&body.name) && app.registry.is_online(&body.name).await {
        app.registry.detach(&body.name).await;
    }

    let already = app.registry.is_online(&body.name).await;
    let (tx, rx) = tokio::sync::mpsc::channel(crate::bus::registry::CHANNEL_CAPACITY);
    let name = app.registry.attach(&body.name, "http", tx).await;
    let _ = app
        .store
        .upsert_agent(&name, "http", "", None, false, None)
        .await;

    // Presence + the registration event fire only on a genuine online
    // transition, so a lease renewal (a fresh register for a name already live)
    // does not flap the console dot or spam the audit log.
    if !already {
        let _ = app
            .store
            .append_event(
                "agent_registered",
                Some(&name),
                None,
                serde_json::json!({
                    "requested_name": body.name,
                    "effective_name": &name,
                    "host": "http",
                    "transport": "http",
                    "is_human": false,
                }),
            )
            .await;
        app.registry
            .notify_presence(crate::proto::FromBus::Presence {
                name: name.clone(),
                host: "http".into(),
                online: true,
                last_seen: crate::store::now_ms(),
            })
            .await;
    }

    let token = app.participants.open(name.clone(), relayer, rx).await;
    Json(RegisterResponse {
        name,
        token,
        relayer,
        lease_ttl_ms: app.participants.cfg().lease_ttl.as_millis() as u64,
    })
    .into_response()
}

#[derive(serde::Deserialize)]
pub(crate) struct SendRequest {
    target: Target,
    text: String,
    #[serde(default)]
    done: bool,
}

/// The wire form of `commands::SendOutcome`'s deliverable cases. camelCase
/// fields, discriminated by `outcome`. `UnknownAgent`/`Error` are not here —
/// they become a 422 with the message, not a normal outcome.
#[derive(serde::Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum SendOutcomeDto {
    Sent {
        room: String,
        msg_id: i64,
        delivered_to: Vec<String>,
        queued_for: Vec<String>,
    },
    RateLimited {
        retry_in_ms: i64,
    },
    Paused {
        room: String,
        count: u32,
        reason: String,
    },
}

/// `POST /api/participants/send` — send as this lease's identity.
///
/// Authority is the lease's (session-level): `human_present` is always false for
/// an HTTP participant, `relayer` is the lease's bit. So a relayer lease's
/// message is labeled with human authority but still runs the guards — hence it
/// can come back `paused`/`rate_limited` like anyone.
pub(crate) async fn send(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<SendRequest>,
) -> Response {
    if !origin_ok(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(lease) = token_lease(&app, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let authority = Authority {
        human_present: false,
        relayer: lease.relayer,
    };
    match do_send(
        &app,
        &lease.name,
        body.target,
        body.text,
        body.done,
        authority,
    )
    .await
    {
        SendOutcome::Sent {
            room,
            msg_id,
            delivered_to,
            queued_for,
        } => Json(SendOutcomeDto::Sent {
            room,
            msg_id,
            delivered_to,
            queued_for,
        })
        .into_response(),
        SendOutcome::RateLimited { retry_in_ms } => {
            Json(SendOutcomeDto::RateLimited { retry_in_ms }).into_response()
        }
        SendOutcome::Paused {
            room,
            count,
            reason,
        } => Json(SendOutcomeDto::Paused {
            room,
            count,
            reason,
        })
        .into_response(),
        SendOutcome::UnknownAgent { message } | SendOutcome::Error { message } => {
            (StatusCode::UNPROCESSABLE_ENTITY, message).into_response()
        }
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct ReceiveQuery {
    after: Option<i64>,
    limit: Option<i64>,
    timeout: Option<u64>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MessageDto {
    id: i64,
    room: String,
    from: String,
    body: String,
    done: bool,
    human: bool,
    created_at: i64,
}

impl From<MessageRow> for MessageDto {
    fn from(m: MessageRow) -> Self {
        Self {
            id: m.id,
            room: m.room,
            from: m.from_agent,
            body: m.body,
            done: m.done,
            human: m.human,
            created_at: m.created_at,
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReceiveResponse {
    messages: Vec<MessageDto>,
    cursor: i64,
}

/// `GET /api/participants/receive` — long-poll for messages across every room
/// this lease is a member of.
///
/// `after` is both the wire cursor and the ack: before reading, every member
/// room's durable cursor is advanced to `after` (monotonic `MAX`), so a single
/// global offset fans onto the per-`(room, name)` cursors. The store is the
/// source of truth; the lease's mpsc is only the wakeup, drained after it fires
/// so a burst that overflowed it still gets caught up by the re-read.
pub(crate) async fn receive(
    State(app): State<App>,
    headers: HeaderMap,
    Query(q): Query<ReceiveQuery>,
) -> Response {
    if !origin_ok(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(lease) = token_lease(&app, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let after = q.after.unwrap_or(0);
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let timeout = Duration::from_secs(q.timeout.unwrap_or(30).clamp(1, 50));

    // The ack. Everything <= after was returned by an earlier poll (a contiguous
    // id-ordered prefix of the union), so advancing every member room to `after`
    // can't skip an unreturned message.
    for room in app
        .store
        .member_rooms(&lease.name)
        .await
        .unwrap_or_default()
    {
        let _ = app.store.set_cursor(&room, &lease.name, after).await;
    }

    let mut msgs = app
        .store
        .undelivered_for_participant(&lease.name, limit)
        .await
        .unwrap_or_default();

    if msgs.is_empty() {
        let mut rx = lease.rx.lock().await;
        if tokio::time::timeout(timeout, rx.recv()).await.is_ok() {
            // Drain the wakeup buffer; the store, not these frames, is what we
            // actually return, so their content doesn't matter — only that
            // something arrived.
            while rx.try_recv().is_ok() {}
            drop(rx);
            msgs = app
                .store
                .undelivered_for_participant(&lease.name, limit)
                .await
                .unwrap_or_default();
        }
    }

    let cursor = msgs.last().map(|m| m.id).unwrap_or(after);
    Json(ReceiveResponse {
        messages: msgs.into_iter().map(MessageDto::from).collect(),
        cursor,
    })
    .into_response()
}

#[derive(serde::Deserialize)]
pub(crate) struct ResumeRequest {
    room: String,
}

/// `POST /api/participants/resume` — clear a room's exchange-cap pause.
///
/// Relayer leases only: a plain (bot) lease gets 403. `ToBus::Resume` is ungated
/// over WS only because every WS agent has a human behind it to authorise
/// "continue"; an HTTP bot has none, so letting it resume would make the exchange
/// cap toothless. Resets only the exchange counter, not the per-agent rate limit.
pub(crate) async fn resume(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<ResumeRequest>,
) -> Response {
    if !origin_ok(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(lease) = token_lease(&app, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !lease.relayer {
        return StatusCode::FORBIDDEN.into_response();
    }
    app.guards.reset(&body.room).await;
    let _ = app
        .store
        .append_event(
            "resumed",
            Some(&lease.name),
            Some(&body.room),
            serde_json::json!({ "via": "participant_resume" }),
        )
        .await;
    StatusCode::NO_CONTENT.into_response()
}
