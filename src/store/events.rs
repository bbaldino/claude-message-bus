//! The event log: what the bus did, as opposed to what agents said.
//!
//! Records mechanical churn (acks, cursor advances) as well as notable events,
//! deliberately. The longest-lived defect this project produced — `ToBus::Ack` having no
//! producer at all — was visible only as an *absence*, and an absence is only meaningful
//! against an expectation. A log that skipped boring events would not have shown it.

use serde_json::Value;
use sqlx::Row;

use super::{Store, now_ms};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    pub id: i64,
    pub created_at: i64,
    pub kind: String,
    pub agent: Option<String>,
    pub room: Option<String>,
    pub detail: Value,
}

fn event_row(r: &sqlx::sqlite::SqliteRow) -> EventRow {
    let raw: String = r.get("detail_json");
    EventRow {
        id: r.get("id"),
        created_at: r.get("created_at"),
        kind: r.get("kind"),
        agent: r.get("agent"),
        room: r.get("room"),
        // A malformed row degrades to Null rather than failing the whole query and
        // hiding every other event on the page.
        detail: serde_json::from_str(&raw).unwrap_or(Value::Null),
    }
}

impl Store {
    pub async fn append_event(
        &self,
        kind: &str,
        agent: Option<&str>,
        room: Option<&str>,
        detail: Value,
    ) -> anyhow::Result<i64> {
        let res = sqlx::query(
            "INSERT INTO events (created_at, kind, agent, room, detail_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(now_ms())
        .bind(kind)
        .bind(agent)
        .bind(room)
        .bind(detail.to_string())
        .execute(self.pool())
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// Most recent first — what a dashboard wants.
    pub async fn events(&self, limit: i64) -> anyhow::Result<Vec<EventRow>> {
        let rows = sqlx::query("SELECT * FROM events ORDER BY id DESC LIMIT ?1")
            .bind(limit)
            .fetch_all(self.pool())
            .await?;
        Ok(rows.iter().map(event_row).collect())
    }

    /// Oldest first — the transcript view merges these with messages in the order they
    /// happened, so this one deliberately differs from the others.
    pub async fn events_for_room(&self, room: &str, limit: i64) -> anyhow::Result<Vec<EventRow>> {
        let rows = sqlx::query(
            "SELECT * FROM (
               SELECT * FROM events WHERE room = ?1 ORDER BY id DESC LIMIT ?2
             ) ORDER BY id ASC",
        )
        .bind(room)
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.iter().map(event_row).collect())
    }

    pub async fn events_for_agent(&self, agent: &str, limit: i64) -> anyhow::Result<Vec<EventRow>> {
        let rows = sqlx::query("SELECT * FROM events WHERE agent = ?1 ORDER BY id DESC LIMIT ?2")
            .bind(agent)
            .bind(limit)
            .fetch_all(self.pool())
            .await?;
        Ok(rows.iter().map(event_row).collect())
    }

    pub async fn events_of_kind(&self, kind: &str, limit: i64) -> anyhow::Result<Vec<EventRow>> {
        let rows = sqlx::query("SELECT * FROM events WHERE kind = ?1 ORDER BY id DESC LIMIT ?2")
            .bind(kind)
            .bind(limit)
            .fetch_all(self.pool())
            .await?;
        Ok(rows.iter().map(event_row).collect())
    }
}
