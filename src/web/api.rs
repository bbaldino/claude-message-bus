//! JSON routes for the single-page app, under `/api`.
//!
//! Response types are defined here rather than serialising `store` rows
//! directly. Those rows are internal shapes that change for storage reasons,
//! and serialising them would turn every schema tweak into a silent API break.
//! Defining the wire format here also lets it be camelCase while the columns
//! stay snake_case.
//!
//! The TypeScript equivalents are generated from these structs by `ts-rs`
//! during `cargo test`, so the frontend's types cannot drift from the server's.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;

use crate::bus::App;

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub name: String,
    pub host: String,
    pub cwd: String,
    pub session_id: Option<String>,
    pub online: bool,
    pub is_human: bool,
    pub version: Option<String>,
    /// Epoch milliseconds.
    ///
    /// `#[ts(type = "number")]` rather than ts-rs's default `bigint` for i64:
    /// serde emits a JSON number, so `JSON.parse` yields a JavaScript number and
    /// a `bigint` type would be a lie the first consumer trips over. The
    /// precision caution behind that default does not apply — epoch millis are
    /// ~1.8e12, far below Number.MAX_SAFE_INTEGER.
    #[ts(type = "number")]
    pub last_seen: i64,
}

/// Every agent the bus has ever seen, with liveness from the registry rather
/// than the persisted `online` column — the column is only reconciled at
/// startup, while the registry knows who is routable right now.
///
/// A store failure is a 500, not `unwrap_or_default`. The HTML pages next door
/// degrade to an empty table because a human reading one can see the page is
/// bare and go look; this is consumed by code that branches on the response, and
/// `200 []` tells it — confidently, and wrongly — that the fleet is empty. This
/// is the first endpoint under `/api`, and every later one will be written by
/// copying it, so the pattern matters more than this single call site.
pub(crate) async fn agents(State(app): State<App>) -> Result<Json<Vec<Agent>>, StatusCode> {
    let rows = app.store.agents().await.map_err(|e| {
        eprintln!("GET /api/agents could not read agents: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let online = app.registry.is_online(&r.name).await;
        out.push(Agent {
            name: r.name,
            host: r.host,
            cwd: r.cwd,
            session_id: r.session_id,
            online,
            is_human: r.is_human,
            version: r.version,
            last_seen: r.last_seen,
        });
    }
    Ok(Json(out))
}

/// A room's derived state for the rail. Data, not sentences: the client composes
/// the subtitle, because the handoff specifies copy as final and design-owned.
///
/// `rename_all_fields` (in addition to `rename_all`, which only renames the
/// variant names) is required here: without it `waiting_on` ships on the wire
/// unrenamed, silently breaking the "camelCase on the wire" rule the rest of
/// this module holds to — `rename_all` on an enum does not reach into its
/// variants' fields.
#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum RoomFlagDto {
    NeedsYou {
        #[ts(type = "number")]
        exchanges: i64,
    },
    Blocked {
        #[ts(type = "number")]
        queued: i64,
        waiting_on: Vec<String>,
    },
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct RailRoom {
    pub name: String,
    pub members: Vec<String>,
    #[ts(type = "number | null")]
    pub last_activity: Option<i64>,
    /// Twelve five-minute slots, oldest first.
    #[ts(type = "Array<number>")]
    pub buckets: Vec<i64>,
    pub flag: Option<RoomFlagDto>,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct RailAgent {
    pub name: String,
    pub host: String,
    pub version: Option<String>,
    pub online: bool,
    pub is_human: bool,
    #[ts(type = "number")]
    pub last_seen: i64,
    #[ts(type = "Array<number>")]
    pub buckets: Vec<i64>,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct RailSummary {
    pub rooms: Vec<RailRoom>,
    pub agents: Vec<RailAgent>,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    /// The host the bus itself is running on. Half of the top bar's pill
    /// ("hardac · 0.3.3") — the reason this endpoint exists.
    pub host: String,
    pub version: String,
}

/// Twelve slots of five minutes: the last hour, matching the rail strip.
const RAIL_BUCKETS: usize = 12;
const BUCKET_MS: i64 = 300_000;

/// Everything the always-visible rail renders, in one call.
///
/// Polled rather than pushed: buckets are five minutes wide, so a push would
/// carry no information a ~25s poll does not, and the design forbids animating
/// the strip on update anyway.
pub(crate) async fn rail(State(app): State<App>) -> Result<Json<RailSummary>, StatusCode> {
    let now = crate::store::now_ms();
    let online = app.registry.online().await;

    let room_rows = app
        .store
        .rooms()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut rooms = Vec::with_capacity(room_rows.len());
    for r in room_rows {
        let buckets = app
            .store
            .message_buckets(
                crate::store::BucketScope::Room(&r.name),
                now,
                BUCKET_MS,
                RAIL_BUCKETS,
            )
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let flag = app
            .store
            .room_flag(&r.name, &online)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .map(|f| match f {
                crate::store::RoomFlag::NeedsYou { exchanges } => {
                    RoomFlagDto::NeedsYou { exchanges }
                }
                crate::store::RoomFlag::Blocked { queued, waiting_on } => {
                    RoomFlagDto::Blocked { queued, waiting_on }
                }
            });
        let last_activity = app
            .store
            .history(&r.name, 1)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .last()
            .map(|m| m.created_at);
        rooms.push(RailRoom {
            name: r.name,
            members: r.members,
            last_activity,
            buckets,
            flag,
        });
    }

    let agent_rows = app
        .store
        .agents()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut agents = Vec::with_capacity(agent_rows.len());
    for a in agent_rows {
        let buckets = app
            .store
            .message_buckets(
                crate::store::BucketScope::Agent(&a.name),
                now,
                BUCKET_MS,
                RAIL_BUCKETS,
            )
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        agents.push(RailAgent {
            online: online.contains(&a.name),
            name: a.name,
            host: a.host,
            version: a.version,
            is_human: a.is_human,
            last_seen: a.last_seen,
            buckets,
        });
    }

    Ok(Json(RailSummary { rooms, agents }))
}

pub(crate) async fn meta() -> Json<Meta> {
    use crate::config::EnvSource;
    Json(Meta {
        // The same source `chat.rs` registers with, so the pill names the host the
        // same way every other surface does.
        host: crate::config::RealEnv.hostname(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct Message {
    #[ts(type = "number")]
    pub id: i64,
    pub room: String,
    pub from: String,
    pub body: String,
    pub done: bool,
    /// True when the sender carried human authority — a person, or a configured
    /// relayer speaking for one.
    pub human: bool,
    #[ts(type = "number")]
    pub created_at: i64,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct Event {
    #[ts(type = "number")]
    pub id: i64,
    pub kind: String,
    pub agent: Option<String>,
    pub room: Option<String>,
    #[ts(type = "unknown")]
    pub detail: serde_json::Value,
    #[ts(type = "number")]
    pub created_at: i64,
}

/// The most rows any client-supplied `limit` can ask for. Both list endpoints
/// clamp to it — they are the only HTTP endpoints that take a limit at all.
const MAX_LIMIT: i64 = 1000;

#[derive(serde::Deserialize)]
pub(crate) struct TranscriptQuery {
    limit: Option<i64>,
    /// A message id to page backwards from — scrollback. Present: the `limit`
    /// messages immediately before it. Absent: the most recent `limit`.
    before: Option<i64>,
}

#[derive(serde::Deserialize)]
pub(crate) struct EventsQuery {
    room: Option<String>,
    kind: Option<String>,
    limit: Option<i64>,
}

pub(crate) async fn room_messages(
    State(app): State<App>,
    Path(name): Path<String>,
    Query(q): Query<TranscriptQuery>,
) -> Result<Json<Vec<Message>>, StatusCode> {
    // Clamped, not merely defaulted: SQLite reads a negative LIMIT as unlimited, so
    // `?limit=-1` would serialise the entire `messages` table out of an
    // unauthenticated endpoint on a bus bound to 0.0.0.0.
    let limit = q.limit.unwrap_or(100).clamp(1, MAX_LIMIT);
    let rows = match q.before {
        Some(before) => app.store.history_before(&name, before, limit).await,
        None => app.store.history(&name, limit).await,
    }
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        rows.into_iter()
            .map(|m| Message {
                id: m.id,
                room: m.room,
                from: m.from_agent,
                body: m.body,
                done: m.done,
                human: m.human,
                created_at: m.created_at,
            })
            .collect(),
    ))
}

pub(crate) async fn events(
    State(app): State<App>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<Vec<Event>>, StatusCode> {
    let rows = app
        .store
        // Clamped for the same reason as the transcript's — see `room_messages`.
        .events_filtered(
            q.room.as_deref(),
            q.kind.as_deref(),
            q.limit.unwrap_or(200).clamp(1, MAX_LIMIT),
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        rows.into_iter()
            .map(|e| Event {
                id: e.id,
                kind: e.kind,
                agent: e.agent,
                room: e.room,
                detail: e.detail,
                created_at: e.created_at,
            })
            .collect(),
    ))
}
