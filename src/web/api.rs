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
    pub hidden: bool,
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
            hidden: r.hidden,
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

/// The most rows any client-supplied `limit` can ask for. The transcript and
/// events endpoints clamp to it — they are the only two HTTP endpoints that
/// take a limit at all. `/api/rooms/{name}/files` (`room_files`, below) takes
/// no limit and does not clamp: `Store::list_files` has no LIMIT clause, but
/// unlike a transcript or event log a room's file list is bounded by how many
/// distinct keys have ever been uploaded to it, each row is small (metadata
/// only — key, size, content type, uploader, timestamp; no bytes), and the
/// bytes behind each key are already capped at `store::files::MAX_BLOB_BYTES`
/// independently. Not a new exposure, just not "both".
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

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct AgentRoomSummary {
    pub name: String,
    #[ts(type = "number")]
    pub message_count: i64,
    /// Epoch milliseconds; 0 when the agent has sent nothing in this room.
    #[ts(type = "number")]
    pub last_activity: i64,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct AgentEventItem {
    #[ts(type = "number")]
    pub id: i64,
    pub kind: String,
    #[ts(type = "unknown")]
    pub detail: serde_json::Value,
    #[ts(type = "number")]
    pub created_at: i64,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct AgentDetail {
    pub name: String,
    pub host: String,
    pub cwd: String,
    pub session_id: Option<String>,
    pub version: Option<String>,
    pub online: bool,
    pub is_human: bool,
    #[ts(type = "number")]
    pub last_seen: i64,
    /// Twenty five-minute slots, oldest first — the detail strip's width.
    #[ts(type = "number[]")]
    pub buckets: Vec<i64>,
    pub rooms: Vec<AgentRoomSummary>,
    pub events: Vec<AgentEventItem>,
    /// The true count, not `events.len()`.
    #[ts(type = "number")]
    pub event_total: i64,
}

/// The event slice is capped; 50 is chosen, not derived — enough that a normal
/// agent's whole history fits and the cap never shows, few enough that a chatty
/// one does not ship thousands of rows to render a list.
const AGENT_EVENT_LIMIT: i64 = 50;

pub(crate) async fn agent_detail(
    State(app): State<App>,
    Path(name): Path<String>,
) -> Result<Json<AgentDetail>, StatusCode> {
    let row = app
        .store
        .agents()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .find(|a| a.name == name)
        .ok_or(StatusCode::NOT_FOUND)?;

    // Twenty five-minute slots — the detail strip's width, against the rail's
    // twelve. Signature is (scope, now_ms, bucket_ms, buckets); `now_ms` is
    // passed in rather than read inside so the query is deterministic in tests.
    let buckets = app
        .store
        .message_buckets(
            crate::store::BucketScope::Agent(&name),
            crate::store::now_ms(),
            5 * 60_000,
            20,
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rooms = app
        .store
        .agent_rooms(&name)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let (events, event_total) = app
        .store
        .agent_events(&name, AGENT_EVENT_LIMIT)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(AgentDetail {
        name: row.name,
        host: row.host,
        cwd: row.cwd,
        session_id: row.session_id,
        version: row.version,
        // The persisted column is only reconciled at startup; the registry
        // knows who is routable right now. Same reason `agents()` reads it
        // this way instead of trusting the row.
        online: app.registry.is_online(&name).await,
        is_human: row.is_human,
        last_seen: row.last_seen,
        buckets,
        rooms: rooms
            .into_iter()
            .map(|r| AgentRoomSummary {
                name: r.room,
                message_count: r.message_count,
                last_activity: r.last_activity,
            })
            .collect(),
        events: events
            .into_iter()
            .map(|e| AgentEventItem {
                id: e.id,
                kind: e.kind,
                detail: e.detail,
                created_at: e.created_at,
            })
            .collect(),
        event_total,
    }))
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct DeletionPreview {
    /// 1 when the agent row exists, 0 otherwise — the modal states it as a count
    /// beside the others rather than as a special case.
    #[ts(type = "number")]
    pub registration: i64,
    #[ts(type = "number")]
    pub memberships: i64,
    #[ts(type = "number")]
    pub cursors: i64,
    /// For the modal's "on buildbox" clause.
    pub host: String,
    /// Decides whether the dialog opens confirmable or refused. The server
    /// re-checks this at delete time regardless; this is for rendering.
    pub online: bool,
}

pub(crate) async fn agent_deletion_preview(
    State(app): State<App>,
    Path(name): Path<String>,
) -> Result<Json<DeletionPreview>, StatusCode> {
    let row = app
        .store
        .agents()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .find(|a| a.name == name)
        .ok_or(StatusCode::NOT_FOUND)?;

    // A failed read is a 500, never an empty listing. `unwrap_or_default` here
    // would make a database error indistinguishable from "this agent belongs to
    // nothing" while the UI still offers the button — the same reasoning the
    // HTML confirm page documents.
    let fp = app
        .store
        .agent_footprint(&name)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(DeletionPreview {
        registration: 1,
        memberships: fp.rooms.len() as i64,
        cursors: fp.cursors,
        host: row.host,
        online: app.registry.is_online(&name).await,
    }))
}

/// The delete. Refuses cross-origin callers, refuses an unknown name, and
/// refuses an online agent with the registry lock held across the database
/// transaction.
///
/// Guards run in the order that lets each one narrow what the next has to
/// handle, mirroring `delete_agent_perform` next door:
///
/// 1. **Cross-origin.** A request whose `Origin` disagrees with `Host` is
///    refused; a request with *no* `Origin` at all (curl, scripts) is
///    allowed, since it could already reach the port directly — refusing it
///    buys nothing. Same rule as the HTML path's `POST`, and for the same
///    reason (see the module doc), even though the mechanism it defends
///    against doesn't transfer: `DELETE` isn't a CORS-safelisted method, so a
///    browser preflights it and a plain cross-origin `<form>` or no-CORS
///    `fetch` can't forge it the way a `POST` can. The check is kept anyway,
///    for parity with the HTML path and as a second line of defence. `Host`
///    absent — a malformed HTTP/1.1 request — is treated as an empty string,
///    which no real `Origin` matches, so a present `Origin` is refused rather
///    than silently waved through.
/// 2. **Unknown name**, looked up here rather than trusted from the preview —
///    it is what stops any name at all from forging an `agent_deleted` event,
///    and the row it returns supplies the `host` this event carries, matching
///    the HTML delete's shape so the audit log stays uniform across both
///    paths.
/// 3. **Liveness.** `Registry::if_offline` is the authority, not the `online`
///    column: that column is written *after* the registry insert, so a live
///    agent's row can still read offline for a moment. `forget_agent`'s own
///    `online = 0` clause is defence in depth beneath this.
///
/// The response status distinguishes a genuine conflict from a storage
/// failure — both `if_offline` returning `None` and `forget_agent` bailing
/// with `ForgetAgentError::StillOnline` are 409, but `ForgetAgentError::Storage`
/// is a 500. Collapsing the two into one 409 (as this used to) told the client
/// "the agent is still connected", which is not just imprecise but wrong: the
/// client latches a 409 into a state that promises to update itself once the
/// agent goes offline, and a database error will never do that — a permanent
/// dead end dressed as a temporary one.
pub(crate) async fn agent_delete(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: axum::http::HeaderMap,
) -> StatusCode {
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        let host = headers
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if !crate::web::origin_matches_host(origin, host) {
            return StatusCode::FORBIDDEN;
        }
    }

    let row = match app.store.agents().await {
        Ok(rows) => rows.into_iter().find(|a| a.name == name),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
    };
    let Some(row) = row else {
        return StatusCode::NOT_FOUND;
    };

    // The store call inside must not touch the registry — the connection lock
    // is held for its whole duration.
    let outcome = app
        .registry
        .if_offline(&name, || async { app.store.forget_agent(&name).await })
        .await;

    match outcome {
        // The registry's own liveness check disagreed — a genuine conflict.
        None => StatusCode::CONFLICT,
        // `forget_agent`'s own `online = 0` clause caught the same conflict a
        // moment later. Still a 409, not a 500: the agent really is the
        // problem, not the database.
        Some(Err(crate::store::ForgetAgentError::StillOnline)) => StatusCode::CONFLICT,
        // An sqlx failure unrelated to liveness. Mapping this to 409 (as this
        // used to) tells the client "the agent is still connected" — a
        // fabricated, unfalsifiable explanation for what is actually a
        // storage problem, and a 409 the client can never resolve by waiting.
        Some(Err(crate::store::ForgetAgentError::Storage(e))) => {
            eprintln!("DELETE /api/agents/{name} could not remove it: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
        // Two concurrent deletes serialise under the registry lock; the
        // second one to run finds the row already gone and removes nothing.
        // That is "not found", not a successful delete of nothing — and must
        // not append a second `agent_deleted` event for a delete that removed
        // no agent row.
        Some(Ok(counts)) if counts.agents == 0 => StatusCode::NOT_FOUND,
        Some(Ok(counts)) => {
            // Matches `delete_agent_perform`'s event exactly — same kind, same
            // agent (`Some(&name)`, so the deleted agent's own activity log
            // still finds this event), same detail shape — so the audit trail
            // reads the same regardless of which path performed the delete.
            if let Err(e) = app
                .store
                .append_event(
                    "agent_deleted",
                    Some(&name),
                    None,
                    serde_json::json!({
                        "name": name,
                        "host": row.host,
                        "last_seen": row.last_seen,
                        "agents": counts.agents,
                        "memberships": counts.memberships,
                        "cursors": counts.cursors,
                    }),
                )
                .await
            {
                eprintln!("agent_deleted event for {name} was not recorded: {e}");
            }
            StatusCode::NO_CONTENT
        }
    }
}

/// One stored file in a room.
///
/// Deliberately not `proto::FileInfo`, which is the websocket protocol's shape
/// and therefore snake_case on the wire. Every `/api` response is camelCase, and
/// reusing the protocol type would put one snake_case object into an otherwise
/// camelCase surface — the drift this module's header exists to prevent.
#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct RoomFile {
    pub key: String,
    #[ts(type = "number")]
    pub size: i64,
    pub content_type: Option<String>,
    pub updated_by: String,
    /// Epoch milliseconds.
    #[ts(type = "number")]
    pub updated_at: i64,
}

/// A room with no files returns an empty list, not a 404. The client renders
/// "no files" and "could not read the file list" differently, and collapsing
/// them here would make that distinction impossible to draw.
pub(crate) async fn room_files(
    State(app): State<App>,
    Path(name): Path<String>,
) -> Result<Json<Vec<RoomFile>>, StatusCode> {
    let rows = app
        .store
        .list_files(&name)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        rows.into_iter()
            .map(|f| RoomFile {
                key: f.key,
                size: f.size,
                content_type: f.content_type,
                updated_by: f.updated_by,
                updated_at: f.updated_at,
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

#[derive(serde::Deserialize)]
pub(crate) struct HiddenBody {
    hidden: bool,
}

/// Hide or unhide a room.
///
/// The same cross-origin guard the delete path carries, for the same reason: a
/// state-changing POST reachable from a browser against a bus that binds
/// `0.0.0.0` with no authentication. A request with no `Origin` is allowed —
/// those callers could already reach the port directly.
///
/// 404 for an unknown room, deliberately the opposite of
/// `GET /api/rooms/{name}/files`, which answers an unknown room with an empty
/// list. A read of "what is in this room" is answerable for a room with nothing
/// in it; a write to a room that does not exist is not a request that can be
/// honoured.
pub(crate) async fn room_set_hidden(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<HiddenBody>,
) -> StatusCode {
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        let host = headers
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if !crate::web::origin_matches_host(origin, host) {
            return StatusCode::FORBIDDEN;
        }
    }

    match app.store.set_room_hidden(&name, body.hidden).await {
        Ok(true) => {
            let kind = if body.hidden {
                "room_hidden"
            } else {
                "room_unhidden"
            };
            if let Err(e) = app
                .store
                .append_event(kind, None, Some(&name), serde_json::json!({}))
                .await
            {
                eprintln!("{kind} event for {name} was not recorded: {e}");
            }
            StatusCode::NO_CONTENT
        }
        Ok(false) => StatusCode::NOT_FOUND,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
