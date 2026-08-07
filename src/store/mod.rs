//! Storage. Knows nothing about WebSockets or MCP — it is a plain persistence
//! facade over SQLite, with blobs on disk.

use std::path::Path;

use anyhow::Context;
use serde_json::Value;
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
    pub version: Option<String>,
    /// Epoch milliseconds of the last registration or online/offline transition.
    /// Written since the beginning; this is the first thing to read it back.
    pub last_seen: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomRow {
    pub name: String,
    pub mode: String,
    pub members: Vec<String>,
}

/// What deleting an agent would remove, for display before it happens.
///
/// Rooms are names rather than a count because the confirmation page lists
/// them individually — a count would not tell anyone what they were losing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFootprint {
    pub rooms: Vec<String>,
    pub cursors: i64,
}

/// One room an agent belongs to, with its own traffic in that room.
#[derive(Debug, Clone)]
pub struct AgentRoomRow {
    pub room: String,
    pub message_count: i64,
    pub last_activity: i64,
}

/// Rows actually removed by `forget_agent`, for the audit event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForgetCounts {
    pub agents: u64,
    pub memberships: u64,
    pub cursors: u64,
}

pub struct Store {
    pool: SqlitePool,
    blobs_dir: std::path::PathBuf,
    events_tx: tokio::sync::broadcast::Sender<crate::store::events::EventRow>,
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

        // 256 is ample for a personal bus; a lagging receiver drops rather than
        // blocking the write path, which is the correct trade for an audit tail.
        let (events_tx, _) = tokio::sync::broadcast::channel(256);

        let store = Self {
            pool,
            blobs_dir,
            events_tx,
        };
        store.migrate().await?;

        Ok(store)
    }

    /// Subscribe to events as they are appended.
    ///
    /// A broadcast channel rather than a registry callback, so `Store` stays
    /// unaware of the bus. Hooking `append_event` — the single funnel every kind
    /// already passes through — is what stops a kind added later from silently
    /// failing to appear in the dock.
    pub fn subscribe_events(
        &self,
    ) -> tokio::sync::broadcast::Receiver<crate::store::events::EventRow> {
        self.events_tx.subscribe()
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
        self.add_column_if_missing("agents", "version", "TEXT")
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
        version: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO agents (name, host, cwd, session_id, connected_at, last_seen, online, is_human, version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, 1, ?6, ?7)
             ON CONFLICT(name) DO UPDATE SET
               host = ?2, cwd = ?3, session_id = ?4, last_seen = ?5, online = 1,
               is_human = ?6, version = ?7",
        )
        .bind(name)
        .bind(host)
        .bind(cwd)
        .bind(session_id)
        .bind(now)
        .bind(is_human)
        .bind(version)
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
            "SELECT name, host, cwd, session_id, online, is_human, version, last_seen
             FROM agents ORDER BY last_seen DESC, name",
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
                version: r.get("version"),
                last_seen: r.get("last_seen"),
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

    /// The rooms and cursors `forget_agent` would delete. Mutates nothing.
    pub async fn agent_footprint(&self, name: &str) -> anyhow::Result<AgentFootprint> {
        let rooms: Vec<String> =
            sqlx::query("SELECT room FROM room_members WHERE agent_name = ?1 ORDER BY room")
                .bind(name)
                .fetch_all(&self.pool)
                .await?
                .iter()
                .map(|r| r.get("room"))
                .collect();
        let cursors: i64 = sqlx::query("SELECT COUNT(*) AS n FROM cursors WHERE agent_name = ?1")
            .bind(name)
            .fetch_one(&self.pool)
            .await?
            .get("n");
        Ok(AgentFootprint { rooms, cursors })
    }

    /// One row per room this agent belongs to, with how many messages it sent there
    /// and when it last did.
    ///
    /// Grouped in SQL rather than counted in Rust for the same reason as the volume
    /// buckets: the alternative ships every message body to count them.
    ///
    /// `message_buckets` takes `now_ms` as a parameter rather than reading the clock
    /// itself; this follows the same house style by taking none and letting the
    /// caller decide what "recent" means, since it reports absolute timestamps.
    pub async fn agent_rooms(&self, name: &str) -> anyhow::Result<Vec<AgentRoomRow>> {
        let rows = sqlx::query(
            "SELECT rm.room AS room, \
                    COUNT(m.id) AS n, \
                    COALESCE(MAX(m.created_at), 0) AS last_at \
             FROM room_members rm \
             LEFT JOIN messages m ON m.room = rm.room AND m.from_agent = rm.agent_name \
             WHERE rm.agent_name = ?1 \
             GROUP BY rm.room ORDER BY last_at DESC, rm.room",
        )
        .bind(name)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| AgentRoomRow {
                room: r.get("room"),
                message_count: r.get("n"),
                last_activity: r.get("last_at"),
            })
            .collect())
    }

    /// The agent's most recent events, newest first, plus the true total.
    ///
    /// The total is a separate COUNT rather than the slice's length because the
    /// screen's section header states "312 total" while showing far fewer.
    pub async fn agent_events(
        &self,
        name: &str,
        limit: i64,
    ) -> anyhow::Result<(Vec<crate::store::events::EventRow>, i64)> {
        let total: i64 = sqlx::query("SELECT COUNT(*) AS n FROM events WHERE agent = ?1")
            .bind(name)
            .fetch_one(&self.pool)
            .await?
            .get("n");
        // `events_for_agent` already selects `WHERE agent = ?1 ORDER BY id DESC
        // LIMIT ?2` through the shared `event_row` mapping — exactly what this
        // needs, so it is reused rather than adding a second query beside it.
        let rows = self.events_for_agent(name, limit).await?;
        Ok((rows, total))
    }

    /// Delete an agent's own rows: its `agents` entry, its room memberships,
    /// and its cursors. Messages and events are deliberately untouched — the
    /// transcript stays readable and the audit trail outlives the agent.
    ///
    /// Transactional because a partial failure is worse than none: losing the
    /// `agents` row while leaving memberships behind strands them, since the
    /// row is what makes an agent reachable in the UI and therefore deletable.
    ///
    /// Refuses an agent whose `online` column is set, rolling the whole
    /// transaction back. This is defence in depth, not the real guard: the
    /// authority for liveness is the in-memory registry (see
    /// `Registry::if_offline`, which the web handler holds across this call),
    /// and `upsert_agent` sets `online = 1` only *after* the registry insert,
    /// so there is a window where a live agent's column still reads offline.
    /// What this does buy is that a caller added later — this is a public
    /// method whose only other protection lives in a different module —
    /// cannot silently drop a connected agent's memberships.
    pub async fn forget_agent(&self, name: &str) -> anyhow::Result<ForgetCounts> {
        let mut tx = self.pool.begin().await?;
        let memberships = sqlx::query("DELETE FROM room_members WHERE agent_name = ?1")
            .bind(name)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let cursors = sqlx::query("DELETE FROM cursors WHERE agent_name = ?1")
            .bind(name)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let agents = sqlx::query("DELETE FROM agents WHERE name = ?1 AND online = 0")
            .bind(name)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if agents == 0 {
            // Zero rows means either "no such agent" — which is not an error,
            // the caller asked to forget something already forgotten — or "the
            // row is there but online", which must take the memberships and
            // cursors back with it.
            let still_there = sqlx::query("SELECT 1 FROM agents WHERE name = ?1")
                .bind(name)
                .fetch_optional(&mut *tx)
                .await?
                .is_some();
            if still_there {
                tx.rollback().await?;
                anyhow::bail!("{name} is online; only offline agents can be deleted");
            }
        }
        tx.commit().await?;
        Ok(ForgetCounts {
            agents,
            memberships,
            cursors,
        })
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
        self.append_message_at(room, from, body, done, human, now_ms())
            .await
    }

    /// `append_message` with an explicit timestamp.
    ///
    /// Exists because bucket and flag logic is time-dependent, and a test that
    /// cannot choose when a message happened can only assert "something landed
    /// somewhere" — which is not a test of bucketing.
    pub async fn append_message_at(
        &self,
        room: &str,
        from: &str,
        body: &str,
        done: bool,
        human: bool,
        created_at: i64,
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
        .bind(created_at)
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

    /// The `limit` messages immediately before `before_id`, returned
    /// oldest-first like `history` — the backwards-scrollback counterpart.
    ///
    /// `history` is left untouched rather than growing an `Option<i64>`
    /// cursor parameter: it has many call sites, and threading an unused
    /// `AND id < ?` through all of them for the sake of this one caller would
    /// be the wrong trade.
    pub async fn history_before(
        &self,
        room: &str,
        before_id: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<MessageRow>> {
        let rows = sqlx::query(
            "SELECT * FROM (
               SELECT id, room, from_agent, body, done, created_at, human
               FROM messages WHERE room = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3
             ) ORDER BY id ASC",
        )
        .bind(room)
        .bind(before_id)
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

/// Which messages a volume strip counts.
#[derive(Debug, Clone, Copy)]
pub enum BucketScope<'a> {
    Room(&'a str),
    Agent(&'a str),
}

impl Store {
    /// Messages per time slot, oldest slot first, always exactly `buckets` long.
    ///
    /// Grouped in SQL rather than by fetching rows and counting in Rust: the rail
    /// draws one of these per room *and* per agent on every poll, and shipping an
    /// hour of message bodies to count them is the thing this exists to avoid.
    ///
    /// Slot 0 is the newest, so the result is reversed before returning — a strip
    /// reads left to right as time moving forward.
    pub async fn message_buckets(
        &self,
        scope: BucketScope<'_>,
        now_ms: i64,
        bucket_ms: i64,
        buckets: usize,
    ) -> anyhow::Result<Vec<i64>> {
        let window_start = now_ms - (bucket_ms * buckets as i64);
        let sql = match scope {
            BucketScope::Room(_) => {
                "SELECT ((?2 - created_at) / ?3) AS slot, COUNT(*) AS n
                 FROM messages WHERE room = ?1 AND created_at > ?4
                 GROUP BY slot"
            }
            BucketScope::Agent(_) => {
                "SELECT ((?2 - created_at) / ?3) AS slot, COUNT(*) AS n
                 FROM messages WHERE from_agent = ?1 AND created_at > ?4
                 GROUP BY slot"
            }
        };
        let key = match scope {
            BucketScope::Room(r) => r,
            BucketScope::Agent(a) => a,
        };
        let rows = sqlx::query(sql)
            .bind(key)
            .bind(now_ms)
            .bind(bucket_ms)
            .bind(window_start)
            .fetch_all(self.pool())
            .await?;

        let mut out = vec![0i64; buckets];
        for r in rows {
            let slot: i64 = r.get("slot");
            if slot >= 0 && (slot as usize) < buckets {
                out[buckets - 1 - slot as usize] = r.get("n");
            }
        }
        Ok(out)
    }
}

/// A room's state, derived from the event stream and membership rather than
/// stored as a column. Only two exist, deliberately — an earlier design draft
/// had four and they blurred together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomFlag {
    /// The exchange cap tripped and the room cannot continue without a person.
    NeedsYou { exchanges: i64 },
    /// Messages are waiting for members who are all offline.
    Blocked {
        queued: i64,
        waiting_on: Vec<String>,
    },
}

impl Store {
    /// Every room whose latest pause/resume event is a pause — the rooms the
    /// log currently claims are waiting on a person.
    ///
    /// Exists for startup reconciliation. The exchange guard's counters live in
    /// memory (`bus::delivery::Guards`), so a fresh process has no paused rooms
    /// at all, while every `room_paused` row survives in the database — leaving
    /// every previously paused room flagged `needs_you` forever after a restart.
    /// This is the same class of stale-truth problem `mark_all_offline` solves
    /// for presence, and it is fixed the same way and in the same place.
    pub async fn paused_rooms(&self) -> anyhow::Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT e.room AS room, e.kind AS kind FROM events e
             WHERE e.room IS NOT NULL
               AND e.kind IN ('room_paused', 'resumed')
               AND e.id = (
                 SELECT MAX(id) FROM events
                 WHERE room = e.room AND kind IN ('room_paused', 'resumed')
               )",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .filter(|r| r.get::<String, _>("kind") == "room_paused")
            .map(|r| r.get("room"))
            .collect())
    }

    /// The room's flag, if any. `online` is the registry's live name list —
    /// liveness is never read from the persisted `agents.online` column, which
    /// is only reconciled at process start.
    ///
    /// `NeedsYou` wins over `Blocked`: it is the state addressed to the operator,
    /// so if both hold, the one asking for action is the one shown.
    pub async fn room_flag(
        &self,
        room: &str,
        online: &[String],
    ) -> anyhow::Result<Option<RoomFlag>> {
        // Latest of the pause/resume pair decides. `room_paused` is the exchange
        // cap; `rate_limited` is the send-interval limiter and is NOT this.
        let paused = sqlx::query(
            "SELECT kind, detail_json FROM events
             WHERE room = ?1 AND kind IN ('room_paused', 'resumed')
             ORDER BY id DESC LIMIT 1",
        )
        .bind(room)
        .fetch_optional(self.pool())
        .await?;

        if let Some(row) = paused {
            let kind: String = row.get("kind");
            if kind == "room_paused" {
                let detail: Value =
                    serde_json::from_str(&row.get::<String, _>("detail_json")).unwrap_or_default();
                let exchanges = detail.get("count").and_then(Value::as_i64).unwrap_or(0);
                return Ok(Some(RoomFlag::NeedsYou { exchanges }));
            }
        }

        let members = self.room_members(room).await?;
        if members.is_empty() || members.iter().any(|m| online.contains(m)) {
            return Ok(None);
        }

        let mut waiting_on = Vec::new();
        for m in &members {
            if self.unread_count(room, m).await? > 0 {
                waiting_on.push(m.clone());
            }
        }
        if waiting_on.is_empty() {
            return Ok(None);
        }
        let queued = self.queued_message_count(room, &waiting_on).await?;
        Ok(Some(RoomFlag::Blocked { queued, waiting_on }))
    }

    /// How many distinct messages in `room` at least one of `waiting` has not
    /// seen.
    ///
    /// Messages, not deliveries. Summing `unread_count` across members counts the
    /// same message once per member who has not read it, so two messages and two
    /// offline members reads as four — while the rail renders the number as a
    /// message count ("2 queued, 0 delivered"). The server ships data rather than
    /// sentences precisely so the client can write that sentence, which only works
    /// if the datum means what the sentence says.
    ///
    /// `from_agent != rm.agent_name` matches `unread_count`'s rule: a member's own
    /// message is never unread for it, and so never counts as queued for it.
    pub async fn queued_message_count(
        &self,
        room: &str,
        waiting: &[String],
    ) -> anyhow::Result<i64> {
        if waiting.is_empty() {
            return Ok(0);
        }
        // `?1` is the room; the waiting members take `?2` onwards. Only the
        // placeholder *count* varies with the input — every value is still bound,
        // so no caller data reaches the SQL text. `AssertSqlSafe` is sqlx's speed
        // bump against building SQL from untrusted data, not a claim of staticness
        // (see `add_column_if_missing`, which does the same for PRAGMA).
        let placeholders = (0..waiting.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT COUNT(DISTINCT m.id) AS n
             FROM messages m
             JOIN room_members rm
               ON rm.room = m.room AND rm.agent_name IN ({placeholders})
             LEFT JOIN cursors c
               ON c.room = m.room AND c.agent_name = rm.agent_name
             WHERE m.room = ?1
               AND m.from_agent != rm.agent_name
               AND m.id > COALESCE(c.last_delivered_id, 0)"
        );
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(room);
        for w in waiting {
            q = q.bind(w);
        }
        Ok(q.fetch_one(self.pool()).await?.get("n"))
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
