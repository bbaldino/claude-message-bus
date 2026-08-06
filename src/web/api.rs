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
use axum::extract::State;

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
pub(crate) async fn agents(State(app): State<App>) -> Json<Vec<Agent>> {
    let rows = app.store.agents().await.unwrap_or_default();
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
    Json(out)
}
