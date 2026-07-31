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
    pub is_human: bool,
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

        let store = Self { pool, blobs_dir };
        store.migrate().await?;

        Ok(store)
    }

    /// Bring an existing database up to the current schema.
    ///
    /// `schema.sql` is all `CREATE TABLE IF NOT EXISTS`, which covers a fresh file but
    /// does nothing for a database created before a column existed — and the deployed
    /// bus keeps its data in a named Docker volume that long outlives any one binary.
    async fn migrate(&self) -> anyhow::Result<()> {
        self.add_column_if_missing("agents", "is_human", "INTEGER NOT NULL DEFAULT 0")
            .await?;
        self.add_column_if_missing("messages", "human", "INTEGER NOT NULL DEFAULT 0")
            .await?;
        Ok(())
    }

    /// SQLite has no `ADD COLUMN IF NOT EXISTS`, so this asks `PRAGMA table_info` what is
    /// actually there rather than issuing the `ALTER` and swallowing the resulting error —
    /// an error whose message is not part of any stability guarantee, and which would hide
    /// a genuinely failed migration behind the same catch.
    async fn add_column_if_missing(
        &self,
        table: &str,
        column: &str,
        ddl: &str,
    ) -> anyhow::Result<()> {
        // `table` and `column` are compile-time literals from `migrate`, never user input;
        // PRAGMA and ALTER take no bind parameters for identifiers. `AssertSqlSafe` is
        // sqlx's speed bump against building dynamic SQL from untrusted data, not a
        // statement that these strings are literally static.
        let cols = sqlx::query(sqlx::AssertSqlSafe(format!("PRAGMA table_info({table})")))
            .fetch_all(&self.pool)
            .await?;
        let present = cols.iter().any(|r| r.get::<String, _>("name") == column);
        if !present {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "ALTER TABLE {table} ADD COLUMN {column} {ddl}"
            )))
            .execute(&self.pool)
            .await?;
        }
        Ok(())
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
        is_human: bool,
    ) -> anyhow::Result<()> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO agents (name, host, cwd, session_id, connected_at, last_seen, online, is_human)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, 1, ?6)
             ON CONFLICT(name) DO UPDATE SET
               host = ?2, cwd = ?3, session_id = ?4, last_seen = ?5, online = 1, is_human = ?6",
        )
        .bind(name)
        .bind(host)
        .bind(cwd)
        .bind(session_id)
        .bind(now)
        .bind(is_human)
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

    /// Mark every agent offline. Called once when a bus starts.
    ///
    /// `online` is persisted, but the connection registry that actually knows who is
    /// connected is in memory and therefore empty at startup — so any row still claiming
    /// `online` is stale by definition. It gets that way whenever a bus process dies
    /// without running its per-connection teardown (a kill, a crash, `docker compose up
    /// --build` recreating the container), which skips the `set_online(false)` that a
    /// graceful disconnect would have performed. Without this reconciliation those rows
    /// stay `online` forever, and the agent list shows ghosts that no longer exist.
    pub async fn mark_all_offline(&self) -> anyhow::Result<()> {
        sqlx::query("UPDATE agents SET online = 0 WHERE online != 0")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn agents(&self) -> anyhow::Result<Vec<AgentRow>> {
        let rows = sqlx::query(
            "SELECT name, host, cwd, session_id, online, is_human FROM agents ORDER BY name",
        )
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
                is_human: r.get::<i64, _>("is_human") != 0,
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

    /// Whether `room` has ever been created, independently of who is in it.
    ///
    /// Membership is not a proxy for existence. A human's membership is dropped when
    /// they disconnect (see `leave_all_rooms`), so a room they were the only
    /// participant in keeps its `rooms` row and its messages while holding no members
    /// at all — a state that could not arise while every membership was durable.
    pub async fn room_exists(&self, room: &str) -> anyhow::Result<bool> {
        let row = sqlx::query("SELECT 1 FROM rooms WHERE name = ?1")
            .bind(room)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    /// Drop every room membership held by `agent`.
    ///
    /// Used for humans only. An agent's membership is durable — that is what makes
    /// messages queue for it while it is away — but a human dipping into a room is not
    /// a subscriber, and leaving them a member would report them in `queued_for` on
    /// every later send, telling agents a reply was pending from someone who had gone.
    pub async fn leave_all_rooms(&self, agent: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM room_members WHERE agent_name = ?1")
            .bind(agent)
            .execute(&self.pool)
            .await?;
        Ok(())
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
        human: bool,
    ) -> anyhow::Result<i64> {
        self.ensure_room(room).await?;
        let res = sqlx::query(
            "INSERT INTO messages (room, from_agent, body, done, created_at, human)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(room)
        .bind(from)
        .bind(body)
        .bind(done as i64)
        .bind(now_ms())
        .bind(human)
        .execute(self.pool())
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// The most recent `limit` messages, returned oldest-first so the reader
    /// sees them in conversational order.
    pub async fn history(&self, room: &str, limit: i64) -> anyhow::Result<Vec<MessageRow>> {
        let rows = sqlx::query(
            "SELECT * FROM (
               SELECT id, room, from_agent, body, done, created_at, human
               FROM messages WHERE room = ?1 ORDER BY id DESC LIMIT ?2
             ) ORDER BY id ASC",
        )
        .bind(room)
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.iter().map(message_row).collect())
    }

    /// The most recent `limit` messages across every room, newest first.
    ///
    /// `history` answers "what happened in this room"; this answers "what is happening
    /// at all", which is the question an overview page exists for. Newest-first because
    /// the caller is scanning for the latest activity, not reading a conversation.
    pub async fn recent_messages(&self, limit: i64) -> anyhow::Result<Vec<MessageRow>> {
        let rows = sqlx::query(
            "SELECT id, room, from_agent, body, done, created_at, human
             FROM messages ORDER BY id DESC LIMIT ?1",
        )
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
            "SELECT id, room, from_agent, body, done, created_at, human
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
    pub human: bool,
}

fn message_row(r: &sqlx::sqlite::SqliteRow) -> MessageRow {
    MessageRow {
        id: r.get("id"),
        room: r.get("room"),
        from_agent: r.get("from_agent"),
        body: r.get("body"),
        done: r.get::<i64, _>("done") != 0,
        created_at: r.get("created_at"),
        human: r.get::<i64, _>("human") != 0,
    }
}
