# Observability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An event log recording everything the bus does, plus read-only server-rendered web views over it and the existing tables, so agent conversations and bus behaviour can be audited after the fact.

**Architecture:** A new `events` table written from the bus's existing operation sites, and a new `src/web/` module serving HTML from the axum server already running in `claude-bus serve` — same binary, same port, same container. No new deployment, no JavaScript toolchain, no writes.

**Tech Stack:** Rust 2024, `axum` (already present), `sqlx` (SQLite, runtime queries), `serde_json`. No new dependencies.

## Global Constraints

- **Add dependencies with `cargo add` only.** This plan needs none — everything used is already in `Cargo.toml`.
- **Use `sqlx` runtime queries (`sqlx::query`), not the compile-time macros.** The macros need `DATABASE_URL` at build time.
- **Format with `cargo +nightly fmt`**; `cargo clippy --all-targets` must stay clean.
- **Acronym casing: only the first letter is capitalized.** `WsClient` not `WSClient`, `HtmlPage` not `HTMLPage`.
- **`println!` is permitted only in `src/init.rs` and `src/tail.rs`.** Everywhere else — including all new web code — use `eprintln!`. In the agent path stdout is the JSON-RPC transport.
- **Every value interpolated into HTML must be escaped.** Message bodies, agent names, room names, and file keys are all model-generated or user-supplied text. Rendering any of them raw is an XSS vector in your own browser.
- **A logging failure must never fail the operation being logged.**
- **The UI performs no writes.** No `POST`, `PUT`, or `DELETE` routes.

---

## File Structure

```
schema.sql                    -- + events table and its indexes
src/store/events.rs           -- NEW: append_event + event queries
src/store/mod.rs              -- + mod events; re-export EventRow
src/bus/commands.rs           -- NEW: handle() and its arms, moved out of mod.rs
src/bus/mod.rs                -- connection loop, routes; + event writes at operation sites
src/web/mod.rs                -- NEW: router, shared layout, HTTP handlers
src/web/html.rs               -- NEW: escaping and small HTML builders
src/main.rs                   -- unchanged (web routes mount inside bus::serve_on_with)
tests/events.rs               -- NEW: event storage
tests/web.rs                  -- NEW: HTTP handlers over a temp SQLite
docs/DEPLOY.md                -- + how to reach the UI
```

`src/bus/mod.rs` is 890 lines and mixes the WebSocket connection loop, the command
handler, and HTTP routes. Two prior reviews flagged it at 496 lines and recommended
exactly this split. Task 2 does it as a pure move before Task 3 adds roughly ten event
write sites, so those land in a file that can be read.

---

### Task 1: Event log storage

**Files:**
- Modify: `schema.sql`
- Create: `src/store/events.rs`
- Modify: `src/store/mod.rs`
- Test: `tests/events.rs`

**Interfaces:**
- Consumes: `Store` from the existing storage layer, `store::now_ms()`.
- Produces:
  - `pub struct EventRow { pub id: i64, pub created_at: i64, pub kind: String, pub agent: Option<String>, pub room: Option<String>, pub detail: serde_json::Value }`
  - `async fn append_event(&self, kind: &str, agent: Option<&str>, room: Option<&str>, detail: serde_json::Value) -> anyhow::Result<i64>`
  - `async fn events(&self, limit: i64) -> anyhow::Result<Vec<EventRow>>` — most recent first
  - `async fn events_for_room(&self, room: &str, limit: i64) -> anyhow::Result<Vec<EventRow>>` — oldest first, for interleaving with a transcript
  - `async fn events_for_agent(&self, agent: &str, limit: i64) -> anyhow::Result<Vec<EventRow>>` — most recent first
  - `async fn events_of_kind(&self, kind: &str, limit: i64) -> anyhow::Result<Vec<EventRow>>` — most recent first

- [ ] **Step 1: Add the table to `schema.sql`**

Append to `schema.sql`:

```sql
CREATE TABLE IF NOT EXISTS events (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  created_at  INTEGER NOT NULL,
  kind        TEXT NOT NULL,
  agent       TEXT,
  room        TEXT,
  detail_json TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS events_room_id  ON events(room, id);
CREATE INDEX IF NOT EXISTS events_agent_id ON events(agent, id);
CREATE INDEX IF NOT EXISTS events_kind_id  ON events(kind, id);
```

`agent` and `room` are nullable because not every event has both.

- [ ] **Step 2: Write the failing tests**

Create `tests/events.rs`:

```rust
use claude_bus::store::Store;
use serde_json::json;

async fn temp_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).await.expect("open store");
    (dir, store)
}

#[tokio::test]
async fn appends_and_reads_back_an_event() {
    let (_d, store) = temp_store().await;
    store
        .append_event("message_sent", Some("caas"), Some("protocol"),
                      json!({"msg_id": 7, "delivered_to": ["dashboard"]}))
        .await
        .unwrap();

    let evs = store.events(10).await.unwrap();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].kind, "message_sent");
    assert_eq!(evs[0].agent.as_deref(), Some("caas"));
    assert_eq!(evs[0].room.as_deref(), Some("protocol"));
    assert_eq!(evs[0].detail["msg_id"], 7);
    assert_eq!(evs[0].detail["delivered_to"][0], "dashboard");
}

#[tokio::test]
async fn nullable_agent_and_room_round_trip_as_none() {
    let (_d, store) = temp_store().await;
    store.append_event("bus_started", None, None, json!({})).await.unwrap();

    let evs = store.events(10).await.unwrap();
    assert!(evs[0].agent.is_none());
    assert!(evs[0].room.is_none());
}

#[tokio::test]
async fn events_returns_most_recent_first() {
    let (_d, store) = temp_store().await;
    for i in 0..3 {
        store
            .append_event("ack", Some("caas"), Some("r"), json!({"n": i}))
            .await
            .unwrap();
    }
    let evs = store.events(10).await.unwrap();
    assert_eq!(evs[0].detail["n"], 2, "newest first");
    assert_eq!(evs[2].detail["n"], 0);
}

#[tokio::test]
async fn events_for_room_returns_oldest_first_for_interleaving() {
    // The transcript view merges these with messages in chronological order, so this
    // query must hand them over in the order they happened, unlike the others.
    let (_d, store) = temp_store().await;
    for i in 0..3 {
        store
            .append_event("message_sent", Some("caas"), Some("protocol"), json!({"n": i}))
            .await
            .unwrap();
    }
    store.append_event("message_sent", Some("caas"), Some("other"), json!({"n": 99})).await.unwrap();

    let evs = store.events_for_room("protocol", 10).await.unwrap();
    assert_eq!(evs.len(), 3, "only this room's events");
    assert_eq!(evs[0].detail["n"], 0, "oldest first");
    assert_eq!(evs[2].detail["n"], 2);
}

#[tokio::test]
async fn events_filter_by_agent_and_by_kind() {
    let (_d, store) = temp_store().await;
    store.append_event("ack", Some("caas"), Some("r"), json!({})).await.unwrap();
    store.append_event("room_paused", Some("caas"), Some("r"), json!({})).await.unwrap();
    store.append_event("ack", Some("dashboard"), Some("r"), json!({})).await.unwrap();

    assert_eq!(store.events_for_agent("caas", 10).await.unwrap().len(), 2);
    assert_eq!(store.events_of_kind("ack", 10).await.unwrap().len(), 2);
}

#[tokio::test]
async fn limit_is_respected() {
    let (_d, store) = temp_store().await;
    for i in 0..5 {
        store.append_event("ack", Some("caas"), Some("r"), json!({"n": i})).await.unwrap();
    }
    assert_eq!(store.events(2).await.unwrap().len(), 2);
}

#[tokio::test]
async fn malformed_detail_json_does_not_poison_reads() {
    // detail_json is TEXT; a bad row should degrade to Null rather than failing the
    // whole query and hiding every other event on the page.
    let (_d, store) = temp_store().await;
    store.append_event("ack", Some("caas"), Some("r"), json!({"ok": true})).await.unwrap();
    sqlx::query("INSERT INTO events (created_at, kind, agent, room, detail_json) VALUES (1, 'bad', 'x', 'r', 'not json')")
        .execute(store.pool_for_test())
        .await
        .unwrap();

    let evs = store.events(10).await.unwrap();
    assert_eq!(evs.len(), 2, "the bad row must not sink the query");
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --test events`
Expected: FAIL — `no method named append_event`.

- [ ] **Step 4: Implement the event store**

Create `src/store/events.rs`:

```rust
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
```

Add to the top of `src/store/mod.rs`:

```rust
mod events;
pub use events::EventRow;
```

The malformed-JSON test needs pool access. Add to `impl Store` in `src/store/mod.rs`:

```rust
/// Test-only accessor. Production code goes through the typed methods.
#[doc(hidden)]
pub fn pool_for_test(&self) -> &SqlitePool {
    &self.pool
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --test events`
Expected: PASS, 7 tests.

- [ ] **Step 6: Format and commit**

```bash
cargo +nightly fmt
git add schema.sql src/store/events.rs src/store/mod.rs tests/events.rs
git commit -m "feat: event log storage"
```

---

### Task 2: Move command handling out of `src/bus/mod.rs`

**Files:**
- Create: `src/bus/commands.rs`
- Modify: `src/bus/mod.rs`

**Interfaces:**
- Consumes: everything `handle()` already uses.
- Produces: `pub(crate) async fn handle(app: &App, me: &str, cmd: ToBus, ctrl: &Sender)` in `crate::bus::commands`, with `App` and its fields made `pub(crate)` so the new module can reach them.

**This is a pure move. No behaviour changes, no logic edits, no renames beyond visibility.**
`src/bus/mod.rs` is 890 lines holding the WebSocket connection loop, the command handler,
and HTTP routes. Two prior reviews flagged it at 496 lines and recommended this exact
split. Task 3 adds roughly ten event write sites into `handle()`; doing that in an
890-line file makes the diff unreadable.

- [ ] **Step 1: Confirm the current baseline**

Run: `cargo test`
Expected: PASS. Record the exact number — every later step must match it.

- [ ] **Step 2: Move `handle` and its helpers**

Create `src/bus/commands.rs`. Move `handle()` verbatim, plus any helper functions used
only by it (`known_rooms`, and the reply builders if present). Add the imports it needs at
the top of the new file. In `src/bus/mod.rs`, add `pub(crate) mod commands;` and delete the
moved code.

Make `App` and its fields `pub(crate)` so `commands.rs` can use them. Change nothing else.

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: PASS with the **same count** as Step 1. A pure move that changes the number has
changed behaviour.

Run: `cargo clippy --all-targets` — expected clean.

- [ ] **Step 4: Verify the split is real**

```bash
wc -l src/bus/mod.rs src/bus/commands.rs
```
Expected: `mod.rs` substantially smaller, `commands.rs` holding the bulk of `handle`.

- [ ] **Step 5: Format and commit**

```bash
cargo +nightly fmt
git add src/bus/mod.rs src/bus/commands.rs
git commit -m "refactor: move command handling into src/bus/commands.rs"
```

---

### Task 3: Write events from the bus

**Files:**
- Modify: `src/bus/commands.rs`, `src/bus/mod.rs`
- Test: `tests/events.rs` (append)

**Interfaces:**
- Consumes: `Store::append_event` from Task 1; `handle()` in `commands.rs` from Task 2.
- Produces: no new public API. Later tasks read the rows this writes.

Every write follows one shape, and it matters:

```rust
// A logging failure must never fail the operation being logged.
let _ = app.store.append_event("message_sent", Some(me), Some(&room), json!({ .. })).await;
```

- [ ] **Step 1: Write the failing tests**

Append to `tests/events.rs`. These drive the real bus over WebSocket, so copy the
`start_bus` / `connect` / `send` / `next_event` helpers from `tests/bus.rs` — or make that
file's helpers `pub` in a shared test module if that is cleaner.

```rust
#[tokio::test]
async fn a_send_records_delivery_outcome_per_recipient() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await; // Registered

    send(&mut a, &ToBus::Send {
        req_id: 1,
        target: Target::Agent { name: "ghost".into() },
        text: "hello".into(),
        done: false,
    }).await;
    next_event(&mut a).await; // Sent reply

    let store = Store::open(&store_dir).await.unwrap();
    let sent: Vec<_> = store.events_of_kind("message_sent", 10).await.unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].detail["queued_for"][0], "ghost", "recipient was offline");
    assert_eq!(sent[0].detail["delivered_to"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn registration_records_both_requested_and_effective_name() {
    // The caas -> caas#2 collision must be visible in the log rather than inferred.
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    let mut b = connect(port, "caas").await; // same name, same host
    next_event(&mut b).await;

    let store = Store::open(&store_dir).await.unwrap();
    let regs = store.events_of_kind("agent_registered", 10).await.unwrap();
    assert_eq!(regs.len(), 2);
    let collided = regs.iter().find(|e| e.detail["effective_name"] != e.detail["requested_name"]);
    let collided = collided.expect("the second registration should differ");
    assert_eq!(collided.detail["requested_name"], "caas");
    assert_eq!(collided.detail["effective_name"], "caas#2");
}

#[tokio::test]
async fn an_ack_is_recorded() {
    // The Ack defect was invisible precisely because nothing recorded acks.
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    send(&mut a, &ToBus::Ack { room: "protocol".into(), last_delivered_id: 5 }).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let store = Store::open(&store_dir).await.unwrap();
    let acks = store.events_of_kind("ack", 10).await.unwrap();
    assert_eq!(acks.len(), 1);
    assert_eq!(acks[0].detail["last_delivered_id"], 5);
}

#[tokio::test]
async fn a_paused_room_is_recorded() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;

    for i in 0..21 {
        send(&mut a, &ToBus::Send {
            req_id: 100 + i,
            target: Target::Room { room: "loop".into() },
            text: format!("m{i}"),
            done: false,
        }).await;
        next_event(&mut a).await;
    }

    let store = Store::open(&store_dir).await.unwrap();
    let paused = store.events_of_kind("room_paused", 10).await.unwrap();
    assert_eq!(paused.len(), 1);
    assert_eq!(paused[0].room.as_deref(), Some("loop"));
}
```

`start_bus_with_dir` is `start_bus` returning the temp dir path as well, so a test can
open a second `Store` against the same database and read what the bus wrote.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test events`
Expected: FAIL — the new tests find zero events.

- [ ] **Step 3: Add the write sites**

In `src/bus/commands.rs`, in the `ToBus::Send` arm after the fan-out computes
`delivered_to` and `queued_for`:

```rust
let _ = app
    .store
    .append_event(
        "message_sent",
        Some(me),
        Some(&room),
        json!({
            "msg_id": msg_id,
            "delivered_to": delivered_to,
            "queued_for": queued_for,
            "done": done,
        }),
    )
    .await;
```

In the `GuardVerdict::Paused` branch:

```rust
let _ = app
    .store
    .append_event("room_paused", Some(me), Some(&room), json!({ "count": count }))
    .await;
```

In the `GuardVerdict::RateLimited` branch:

```rust
let _ = app
    .store
    .append_event("rate_limited", Some(me), Some(&room), json!({ "retry_in_ms": retry_in_ms }))
    .await;
```

In the `ToBus::Ack` arm:

```rust
let _ = app
    .store
    .append_event("ack", Some(me), Some(&room), json!({ "last_delivered_id": last_delivered_id }))
    .await;
```

In the `ToBus::Join` arm, after a successful join:

```rust
let _ = app.store.append_event("room_joined", Some(me), Some(&room), json!({})).await;
```

In the `ToBus::Resume` arm:

```rust
let _ = app.store.append_event("resumed", Some(me), Some(&room), json!({})).await;
```

In the `ToBus::PutFile` arm on success:

```rust
let _ = app
    .store
    .append_event("file_stored", Some(me), Some(&room),
                  json!({ "key": f.key, "size": f.size, "sha256": f.sha256 }))
    .await;
```

In the `ToBus::GetFile` arm on success:

```rust
let _ = app.store.append_event("file_fetched", Some(me), Some(&room), json!({ "key": key })).await;
```

In `src/bus/mod.rs`, in the `Register` handler after `attach` returns the effective name:

```rust
let _ = app
    .store
    .append_event(
        "agent_registered",
        Some(&effective),
        None,
        json!({
            "requested_name": name,
            "effective_name": effective,
            "host": host,
            "session_id": session_id,
        }),
    )
    .await;
```

In the teardown block, alongside `detach` and `set_online(false)`:

```rust
let _ = app
    .store
    .append_event("agent_disconnected", Some(&name), None, json!({ "reason": reason }))
    .await;
```

`reason` must distinguish a closed socket from a keepalive timeout — the teardown already
knows which path it took. Pass `"socket_closed"` or `"keepalive_timeout"` accordingly; a
ghost agent is only diagnosable if the log says which happened.

Add `use serde_json::json;` where needed.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test events`
Expected: PASS.

Run: `cargo test`
Expected: all prior tests still pass.

- [ ] **Step 5: Verify a logging failure cannot fail an operation**

Every call site uses `let _ = ...`. Confirm by grep that none propagates:

```bash
grep -n "append_event" src/bus/*.rs | grep -v "let _ ="
```
Expected: no output. Any hit is a call that could fail a real operation because logging
broke.

- [ ] **Step 6: Format and commit**

```bash
cargo +nightly fmt
git add src/bus/commands.rs src/bus/mod.rs tests/events.rs
git commit -m "feat: record what the bus does to the event log"
```

---

### Task 4: HTML escaping and page scaffolding

**Files:**
- Create: `src/web/html.rs`, `src/web/mod.rs`
- Modify: `src/lib.rs`, `src/bus/mod.rs`
- Test: unit tests in `src/web/html.rs`; `tests/web.rs`

**Interfaces:**
- Consumes: `Store`.
- Produces:
  - `web::html::esc(s: &str) -> String`
  - `web::html::page(title: &str, body: &str) -> String`
  - `pub fn routes() -> axum::Router<crate::bus::App>` mounting `GET /` and returning HTML
  - `App` must be `Clone` and reachable — already true from Task 2.

**Escaping is the security requirement of this whole project.** Message bodies are model
output. An agent that writes `<script>alert(1)</script>` into a message would have it
execute in your browser when you open the transcript. Agent names, room names, and file
keys reach the same pages. There is no framework auto-escaping here — every interpolation
is manual, so the helper must be used everywhere and tested directly.

- [ ] **Step 1: Write the failing tests**

Create `src/web/html.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_every_character_that_can_break_out_of_html() {
        assert_eq!(esc("<script>"), "&lt;script&gt;");
        assert_eq!(esc("a & b"), "a &amp; b");
        assert_eq!(esc("\"quoted\""), "&quot;quoted&quot;");
        assert_eq!(esc("it's"), "it&#39;s");
    }

    #[test]
    fn a_message_body_with_a_script_tag_is_inert() {
        // The realistic attack: an agent writes this into a message and a human opens
        // the transcript.
        let body = "<script>alert('pwned')</script>";
        let out = esc(body);
        assert!(!out.contains("<script"), "must not survive as a tag: {out}");
        assert!(out.contains("&lt;script&gt;"));
    }

    #[test]
    fn ampersand_is_escaped_first_so_entities_are_not_doubled() {
        // Escaping < before & would turn "<" into "&lt;" and then into "&amp;lt;".
        assert_eq!(esc("<"), "&lt;");
        assert_eq!(esc("&lt;"), "&amp;lt;");
    }

    #[test]
    fn page_wraps_body_and_escapes_the_title() {
        let out = page("a <b> title", "<p>hello</p>");
        assert!(out.starts_with("<!doctype html>"));
        assert!(out.contains("a &lt;b&gt; title"), "title must be escaped");
        assert!(out.contains("<p>hello</p>"), "body is pre-rendered and passed through");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib web::html`
Expected: FAIL — `cannot find function esc`.

- [ ] **Step 3: Implement**

Prepend to `src/web/html.rs`:

```rust
//! HTML building. There is no template engine here, so escaping is manual and
//! `esc` must wrap every interpolated value.
//!
//! This matters more than it looks: message bodies are model output, and agent names,
//! room names, and file keys are all attacker-influencable in the sense that an agent
//! can choose them. Rendering any of them raw is self-inflicted XSS in your own browser.

/// Escape text for interpolation into HTML element content or a quoted attribute.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            // `&` first: escaping `<` before `&` would produce `&amp;lt;`.
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Wrap a pre-rendered body in the shared page chrome. `body` is passed through
/// verbatim; it is the caller's job to have escaped its parts.
pub fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{t} · claude-bus</title><style>{CSS}</style></head>\
         <body><nav><a href=\"/\">overview</a> <a href=\"/rooms\">rooms</a> \
         <a href=\"/agents\">agents</a> <a href=\"/events\">events</a></nav>\
         <main>{body}</main></body></html>",
        t = esc(title),
    )
}

const CSS: &str = "\
body{font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;margin:0;background:#111;color:#ddd}\
nav{padding:.6rem 1rem;background:#000;border-bottom:1px solid #333}\
nav a{color:#8ab4f8;margin-right:1rem;text-decoration:none}\
main{padding:1rem;max-width:60rem}\
table{border-collapse:collapse;width:100%}\
td,th{text-align:left;padding:.3rem .6rem;border-bottom:1px solid #222;vertical-align:top}\
th{color:#888;font-weight:normal}\
a{color:#8ab4f8}\
.msg{white-space:pre-wrap}\
.ev{color:#888;font-style:italic}\
.off{color:#666}\
";
```

Create `src/web/mod.rs`:

```rust
//! Read-only web views over the bus's own data. Performs no writes: it cannot be the
//! cause of a bug it is being used to investigate, and with no authentication on the
//! bus, anything this could do would be available to anything that can reach the port.

pub mod html;

use axum::extract::State;
use axum::response::Html;
use axum::{Router, routing::get};

use crate::bus::App;
use html::{esc, page};

pub fn routes() -> Router<App> {
    Router::new().route("/", get(overview))
}

async fn overview(State(app): State<App>) -> Html<String> {
    let agents = app.store.agents().await.unwrap_or_default();
    let rooms = app.store.rooms().await.unwrap_or_default();
    let events = app.store.events(20).await.unwrap_or_default();

    let mut b = String::new();
    b.push_str("<h1>overview</h1><h2>agents</h2><table>");
    for a in &agents {
        b.push_str(&format!(
            "<tr><td><a href=\"/agents/{n}\">{n}</a></td><td>{h}</td><td class=\"{c}\">{s}</td></tr>",
            n = esc(&a.name),
            h = esc(&a.host),
            c = if a.online { "" } else { "off" },
            s = if a.online { "online" } else { "offline" },
        ));
    }
    b.push_str("</table><h2>rooms</h2><table>");
    for r in &rooms {
        b.push_str(&format!(
            "<tr><td><a href=\"/rooms/{n}\">{n}</a></td><td>{m}</td></tr>",
            n = esc(&r.name),
            m = esc(&r.members.join(", ")),
        ));
    }
    b.push_str("</table><h2>recent events</h2><table>");
    for e in &events {
        b.push_str(&format!(
            "<tr><td>{k}</td><td>{a}</td><td>{r}</td></tr>",
            k = esc(&e.kind),
            a = esc(e.agent.as_deref().unwrap_or("")),
            r = esc(e.room.as_deref().unwrap_or("")),
        ));
    }
    b.push_str("</table>");
    Html(page("overview", &b))
}
```

Add `pub mod web;` to `src/lib.rs`. In `src/bus/mod.rs`'s `serve_on_with`, merge the
routes:

```rust
let router = Router::new()
    .route("/ws", get(upgrade))
    .route("/human-active", axum::routing::post(human_active))
    .merge(crate::web::routes())
    .with_state(app);
```

- [ ] **Step 4: Write the handler test**

Create `tests/web.rs`:

```rust
// Drives the real server over HTTP against a temp SQLite. Asserts rendered content,
// not status codes: a page that returns 200 with an empty table has failed at its
// only job.
use claude_bus::store::Store;

async fn start(dir: &std::path::Path) -> u16 {
    let path = dir.to_path_buf();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { claude_bus::bus::serve_on(listener, path).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    port
}

async fn get(port: u16, path: &str) -> String {
    let url = format!("http://127.0.0.1:{port}{path}");
    // Minimal HTTP/1.1 GET so the test needs no HTTP client dependency.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let _ = url;
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    buf
}

#[tokio::test]
async fn overview_lists_agents_and_rooms() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.upsert_agent("caas", "lisa", "/w/caas", None).await.unwrap();
        store.join_room("protocol", "caas").await.unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/").await;
    assert!(body.contains("caas"), "agent must appear: {body}");
    assert!(body.contains("protocol"), "room must appear");
}

#[tokio::test]
async fn a_script_tag_in_a_room_name_is_escaped() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room("<script>alert(1)</script>", "caas").await.unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/").await;
    assert!(!body.contains("<script>alert(1)</script>"), "raw tag must not survive");
    assert!(body.contains("&lt;script&gt;"), "must be escaped instead");
}
```

- [ ] **Step 5: Run and verify**

Run: `cargo test --lib web::html && cargo test --test web`
Expected: PASS.

- [ ] **Step 6: Format and commit**

```bash
cargo +nightly fmt
git add src/web/ src/lib.rs src/bus/mod.rs tests/web.rs
git commit -m "feat: web scaffolding, HTML escaping, and the overview page"
```

---

### Task 5: The room transcript

**Files:**
- Modify: `src/web/mod.rs`
- Test: `tests/web.rs` (append)

**Interfaces:**
- Consumes: `Store::history`, `Store::events_for_room` (oldest-first), `html::{esc, page}`.
- Produces: `GET /rooms` and `GET /rooms/{name}`.

**This is the page that does not exist today in any form.** `tail` shows a live room to
whoever is watching; this shows what happened, afterwards, with the bus's own behaviour
interleaved against it. The interleaving is the entire value — get the ordering wrong and
the page is decorative.

- [ ] **Step 1: Write the failing test**

Append to `tests/web.rs`:

```rust
#[tokio::test]
async fn a_transcript_interleaves_messages_and_events_in_time_order() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room("protocol", "caas").await.unwrap();
        store.append_message("protocol", "caas", "FIRST_MESSAGE", false).await.unwrap();
        store
            .append_event("room_paused", Some("caas"), Some("protocol"),
                          serde_json::json!({"count": 20}))
            .await
            .unwrap();
        store.append_message("protocol", "caas", "LAST_MESSAGE", false).await.unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/rooms/protocol").await;
    let first = body.find("FIRST_MESSAGE").expect("first message rendered");
    let pause = body.find("room_paused").expect("the pause event is shown inline");
    let last = body.find("LAST_MESSAGE").expect("last message rendered");

    assert!(first < pause, "the pause must appear after the first message");
    assert!(pause < last, "and before the last — chronological, not grouped by type");
}

#[tokio::test]
async fn a_script_tag_in_a_message_body_is_escaped() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room("protocol", "caas").await.unwrap();
        store
            .append_message("protocol", "caas", "<script>alert('xss')</script>", false)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/rooms/protocol").await;
    assert!(!body.contains("<script>alert"), "an agent must not be able to inject script");
    assert!(body.contains("&lt;script&gt;"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test web`
Expected: FAIL — 404, no transcript rendered.

- [ ] **Step 3: Implement**

Add to `src/web/mod.rs`:

```rust
/// One row of a transcript, from either source, sorted into a single timeline.
enum Entry {
    Message { at: i64, from: String, body: String },
    Event { at: i64, kind: String, detail: String },
}

impl Entry {
    fn at(&self) -> i64 {
        match self {
            Entry::Message { at, .. } | Entry::Event { at, .. } => *at,
        }
    }
}

async fn room(State(app): State<App>, Path(name): Path<String>) -> Html<String> {
    let msgs = app.store.history(&name, 500).await.unwrap_or_default();
    let evs = app.store.events_for_room(&name, 500).await.unwrap_or_default();

    let mut entries: Vec<Entry> = Vec::with_capacity(msgs.len() + evs.len());
    for m in msgs {
        entries.push(Entry::Message { at: m.created_at, from: m.from_agent, body: m.body });
    }
    for e in evs {
        entries.push(Entry::Event { at: e.created_at, kind: e.kind, detail: e.detail.to_string() });
    }
    // The whole point of the page: one timeline, not two lists.
    entries.sort_by_key(|e| e.at());

    let mut b = format!("<h1>{}</h1><table>", esc(&name));
    for e in &entries {
        match e {
            Entry::Message { from, body, .. } => b.push_str(&format!(
                "<tr><td>{f}</td><td class=\"msg\">{t}</td></tr>",
                f = esc(from),
                t = esc(body),
            )),
            Entry::Event { kind, detail, .. } => b.push_str(&format!(
                "<tr><td class=\"ev\">{k}</td><td class=\"ev\">{d}</td></tr>",
                k = esc(kind),
                d = esc(detail),
            )),
        }
    }
    b.push_str("</table>");
    Html(page(&name, &b))
}

async fn rooms(State(app): State<App>) -> Html<String> {
    let rooms = app.store.rooms().await.unwrap_or_default();
    let mut b = String::from("<h1>rooms</h1><table>");
    for r in &rooms {
        b.push_str(&format!(
            "<tr><td><a href=\"/rooms/{n}\">{n}</a></td><td>{m}</td></tr>",
            n = esc(&r.name),
            m = esc(&r.members.join(", ")),
        ));
    }
    b.push_str("</table>");
    Html(page("rooms", &b))
}
```

Add `use axum::extract::Path;` and extend `routes()`:

```rust
Router::new()
    .route("/", get(overview))
    .route("/rooms", get(rooms))
    .route("/rooms/{name}", get(room))
```

- [ ] **Step 4: Run and verify**

Run: `cargo test --test web`
Expected: PASS.

- [ ] **Step 5: Format and commit**

```bash
cargo +nightly fmt
git add src/web/mod.rs tests/web.rs
git commit -m "feat: room transcript interleaving messages and bus events"
```

---

### Task 6: Agents, files, and the raw event log

**Files:**
- Modify: `src/web/mod.rs`
- Test: `tests/web.rs` (append)

**Interfaces:**
- Consumes: `Store::agents`, `Store::rooms`, `Store::events_for_agent`, `Store::list_files`, `Store::events`, `Store::events_of_kind`.
- Produces: `GET /agents`, `GET /agents/{name}`, `GET /rooms/{name}/files`, `GET /events`, `GET /events?kind=`.

- [ ] **Step 1: Write the failing tests**

Append to `tests/web.rs`:

```rust
#[tokio::test]
async fn an_agent_page_shows_its_registration_history() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.upsert_agent("caas", "lisa", "/w/caas", None).await.unwrap();
        store
            .append_event("agent_registered", Some("caas"), None,
                serde_json::json!({"requested_name":"caas","effective_name":"caas","host":"lisa"}))
            .await
            .unwrap();
        store
            .append_event("agent_disconnected", Some("caas"), None,
                serde_json::json!({"reason":"keepalive_timeout"}))
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/agents/caas").await;
    assert!(body.contains("agent_registered"));
    assert!(body.contains("keepalive_timeout"), "the disconnect reason is the diagnostic");
}

#[tokio::test]
async fn a_files_page_lists_artifacts_with_uploader_and_size() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.join_room("protocol", "caas").await.unwrap();
        store.put_file("protocol", "schema.json", b"{\"a\":1}", None, "caas").await.unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/rooms/protocol/files").await;
    assert!(body.contains("schema.json"));
    assert!(body.contains("caas"), "uploader must be shown");
}

#[tokio::test]
async fn the_event_log_filters_by_kind() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.append_event("ack", Some("caas"), Some("r"), serde_json::json!({})).await.unwrap();
        store.append_event("room_paused", Some("caas"), Some("r"), serde_json::json!({})).await.unwrap();
    }
    let port = start(dir.path()).await;

    let all = get(port, "/events").await;
    assert!(all.contains("ack") && all.contains("room_paused"));

    let filtered = get(port, "/events?kind=room_paused").await;
    assert!(filtered.contains("room_paused"));
    assert!(!filtered.contains(">ack<"), "the filter must actually exclude other kinds");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test web`
Expected: FAIL — 404 on the new routes.

- [ ] **Step 3: Implement**

Add to `src/web/mod.rs`:

```rust
use axum::extract::Query;
use std::collections::HashMap;

async fn agents(State(app): State<App>) -> Html<String> {
    let agents = app.store.agents().await.unwrap_or_default();
    let mut b = String::from("<h1>agents</h1><table><tr><th>name<th>host<th>state</tr>");
    for a in &agents {
        b.push_str(&format!(
            "<tr><td><a href=\"/agents/{n}\">{n}</a></td><td>{h}</td><td class=\"{c}\">{s}</td></tr>",
            n = esc(&a.name),
            h = esc(&a.host),
            c = if a.online { "" } else { "off" },
            s = if a.online { "online" } else { "offline" },
        ));
    }
    b.push_str("</table>");
    Html(page("agents", &b))
}

async fn agent(State(app): State<App>, Path(name): Path<String>) -> Html<String> {
    let rooms = app.store.rooms().await.unwrap_or_default();
    let mine: Vec<&str> = rooms
        .iter()
        .filter(|r| r.members.iter().any(|m| *m == name))
        .map(|r| r.name.as_str())
        .collect();
    let evs = app.store.events_for_agent(&name, 200).await.unwrap_or_default();

    let mut b = format!("<h1>{}</h1><h2>rooms</h2><ul>", esc(&name));
    for r in &mine {
        b.push_str(&format!("<li><a href=\"/rooms/{r}\">{r}</a></li>", r = esc(r)));
    }
    b.push_str("</ul><h2>activity</h2><table><tr><th>kind<th>room<th>detail</tr>");
    for e in &evs {
        b.push_str(&format!(
            "<tr><td>{k}</td><td>{r}</td><td>{d}</td></tr>",
            k = esc(&e.kind),
            r = esc(e.room.as_deref().unwrap_or("")),
            d = esc(&e.detail.to_string()),
        ));
    }
    b.push_str("</table>");
    Html(page(&name, &b))
}

async fn files(State(app): State<App>, Path(name): Path<String>) -> Html<String> {
    let files = app.store.list_files(&name).await.unwrap_or_default();
    let mut b = format!(
        "<h1>{} · files</h1><table><tr><th>key<th>size<th>by<th>sha256</tr>",
        esc(&name)
    );
    for f in &files {
        b.push_str(&format!(
            "<tr><td>{k}</td><td>{s}</td><td>{u}</td><td>{h}</td></tr>",
            k = esc(&f.key),
            s = f.size,
            u = esc(&f.updated_by),
            h = esc(&f.sha256[..16.min(f.sha256.len())]),
        ));
    }
    b.push_str("</table>");
    Html(page(&name, &b))
}

async fn events_page(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> Html<String> {
    let evs = match q.get("kind") {
        Some(k) => app.store.events_of_kind(k, 500).await.unwrap_or_default(),
        None => app.store.events(500).await.unwrap_or_default(),
    };
    let mut b = String::from("<h1>events</h1><table><tr><th>kind<th>agent<th>room<th>detail</tr>");
    for e in &evs {
        b.push_str(&format!(
            "<tr><td><a href=\"/events?kind={k}\">{k}</a></td><td>{a}</td><td>{r}</td><td>{d}</td></tr>",
            k = esc(&e.kind),
            a = esc(e.agent.as_deref().unwrap_or("")),
            r = esc(e.room.as_deref().unwrap_or("")),
            d = esc(&e.detail.to_string()),
        ));
    }
    b.push_str("</table>");
    Html(page("events", &b))
}
```

Extend `routes()`:

```rust
.route("/agents", get(agents))
.route("/agents/{name}", get(agent))
.route("/rooms/{name}/files", get(files))
.route("/events", get(events_page))
```

- [ ] **Step 4: Run and verify**

Run: `cargo test --test web && cargo test`
Expected: PASS, everything.

- [ ] **Step 5: Format and commit**

```bash
cargo +nightly fmt
git add src/web/mod.rs tests/web.rs
git commit -m "feat: agent, files, and event log pages"
```

---

### Task 7: Documentation

**Files:**
- Modify: `docs/DEPLOY.md`, `README.md`

- [ ] **Step 1: Document the UI in `docs/DEPLOY.md`**

Add after the "Watching a conversation" section:

````markdown
## Reading the record afterwards

The bus serves a read-only web UI on the same port:

```
http://nas.lan:7777/
```

`claude-bus tail` shows one room live, to whoever happens to be watching. This shows what
happened afterwards — transcripts with the bus's own behaviour interleaved against them,
so you can see not just what two agents said but whether each message was delivered or
merely queued, when a room hit the exchange cap, and why an agent went offline.

Pages: overview, rooms and their transcripts, agents and their connect/disconnect history,
artifacts per room, and the raw event log filterable by kind.

It performs no writes. With no authentication on the bus, anything the UI could do would
be available to anything that can reach the port — so it does nothing.

Events accumulate with no retention policy. At LAN volumes that is fine for a long time,
but nothing prunes them.
````

- [ ] **Step 2: Mention it in `README.md`**

Add to the subcommand list:

```markdown
The bus also serves a read-only web UI on its own port for reading conversations and bus
behaviour after the fact — see `docs/DEPLOY.md`.
```

- [ ] **Step 3: Verify and commit**

Run: `cargo test` — expected PASS.

```bash
git add docs/DEPLOY.md README.md
git commit -m "docs: describe the observability UI"
```

---

## Self-Review

**Spec coverage.** Walked each spec section against the tasks:

| Spec section | Task |
| --- | --- |
| Event log table and indexes | 1 |
| Every event kind in the spec's table | 3 |
| Write discipline: never fail the operation | 3 (Step 5 greps for it) |
| Write discipline: no lock, no control-flow change | 3 (call sites sit beside existing awaits) |
| Same binary, same port, routes on the existing server | 4 |
| Server-rendered, no SPA, no new deps | 4 (hand-rolled HTML, `cargo add` unused) |
| Overview page | 4 |
| Room transcript with events inline | 5 |
| Agents, agent detail, files, events pages | 6 |
| Read-only | All — no `POST`/`PUT`/`DELETE` route anywhere |
| Testing: event writes fire, handlers assert content, interleaving ordering | 1, 3, 5 |
| Docs | 7 |

Two things the spec did not call out that the plan adds: **HTML escaping** as a
first-class requirement with its own tests (Task 4), because message bodies are model
output rendered into a browser; and the **`src/bus/mod.rs` split** (Task 2), because the
file has grown to 890 lines and this plan adds ten write sites to it.

Not covered by design, per the spec's own out-of-scope list: retention, full-text search,
authentication, and any write path.

**Placeholder scan.** No TBD/TODO, no "add error handling", no "similar to Task N". Every
code step carries the actual code.

**Type consistency.** `EventRow`'s fields are used identically in Tasks 4–6 as defined in
Task 1. `esc`/`page` signatures match across every use. `Store` method names match the
existing codebase (`history`, `rooms`, `agents`, `list_files`) and Task 1's additions
(`events`, `events_for_room`, `events_for_agent`, `events_of_kind`, `append_event`).
`App` is made `pub(crate)` in Task 2 and consumed as `axum::extract::State<App>` from Task
4 onward.

**One risk carried forward.** Task 3's `agent_disconnected` requires the teardown to know
whether it exited via a closed socket or a keepalive timeout. The code has both paths but
currently converges before the event would be written; the implementer must thread the
reason through rather than logging a constant. A single hardcoded reason would make the
ghost-agent case — the thing this event exists to diagnose — indistinguishable from an
ordinary disconnect.
