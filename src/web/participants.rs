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

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::bus::App;
use crate::bus::commands::{Authority, SendOutcome, do_send};
use crate::bus::participant::{LeaseHandle, RelayerDecision};
use crate::proto::Target;

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
