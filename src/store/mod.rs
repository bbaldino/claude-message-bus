//! Storage. Knows nothing about WebSockets or MCP — it is a plain persistence
//! facade over SQLite, with blobs on disk.

use std::path::Path;

use anyhow::Context;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

mod events;
pub use events::EventRow;

mod files;
pub use files::{FileRow, MAX_BLOB_BYTES};

const SCHEMA: &str = include_str!("../../schema.sql");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub name: String,
    pub host: String,
    pub cwd: String,
    pub session_id: Option<String>,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomRow {
    pub name: String,
    pub mode: String,
    pub members: Vec<String>,
}

pub struct Store {
    pool: SqlitePool,
    blobs_dir: std::path::PathBuf,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Store {
    pub async fn open(dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir).context("creating data dir")?;
        let blobs_dir = dir.join("blobs");
        std::fs::create_dir_all(&blobs_dir).context("creating blobs dir")?;

        let opts = SqliteConnectOptions::new()
            .filename(dir.join("bus.db"))
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .context("opening sqlite")?;

        // Schema is idempotent (CREATE TABLE IF NOT EXISTS), so applying it on
        // every start doubles as the migration story for a single-writer service.
        for statement in SCHEMA.split(';') {
            let sql = statement.trim();
            if sql.is_empty() {
                continue;
            }
            sqlx::query(sql)
                .execute(&pool)
                .await
                .with_context(|| format!("applying schema statement: {sql}"))?;
        }

        Ok(Self { pool, blobs_dir })
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Test-only accessor. Production code goes through the typed methods.
    #[doc(hidden)]
    pub fn pool_for_test(&self) -> &SqlitePool {
        &self.pool
    }

    pub(crate) fn blobs_dir(&self) -> &Path {
        &self.blobs_dir
    }

    pub async fn upsert_agent(
        &self,
        name: &str,
        host: &str,
        cwd: &str,
        session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO agents (name, host, cwd, session_id, connected_at, last_seen, online)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, 1)
             ON CONFLICT(name) DO UPDATE SET
               host = ?2, cwd = ?3, session_id = ?4, last_seen = ?5, online = 1",
        )
        .bind(name)
        .bind(host)
        .bind(cwd)
        .bind(session_id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_online(&self, name: &str, online: bool) -> anyhow::Result<()> {
        sqlx::query("UPDATE agents SET online = ?2, last_seen = ?3 WHERE name = ?1")
            .bind(name)
            .bind(online as i64)
            .bind(now_ms())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn agents(&self) -> anyhow::Result<Vec<AgentRow>> {
        let rows =
            sqlx::query("SELECT name, host, cwd, session_id, online FROM agents ORDER BY name")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|r| AgentRow {
                name: r.get("name"),
                host: r.get("host"),
                cwd: r.get("cwd"),
                session_id: r.get("session_id"),
                online: r.get::<i64, _>("online") != 0,
            })
            .collect())
    }

    pub async fn ensure_room(&self, room: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO rooms (name, mode, created_at) VALUES (?1, 'discuss', ?2)
             ON CONFLICT(name) DO NOTHING",
        )
        .bind(room)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn join_room(&self, room: &str, agent: &str) -> anyhow::Result<()> {
        self.ensure_room(room).await?;
        sqlx::query(
            "INSERT INTO room_members (room, agent_name) VALUES (?1, ?2)
             ON CONFLICT(room, agent_name) DO NOTHING",
        )
        .bind(room)
        .bind(agent)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn room_members(&self, room: &str) -> anyhow::Result<Vec<String>> {
        let rows =
            sqlx::query("SELECT agent_name FROM room_members WHERE room = ?1 ORDER BY agent_name")
                .bind(room)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|r| r.get("agent_name")).collect())
    }

    pub async fn rooms(&self) -> anyhow::Result<Vec<RoomRow>> {
        let rows = sqlx::query("SELECT name, mode FROM rooms ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let name: String = r.get("name");
            let members = self.room_members(&name).await?;
            out.push(RoomRow {
                name,
                mode: r.get("mode"),
                members,
            });
        }
        Ok(out)
    }

    pub async fn append_message(
        &self,
        room: &str,
        from: &str,
        body: &str,
        done: bool,
    ) -> anyhow::Result<i64> {
        self.ensure_room(room).await?;
        let res = sqlx::query(
            "INSERT INTO messages (room, from_agent, body, done, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(room)
        .bind(from)
        .bind(body)
        .bind(done as i64)
        .bind(now_ms())
        .execute(self.pool())
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// The most recent `limit` messages, returned oldest-first so the reader
    /// sees them in conversational order.
    pub async fn history(&self, room: &str, limit: i64) -> anyhow::Result<Vec<MessageRow>> {
        let rows = sqlx::query(
            "SELECT * FROM (
               SELECT id, room, from_agent, body, done, created_at
               FROM messages WHERE room = ?1 ORDER BY id DESC LIMIT ?2
             ) ORDER BY id ASC",
        )
        .bind(room)
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.iter().map(message_row).collect())
    }

    pub async fn cursor(&self, room: &str, agent: &str) -> anyhow::Result<i64> {
        let row = sqlx::query(
            "SELECT last_delivered_id FROM cursors WHERE room = ?1 AND agent_name = ?2",
        )
        .bind(room)
        .bind(agent)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(|r| r.get("last_delivered_id")).unwrap_or(0))
    }

    /// Never regresses: a stale or out-of-order ack (or a `history` call that
    /// only saw an older window of messages) must not drag the cursor
    /// backwards and silently resurrect already-read messages as unread.
    pub async fn set_cursor(&self, room: &str, agent: &str, id: i64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO cursors (room, agent_name, last_delivered_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(room, agent_name) DO UPDATE SET
               last_delivered_id = MAX(last_delivered_id, ?3)",
        )
        .bind(room)
        .bind(agent)
        .bind(id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn unread_count(&self, room: &str, agent: &str) -> anyhow::Result<i64> {
        let cursor = self.cursor(room, agent).await?;
        let row = sqlx::query(
            "SELECT COUNT(*) AS n FROM messages WHERE room = ?1 AND id > ?2 AND from_agent != ?3",
        )
        .bind(room)
        .bind(cursor)
        .bind(agent)
        .fetch_one(self.pool())
        .await?;
        Ok(row.get("n"))
    }

    /// Messages this agent has not been shown yet, excluding its own.
    pub async fn undelivered(&self, room: &str, agent: &str) -> anyhow::Result<Vec<MessageRow>> {
        let cursor = self.cursor(room, agent).await?;
        let rows = sqlx::query(
            "SELECT id, room, from_agent, body, done, created_at
             FROM messages WHERE room = ?1 AND id > ?2 AND from_agent != ?3 ORDER BY id ASC",
        )
        .bind(room)
        .bind(cursor)
        .bind(agent)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.iter().map(message_row).collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    pub id: i64,
    pub room: String,
    pub from_agent: String,
    pub body: String,
    pub done: bool,
    pub created_at: i64,
}

fn message_row(r: &sqlx::sqlite::SqliteRow) -> MessageRow {
    MessageRow {
        id: r.get("id"),
        room: r.get("room"),
        from_agent: r.get("from_agent"),
        body: r.get("body"),
        done: r.get::<i64, _>("done") != 0,
        created_at: r.get("created_at"),
    }
}
