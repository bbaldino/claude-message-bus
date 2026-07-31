# Human Participant Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a human join a bus room under their own name, send and receive in realtime from a terminal, and have the bus's runaway guards recognise them as the human-input signal those guards were built to detect.

**Architecture:** A human is an ordinary agent carrying one boolean. `ToBus::Register` gains a `#[serde(default)] human: bool`; the `agents` table gains `is_human`. Room membership for a human is ephemeral (created on join, dropped on disconnect) while the `agents` row persists. A new `claude-bus chat <room>` subcommand mirrors `tail`'s WebSocket loop but registers instead of observing and reads stdin.

**Tech Stack:** Rust, tokio, axum, sqlx (SQLite, runtime queries not macros), tokio-tungstenite, serde.

## Global Constraints

- **Backward compatibility is non-negotiable.** `Register`'s new field MUST be `#[serde(default)]`. Claude Code does not respawn stdio MCP servers mid-session, so every agent binary currently running must keep working against the new bus without a redeploy. A test asserts this directly.
- **The migration must be idempotent by construction** — `PRAGMA table_info(agents)` then conditional `ALTER TABLE`. Never by swallowing an error whose message could change.
- **Two lifetimes:** the `agents` row persists; `room_members` for a human is ephemeral.
- Every event write uses `let _ = app.store.append_event(...)` — a logging failure must never fail the operation being logged.
- `Guards::check` is called exactly once per send, before `append_message`, with rejection paths returning first.
- A connection's own replies go via `control_tx`; fan-out to others via `app.registry`. Never blur these.
- The web layer stays read-only: no `POST`/`PUT`/`DELETE`, no store writes.
- Rust formatting: `cargo +nightly fmt` (nightly specifically). `cargo clippy --all-targets` must end clean.
- Only capitalize the first letter of multi-letter acronyms (`RagService`, not `RAGService`).
- No new crate dependencies.
- Baseline before Task 1: **205 tests passing**. Every task must leave the suite green.

---

## File Structure

| File | Responsibility | Tasks |
| --- | --- | --- |
| `schema.sql` | The `is_human` column on a fresh database | 1 |
| `src/store/mod.rs` | Migration, `AgentRow.is_human`, `upsert_agent`, `leave_all_rooms` | 1, 4 |
| `src/proto.rs` | `Register.human` | 2 |
| `src/bus/mod.rs` | Threading `human` from Register to the store, registry, and teardown | 3, 4 |
| `src/bus/delivery.rs` | `Guards::check` human path | 5 |
| `src/bus/commands.rs` | Passing `is_human` into the guard call | 5 |
| `src/chat.rs` (new) | The `claude-bus chat` client | 6 |
| `src/main.rs` | The `chat` subcommand | 6 |
| `src/web/mod.rs` | The human marker on `/` and `/agents` | 7 |
| `docs/DEPLOY.md` | Documenting `chat` and the guard behaviour | 7 |

---

### Task 1: The `is_human` column and its migration

**Files:**
- Modify: `schema.sql`, `src/store/mod.rs`
- Test: `tests/store.rs` (append)

**Interfaces:**
- Produces: `Store::migrate()` (called from `Store::open`), `AgentRow.is_human: bool`, and `upsert_agent(name, host, cwd, session_id, is_human)`.

This is the project's first migration against a live database. `schema.sql` is entirely `CREATE TABLE IF NOT EXISTS`, so a fresh database gets the column from the table definition, while an existing one — the deployed container's named volume — needs `ALTER TABLE`. SQLite has no `ADD COLUMN IF NOT EXISTS`, so the guard is a `PRAGMA table_info` check.

- [ ] **Step 1: Write the failing tests**

Append to `tests/store.rs`:

```rust
#[tokio::test]
async fn a_fresh_database_has_the_is_human_column() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store
        .upsert_agent("bbaldino", "hardac", "/w", None, true)
        .await
        .unwrap();
    let agents = store.agents().await.unwrap();
    let me = agents.iter().find(|a| a.name == "bbaldino").unwrap();
    assert!(me.is_human, "the flag must round-trip through the store");
}

#[tokio::test]
async fn an_agent_defaults_to_not_human() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store
        .upsert_agent("caas", "hardac", "/w", None, false)
        .await
        .unwrap();
    let agents = store.agents().await.unwrap();
    assert!(!agents.iter().find(|a| a.name == "caas").unwrap().is_human);
}

#[tokio::test]
async fn the_migration_adds_the_column_to_a_database_that_predates_it() {
    // The real case: the deployed bus's volume already holds an `agents` table
    // without this column. Simulate it by creating the old shape by hand, then
    // opening a Store over it the way a freshly deployed binary would.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bus.db");
    {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=rwc", db.display()))
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE agents (
               name TEXT PRIMARY KEY, host TEXT NOT NULL, cwd TEXT NOT NULL,
               session_id TEXT, connected_at INTEGER NOT NULL,
               last_seen INTEGER NOT NULL, online INTEGER NOT NULL DEFAULT 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agents (name, host, cwd, connected_at, last_seen, online)
             VALUES ('caas', 'hardac', '/w', 1, 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    }

    let store = Store::open(dir.path()).await.unwrap();

    let agents = store.agents().await.unwrap();
    let caas = agents.iter().find(|a| a.name == "caas").unwrap();
    assert!(!caas.is_human, "a pre-existing row defaults to not human");
    assert_eq!(caas.host, "hardac", "existing data must survive the migration");
}

#[tokio::test]
async fn the_migration_is_idempotent() {
    // Opening the same database twice must not fail on a duplicate column.
    let dir = tempfile::tempdir().unwrap();
    let first = Store::open(dir.path()).await.unwrap();
    first
        .upsert_agent("caas", "hardac", "/w", None, false)
        .await
        .unwrap();
    drop(first);

    let second = Store::open(dir.path()).await.unwrap();
    assert_eq!(second.agents().await.unwrap().len(), 1);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test store`
Expected: FAIL — `upsert_agent` takes 4 arguments, `AgentRow` has no field `is_human`.

- [ ] **Step 3: Add the column to the fresh-database schema**

In `schema.sql`, change the `agents` table to:

```sql
CREATE TABLE IF NOT EXISTS agents (
  name         TEXT PRIMARY KEY,
  host         TEXT NOT NULL,
  cwd          TEXT NOT NULL,
  session_id   TEXT,
  connected_at INTEGER NOT NULL,
  last_seen    INTEGER NOT NULL,
  online       INTEGER NOT NULL DEFAULT 0,
  is_human     INTEGER NOT NULL DEFAULT 0
);
```

- [ ] **Step 4: Add the migration**

In `src/store/mod.rs`, add this method to `impl Store` and call it from `open` immediately after the schema is applied (find where `schema.sql` is executed and add the call on the next line):

```rust
    /// Bring an existing database up to the current schema.
    ///
    /// `schema.sql` is all `CREATE TABLE IF NOT EXISTS`, which covers a fresh file but
    /// does nothing for a database created before a column existed — and the deployed
    /// bus keeps its data in a named Docker volume that long outlives any one binary.
    ///
    /// SQLite has no `ADD COLUMN IF NOT EXISTS`, so this asks `PRAGMA table_info` what
    /// is actually there rather than issuing the `ALTER` and swallowing the resulting
    /// error — an error whose message is not part of any stability guarantee, and which
    /// would hide a genuinely failed migration behind the same catch.
    async fn migrate(&self) -> anyhow::Result<()> {
        let cols = sqlx::query("PRAGMA table_info(agents)")
            .fetch_all(&self.pool)
            .await?;
        let has_is_human = cols
            .iter()
            .any(|r| r.get::<String, _>("name") == "is_human");
        if !has_is_human {
            sqlx::query("ALTER TABLE agents ADD COLUMN is_human INTEGER NOT NULL DEFAULT 0")
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }
```

- [ ] **Step 5: Add the field to `AgentRow` and widen `upsert_agent`**

In `src/store/mod.rs`, add `pub is_human: bool,` as the last field of `AgentRow`. Update the `agents()` query and its row mapping to select and read it, and widen `upsert_agent`:

```rust
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
```

In `agents()`, change the SELECT to `SELECT name, host, cwd, session_id, online, is_human FROM agents ORDER BY name` and add `is_human: r.get("is_human"),` to the `AgentRow` construction.

- [ ] **Step 6: Fix every existing `upsert_agent` call site**

Run: `cargo build 2>&1 | grep -n "upsert_agent"`
Add `, false` as the final argument at each site the compiler names. These are existing agents; none of them is a human.

- [ ] **Step 7: Run the tests**

Run: `cargo test`
Expected: PASS, count up by 4 from 205.

- [ ] **Step 8: Format and commit**

```bash
cargo +nightly fmt
git add schema.sql src/store/mod.rs tests/store.rs
git commit -m "feat: record whether an agent is a human"
```

---

### Task 2: The backward-compatible `Register` field

**Files:**
- Modify: `src/proto.rs`
- Test: `tests/store.rs` (append — this is a pure serde test, no bus needed)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `ToBus::Register { name, host, cwd, session_id, human }` where `human` is `#[serde(default)]`.

**The `#[serde(default)]` is the single most important line in this plan.** A Claude Code session holds a stdio MCP subprocess spawned once at session start, and Claude Code does not respawn stdio servers mid-session — so agents running right now cannot be updated without restarting their sessions. They send `Register` payloads with no `human` key. Without the default, every one of them fails to deserialize and silently loses its bus connection.

- [ ] **Step 1: Write the failing test**

Append to `tests/store.rs`:

```rust
#[test]
fn a_register_payload_without_the_human_field_still_deserializes() {
    // This is exactly the payload an already-running agent binary sends. Claude Code
    // does not respawn stdio MCP servers mid-session, so those binaries cannot be
    // updated without restarting their sessions — if this ever fails, shipping the
    // change silently disconnects every live agent.
    let old = r#"{"type":"register","name":"caas","host":"hardac","cwd":"/w","session_id":null}"#;
    let parsed: claude_bus::proto::ToBus = serde_json::from_str(old).unwrap();
    match parsed {
        claude_bus::proto::ToBus::Register { name, human, .. } => {
            assert_eq!(name, "caas");
            assert!(!human, "an absent field must mean not human");
        }
        other => panic!("expected Register, got {other:?}"),
    }
}

#[test]
fn a_register_payload_with_human_true_round_trips() {
    let new = r#"{"type":"register","name":"bbaldino","host":"hardac","cwd":"/w","session_id":null,"human":true}"#;
    let parsed: claude_bus::proto::ToBus = serde_json::from_str(new).unwrap();
    match parsed {
        claude_bus::proto::ToBus::Register { human, .. } => assert!(human),
        other => panic!("expected Register, got {other:?}"),
    }
}
```

The JSON literals above are correct as written: `ToBus` carries `#[serde(tag = "type", rename_all = "snake_case")]`, so `"type":"register"` is the right discriminator.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test store a_register_payload`
Expected: FAIL — no field `human` on `ToBus::Register`.

- [ ] **Step 3: Add the field**

In `src/proto.rs`, in the `Register` variant:

```rust
    Register {
        name: String,
        host: String,
        cwd: String,
        session_id: Option<String>,
        /// Absent on the wire means `false`, which is what makes this change safe to
        /// deploy under running agents: Claude Code spawns a stdio MCP server once at
        /// session start and never respawns it, so agent binaries in flight when this
        /// ships keep sending the old payload shape indefinitely.
        #[serde(default)]
        human: bool,
    },
```

- [ ] **Step 4: Fix the destructure in the connection loop**

`src/bus/mod.rs` destructures `ToBus::Register { name, host, cwd, session_id }` around line 319. Add `human,` to that pattern. Do not use it yet — Task 3 threads it through. Binding it with a leading underscore (`human: _human`) is fine for this task if the compiler warns about it being unused; Task 3 removes the underscore.

- [ ] **Step 5: Run the tests**

Run: `cargo test`
Expected: PASS, count up by 2.

- [ ] **Step 6: Format and commit**

```bash
cargo +nightly fmt
git add src/proto.rs src/bus/mod.rs tests/store.rs
git commit -m "feat: let a registration declare itself human"
```

---

### Task 3: Thread `human` through registration

**Files:**
- Modify: `src/bus/mod.rs`
- Test: `tests/bus.rs` (append)

**Interfaces:**
- Consumes: `upsert_agent(..., is_human)` from Task 1, `Register.human` from Task 2.
- Produces: a connection-scoped `is_human: bool` in `connection()`, available to Tasks 4 and 5. The `agent_registered` event's detail gains `"is_human"`.

- [ ] **Step 1: Write the failing test**

Append to `tests/bus.rs`. Note `connect` in `tests/common/mod.rs` hardcodes `human: false`; add a sibling helper there named `connect_human(port, name)` that sends the same `Register` with `human: true`, and export it the same way `connect` is exported.

```rust
#[tokio::test]
async fn a_human_registration_is_recorded_as_human() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await; // Registered

    let store = Store::open(&store_dir).await.unwrap();
    let agents = store.agents().await.unwrap();
    let me = agents.iter().find(|a| a.name == "bbaldino").unwrap();
    assert!(me.is_human, "the flag must reach the store");

    let regs = store.events_of_kind("agent_registered", 10).await.unwrap();
    let mine = regs
        .iter()
        .find(|e| e.agent.as_deref() == Some("bbaldino"))
        .expect("a registration event");
    assert_eq!(
        mine.detail["is_human"], true,
        "the event log must distinguish a person joining from a bot: {:?}",
        mine.detail
    );
}

#[tokio::test]
async fn an_ordinary_agent_is_not_recorded_as_human() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;

    let store = Store::open(&store_dir).await.unwrap();
    let agents = store.agents().await.unwrap();
    assert!(!agents.iter().find(|a| a.name == "caas").unwrap().is_human);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test bus a_human_registration`
Expected: FAIL — `connect_human` not found, then once added, `is_human` is false.

- [ ] **Step 3: Thread it through**

In `src/bus/mod.rs`'s `connection()`, alongside the existing `let mut me: Option<String> = None;` add:

```rust
    // Set once, at Register, and read by the teardown (to decide whether room
    // membership was ephemeral) and by every send (to decide whether the guards
    // apply). A connection registers exactly once, so this never changes after.
    let mut is_human = false;
```

In the `Register` arm, remove the underscore from the destructured `human`, set `is_human = *human;` where the other registration state is assigned, pass `is_human` as the new final argument to `upsert_agent`, and add `"is_human": is_human` to the `agent_registered` event's `json!` detail.

- [ ] **Step 4: Run the tests**

Run: `cargo test`
Expected: PASS, count up by 2.

- [ ] **Step 5: Format and commit**

```bash
cargo +nightly fmt
git add src/bus/mod.rs tests/bus.rs tests/common/mod.rs
git commit -m "feat: record a human registration in the store and event log"
```

---

### Task 4: Ephemeral room membership

**Files:**
- Modify: `src/store/mod.rs`, `src/bus/mod.rs`
- Test: `tests/bus.rs` (append)

**Interfaces:**
- Consumes: the connection-scoped `is_human` from Task 3.
- Produces: `Store::leave_all_rooms(agent: &str) -> anyhow::Result<()>`.

An agent stays a room member forever; a human must not. If a human's membership persisted, every later room send would report `queued_for: [bbaldino]` and agents would reasonably infer a reply was coming from someone who had closed their terminal.

- [ ] **Step 1: Write the failing test**

Append to `tests/bus.rs`:

```rust
#[tokio::test]
async fn a_humans_room_membership_ends_when_they_disconnect() {
    let (_d, port, store_dir) = start_bus_with_dir().await;

    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    send(&mut a, &ToBus::Join { req_id: 1, room: "demo".into() }).await;
    next_event(&mut a).await;

    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await;
    send(&mut h, &ToBus::Join { req_id: 2, room: "demo".into() }).await;
    next_event(&mut h).await;

    let store = Store::open(&store_dir).await.unwrap();
    assert!(
        store.room_members("demo").await.unwrap().contains(&"bbaldino".to_string()),
        "precondition: the human is a member while connected"
    );

    drop(h); // the human closes their terminal
    assert!(
        wait_until(|| async {
            let s = Store::open(&store_dir).await.unwrap();
            !s.room_members("demo").await.unwrap().contains(&"bbaldino".to_string())
        })
        .await,
        "the human's membership must not outlive their connection"
    );

    // And the agent must not be told a departed human is a pending recipient.
    send(&mut a, &ToBus::Send {
        req_id: 3,
        target: Target::Room { room: "demo".into() },
        text: "anyone there?".into(),
        done: false,
    })
    .await;
    match next_event(&mut a).await {
        FromBus::Reply { result: ReplyResult::Sent { delivered_to, queued_for, .. }, .. } => {
            assert!(!queued_for.contains(&"bbaldino".to_string()), "queued_for: {queued_for:?}");
            assert!(!delivered_to.contains(&"bbaldino".to_string()), "delivered_to: {delivered_to:?}");
        }
        other => panic!("expected a Sent reply, got {other:?}"),
    }
}

#[tokio::test]
async fn an_agents_room_membership_survives_disconnection() {
    // The contrast that makes the human case meaningful: agents stay members so
    // messages queue for them while they are away.
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    send(&mut a, &ToBus::Join { req_id: 1, room: "demo".into() }).await;
    next_event(&mut a).await;

    drop(a);
    assert!(
        wait_until(|| async { !agent_is_online(port, "caas").await }).await,
        "caas never went offline"
    );

    let store = Store::open(&store_dir).await.unwrap();
    assert!(
        store.room_members("demo").await.unwrap().contains(&"caas".to_string()),
        "an agent stays a member after disconnecting"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test bus room_membership`
Expected: FAIL — the human is still a member after disconnect.

- [ ] **Step 3: Add `leave_all_rooms`**

In `src/store/mod.rs`:

```rust
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
```

- [ ] **Step 4: Call it from teardown**

In `src/bus/mod.rs`, in the teardown block that runs `detach` and `set_online(false)`, add before the `agent_disconnected` event write:

```rust
        if is_human {
            // Ephemeral by design — see `leave_all_rooms`.
            let _ = app.store.leave_all_rooms(&name).await;
        }
```

- [ ] **Step 5: Run the tests**

Run: `cargo test`
Expected: PASS, count up by 2.

- [ ] **Step 6: Format and commit**

```bash
cargo +nightly fmt
git add src/store/mod.rs src/bus/mod.rs tests/bus.rs
git commit -m "feat: a human's room membership lasts only as long as their connection"
```

---

### Task 5: The guards recognise a human

**Files:**
- Modify: `src/bus/delivery.rs`, `src/bus/commands.rs`, `src/bus/mod.rs`
- Test: `tests/bus.rs` (append)

**Interfaces:**
- Consumes: the connection-scoped `is_human` from Task 3.
- Produces: `Guards::check(&self, room: &str, agent: &str, now_ms: i64, is_human: bool) -> GuardVerdict`, and `commands::handle(app, me, cmd, control_tx, is_human)`.

Three behaviours, all one idea — the guards exist to stop agents talking to each other unattended, and a human in the room is the condition they were watching for:

1. A human's send resets the exchange counter.
2. A paused room un-pauses, and the send goes through. **This is the one that is easy to get wrong**: a human whose message bounced off a pause could not rescue the conversation the pause was protecting them from.
3. The per-agent rate limit does not apply.

- [ ] **Step 1: Write the failing tests**

Append to `tests/bus.rs`:

```rust
#[tokio::test]
async fn a_human_send_resets_the_exchange_counter() {
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await;
    send(&mut h, &ToBus::Join { req_id: 1, room: "loop".into() }).await;
    next_event(&mut h).await;

    // Nineteen agent sends: one short of the cap.
    for i in 0..19 {
        send(&mut a, &ToBus::Send {
            req_id: 100 + i,
            target: Target::Room { room: "loop".into() },
            text: format!("m{i}"),
            done: false,
        })
        .await;
        next_event(&mut a).await;
    }

    // The human speaks, which is the signal the cap was built to detect.
    send(&mut h, &ToBus::Send {
        req_id: 2,
        target: Target::Room { room: "loop".into() },
        text: "still here".into(),
        done: false,
    })
    .await;
    next_event(&mut h).await;

    // The counter is back to zero, so the agent gets a full cap's worth again.
    for i in 0..19 {
        send(&mut a, &ToBus::Send {
            req_id: 200 + i,
            target: Target::Room { room: "loop".into() },
            text: format!("n{i}"),
            done: false,
        })
        .await;
        match next_event(&mut a).await {
            FromBus::Reply { result: ReplyResult::Sent { .. }, .. } => {}
            other => panic!("send {i} after the human spoke should have been allowed: {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_human_can_send_into_a_paused_room_and_unpauses_it() {
    // The important one. A pause exists because no human was present; a human
    // arriving is exactly the condition that should clear it. If the human's send
    // bounced, they could not rescue the conversation.
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;

    // Drive the room past the cap.
    for i in 0..21 {
        send(&mut a, &ToBus::Send {
            req_id: 100 + i,
            target: Target::Room { room: "loop".into() },
            text: format!("m{i}"),
            done: false,
        })
        .await;
        next_event(&mut a).await;
    }
    // Confirm it really is paused for an agent.
    send(&mut a, &ToBus::Send {
        req_id: 300,
        target: Target::Room { room: "loop".into() },
        text: "still going".into(),
        done: false,
    })
    .await;
    match next_event(&mut a).await {
        FromBus::Paused { .. } | FromBus::Error { .. } => {}
        other => panic!("precondition: the room should be paused for an agent, got {other:?}"),
    }

    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await;
    send(&mut h, &ToBus::Join { req_id: 1, room: "loop".into() }).await;
    next_event(&mut h).await;
    send(&mut h, &ToBus::Send {
        req_id: 2,
        target: Target::Room { room: "loop".into() },
        text: "hold on, let me look".into(),
        done: false,
    })
    .await;
    match next_event(&mut h).await {
        FromBus::Reply { result: ReplyResult::Sent { .. }, .. } => {}
        other => panic!("a human must be able to speak into a paused room: {other:?}"),
    }

    // And the room is open again for the agent.
    send(&mut a, &ToBus::Send {
        req_id: 301,
        target: Target::Room { room: "loop".into() },
        text: "ok".into(),
        done: false,
    })
    .await;
    match next_event(&mut a).await {
        FromBus::Reply { result: ReplyResult::Sent { .. }, .. } => {}
        other => panic!("the room should have un-paused: {other:?}"),
    }
}

#[tokio::test]
async fn a_human_is_not_rate_limited() {
    // start_bus_with_keepalive is not what we want here; use the rate-limited variant.
    let guards = claude_bus::bus::delivery::Guards::new(claude_bus::bus::delivery::DEFAULT_CAP, 5_000);
    let (_d, port, _path) = start_bus_with_guards(guards).await;

    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await;
    send(&mut h, &ToBus::Join { req_id: 1, room: "demo".into() }).await;
    next_event(&mut h).await;

    for i in 0..3 {
        send(&mut h, &ToBus::Send {
            req_id: 10 + i,
            target: Target::Room { room: "demo".into() },
            text: format!("typing fast {i}"),
            done: false,
        })
        .await;
        match next_event(&mut h).await {
            FromBus::Reply { result: ReplyResult::Sent { .. }, .. } => {}
            other => panic!("a person typing is not a runaway loop: {other:?}"),
        }
    }
}
```

`tests/common/mod.rs` already has `start_bus_with_guards_dir(guards)`, which returns `(TempDir, u16, PathBuf)`. Use that and discard the path: `let (_d, port, _path) = start_bus_with_guards_dir(guards).await;`. Do not add a new helper.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test bus human`
Expected: FAIL — the human's send is refused by the pause, and rate-limited.

- [ ] **Step 3: Add the human path to `Guards::check`**

In `src/bus/delivery.rs`, change the signature and add the branch at the very top of the method body, before the cap check:

```rust
    pub async fn check(
        &self,
        room: &str,
        agent: &str,
        now_ms: i64,
        is_human: bool,
    ) -> GuardVerdict {
        let mut rooms = self.rooms.lock().await;
        let state = rooms.entry(room.to_string()).or_default();

        // Both guards exist to stop agents talking to each other unattended. A human
        // speaking is the condition they were watching for, so it clears the counter
        // outright rather than consuming from it — which also un-pauses a room that
        // had already hit the cap. Returning early likewise skips the rate limit: a
        // person typing is not a runaway loop, and throttling someone mid-interjection
        // would be maddening.
        if is_human {
            state.exchanges = 0;
            state.last_send.insert(agent.to_string(), now_ms);
            return GuardVerdict::Allow;
        }

        if state.exchanges >= self.cap {
            return GuardVerdict::Paused {
                count: state.exchanges,
            };
        }
        // ... rest unchanged
```

- [ ] **Step 4: Pass `is_human` down to the call site**

In `src/bus/commands.rs`, widen `handle`:

```rust
pub(crate) async fn handle(
    app: &App,
    me: &str,
    cmd: ToBus,
    control_tx: &registry::Sender,
    is_human: bool,
) {
```

and change the single `app.guards.check(&room, me, now_ms())` call to `app.guards.check(&room, me, now_ms(), is_human)`. Do not move that call or add a second one — it mutates on `Allow`, so a duplicate would consume two units of the room's budget per send.

In `src/bus/mod.rs`, change the dispatch to `commands::handle(&app, &name, cmd, &control_tx, is_human).await;`.

- [ ] **Step 5: Run the tests**

Run: `cargo test`
Expected: PASS, count up by 3.

- [ ] **Step 6: Format and commit**

```bash
cargo +nightly fmt
git add src/bus/delivery.rs src/bus/commands.rs src/bus/mod.rs tests/bus.rs tests/common/mod.rs
git commit -m "feat: a human's send resets the cap, un-pauses the room, and skips the rate limit"
```

---

### Task 6: The `chat` CLI

**Files:**
- Create: `src/chat.rs`
- Modify: `src/lib.rs`, `src/main.rs`
- Test: `tests/bus.rs` (append — one end-to-end test over the real protocol)

**Interfaces:**
- Consumes: everything from Tasks 1–5.
- Produces: `claude_bus::chat::run(bus_url: String, room: String, name: String) -> anyhow::Result<()>` and the `claude-bus chat <room>` subcommand.

Model this on `src/tail.rs`, which already runs the same WebSocket receive loop. The differences: `Register` with `human: true` instead of `Observe`, `Join` instead of `Watch`, and a stdin reader feeding `Send`.

- [ ] **Step 1: Write the failing test**

Append to `tests/bus.rs`. This drives the protocol the CLI speaks rather than the terminal UI, which is the part worth pinning:

```rust
#[tokio::test]
async fn a_human_and_two_agents_can_all_talk_in_one_room() {
    // The whole point of the feature: three participants, and every message reaches
    // the other two.
    let (_d, port) = start_bus().await;

    let mut a = connect(port, "caas").await;
    next_event(&mut a).await;
    send(&mut a, &ToBus::Join { req_id: 1, room: "design".into() }).await;
    next_event(&mut a).await;

    let mut b = connect(port, "dashboard").await;
    next_event(&mut b).await;
    send(&mut b, &ToBus::Join { req_id: 2, room: "design".into() }).await;
    next_event(&mut b).await;

    let mut h = connect_human(port, "bbaldino").await;
    next_event(&mut h).await;
    send(&mut h, &ToBus::Join { req_id: 3, room: "design".into() }).await;
    next_event(&mut h).await;

    send(&mut h, &ToBus::Send {
        req_id: 4,
        target: Target::Room { room: "design".into() },
        text: "what do you two think?".into(),
        done: false,
    })
    .await;
    match next_event(&mut h).await {
        FromBus::Reply { result: ReplyResult::Sent { delivered_to, .. }, .. } => {
            assert!(delivered_to.contains(&"caas".to_string()), "{delivered_to:?}");
            assert!(delivered_to.contains(&"dashboard".to_string()), "{delivered_to:?}");
            assert!(!delivered_to.contains(&"bbaldino".to_string()), "not to the sender");
        }
        other => panic!("expected Sent, got {other:?}"),
    }

    for (who, ws) in [("caas", &mut a), ("dashboard", &mut b)] {
        match next_event(ws).await {
            FromBus::Message { from, text, .. } => {
                assert_eq!(from, "bbaldino", "{who} should see the human as the sender");
                assert_eq!(text, "what do you two think?");
            }
            other => panic!("{who} should have received the human's message: {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test bus a_human_and_two_agents`
Expected: PASS already if Tasks 1–5 are complete — this test pins the protocol behaviour the CLI depends on, and that behaviour is already built. If it fails, something in Tasks 3–5 regressed; fix that before continuing.

- [ ] **Step 3: Write the chat client**

Create `src/chat.rs`:

```rust
//! An interactive room client for a human.
//!
//! `tail` watches a room without joining it — it identifies via `Observe`, which
//! deliberately creates no `agents` row and no `room_members` row. This is the
//! participant counterpart: it registers as a human, joins the room, and sends what
//! is typed. Realtime needs nothing new; the bus already pushes `FromBus::Message`
//! over this same socket.

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use crate::proto::{FromBus, Target, ToBus};

/// How much history to print on joining. Matches the exchange cap, so a human
/// arriving sees at most one full unattended stretch of conversation.
const HISTORY_ON_JOIN: i64 = 20;

pub async fn run(bus_url: String, room: String, name: String) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(&bus_url).await?;
    let (mut sink, mut stream) = ws.split();

    sink.send(Message::text(serde_json::to_string(&ToBus::Register {
        name: name.clone(),
        host: crate::config::RealEnv.hostname(),
        cwd: std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| ".".to_string()),
        session_id: None,
        human: true,
    })?))
    .await?;
    sink.send(Message::text(serde_json::to_string(&ToBus::Join {
        req_id: 1,
        room: room.clone(),
    })?))
    .await?;
    sink.send(Message::text(serde_json::to_string(&ToBus::History {
        req_id: 2,
        room: room.clone(),
        limit: HISTORY_ON_JOIN,
    })?))
    .await?;

    println!("— {room} as {name} — type to send, Ctrl-D to leave —");

    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    // Stdin is blocking, so it gets its own thread rather than starving the runtime.
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut req_id = 100u64;
    loop {
        tokio::select! {
            incoming = stream.next() => {
                let Some(Ok(msg)) = incoming else { break };
                let Ok(text) = msg.into_text() else { continue };
                if text.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<FromBus>(&text) {
                    Ok(FromBus::Message { from, text, .. }) => println!("{from}: {text}"),
                    Ok(FromBus::Reply { result: crate::proto::ReplyResult::History { messages }, .. }) => {
                        for m in messages {
                            println!("{}: {}", m.from, m.text);
                        }
                        println!("— live —");
                    }
                    Ok(FromBus::Error { message, .. }) => eprintln!("! {message}"),
                    _ => {}
                }
            }
            line = line_rx.recv() => {
                let Some(line) = line else { break }; // Ctrl-D
                req_id += 1;
                sink.send(Message::text(serde_json::to_string(&ToBus::Send {
                    req_id,
                    target: Target::Room { room: room.clone() },
                    text: line,
                    done: false,
                })?))
                .await?;
            }
        }
    }
    Ok(())
}
```

`HistoryItem` is `{ id, from, text, done, created_at }`, so the `m.from` / `m.text` above are correct as written. `hostname()` is a trait method, so add `use crate::config::EnvSource;` to the imports at the top of the file.

- [ ] **Step 4: Wire up the subcommand**

Add `pub mod chat;` to `src/lib.rs`. In `src/main.rs`, add a `chat` arm alongside `tail`:

```rust
        Some("chat") => {
            let room = args
                .get(2)
                .filter(|a| !a.starts_with("--"))
                .cloned()
                .unwrap_or_else(|| {
                    eprintln!("usage: claude-bus chat <room> [--bus ws://host:7777/ws] [--name <n>]");
                    std::process::exit(2);
                });
            let bus = flag(&args, "--bus").unwrap_or_else(|| "ws://127.0.0.1:7777/ws".to_string());
            let name = flag(&args, "--name")
                .or_else(|| std::env::var("USER").ok())
                .unwrap_or_else(|| "human".to_string());
            claude_bus::chat::run(bus, room, name).await?;
            Ok(())
        }
```

Add a usage line to the banner printed by the existing usage function:

```rust
    eprintln!("  claude-bus chat <room> [--bus ws://host:7777/ws] [--name <n>]");
```

- [ ] **Step 5: Run the tests and check it builds**

Run: `cargo test && cargo clippy --all-targets`
Expected: PASS, clippy clean, count up by 1.

- [ ] **Step 6: Format and commit**

```bash
cargo +nightly fmt
git add src/chat.rs src/lib.rs src/main.rs tests/bus.rs
git commit -m "feat: claude-bus chat, an interactive room client for a human"
```

---

### Task 7: The web marker and documentation

**Files:**
- Modify: `src/web/mod.rs`, `docs/DEPLOY.md`, `README.md`
- Test: `tests/web.rs` (append)

**Interfaces:**
- Consumes: `AgentRow.is_human` from Task 1.
- Produces: no new API.

- [ ] **Step 1: Write the failing test**

Append to `tests/web.rs`:

```rust
#[tokio::test]
async fn a_human_is_marked_distinctly_in_the_agent_list() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("caas", "hardac", "/w", None, false)
            .await
            .unwrap();
        store
            .upsert_agent("bbaldino", "hardac", "/w", None, true)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/agents").await;
    assert!(body.contains("bbaldino"), "the human must be listed: {body}");
    assert!(
        body.contains("human"),
        "and marked as one rather than looking like a bot: {body}"
    );
    // The marker must not be applied to everyone.
    let human_marks = body.matches("class=\"human\"").count();
    assert_eq!(human_marks, 1, "exactly one row should carry the marker: {body}");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test web a_human_is_marked`
Expected: FAIL — no marker in the output.

- [ ] **Step 3: Render the marker**

In `src/web/mod.rs`'s `agents()` handler, inside the row loop, build a marker and add it after the name cell's anchor:

```rust
        let mark = if a.is_human {
            " <span class=\"human\">human</span>"
        } else {
            ""
        };
```

and change the name cell of the `format!` to `"<tr><td><a href=\"/agents/{p}\">{n}</a>{mark}</td>..."`, adding `mark = mark,` to the argument list. Apply the identical change to `overview()`'s agents table.

Add to the `CSS` const in `src/web/html.rs`:

```
.human{font-size:.8rem;color:#0b57d0;border:1px solid #cfe0fb;border-radius:.6rem;padding:0 .35rem;margin-left:.4rem}\
```

- [ ] **Step 4: Run the tests**

Run: `cargo test`
Expected: PASS, count up by 1.

- [ ] **Step 5: Document it**

In `docs/DEPLOY.md`, add after the "Reading the record afterwards" section:

````markdown
## Joining a conversation yourself

`claude-bus tail <room>` shows a room without joining it. To take part:

```
claude-bus chat protocol
```

You register under your `$USER` name (override with `--name`), join the room, see its
last 20 messages, and then send by typing. Ctrl-D leaves.

Your membership lasts only as long as the session. Agents never see you queued as a
pending recipient once you have gone, and you accumulate no unread backlog — reconnecting
shows recent history instead.

Being in the room also changes how the bus's runaway guards behave. A room pauses after 20
messages with no human input; your sending a message resets that counter, un-pauses a room
that had already stopped, and is never rate-limited. The bus treats you speaking as the
signal the cap was always trying to infer, which makes `contrib/human-active-hook.sh`
unnecessary for rooms you are actually in.
````

In `README.md`, add to the subcommand list:

```markdown
- `claude-bus chat <room>` — join a room as yourself and take part in the conversation.
```

- [ ] **Step 6: Format and commit**

```bash
cargo +nightly fmt
git add src/web/mod.rs src/web/html.rs tests/web.rs docs/DEPLOY.md README.md
git commit -m "feat: mark humans in the agent list and document claude-bus chat"
```

---

## Self-Review

**Spec coverage.** Each spec section against a task:

| Spec section | Task |
| --- | --- |
| `Register.human` with `#[serde(default)]`, backward compat | 2 |
| `agents.is_human` column | 1 |
| Migration via `PRAGMA table_info` | 1 |
| Persistent `agents` row | 1, 3 |
| Ephemeral `room_members` | 4 |
| `claude-bus chat <room>`, 20 messages of history, `$USER` default | 6 |
| Cap reset on human send | 5 |
| Pause bypass and un-pause | 5 |
| Rate limit bypass | 5 |
| Human marker on `/` and `/agents` | 7 |
| `agent_registered` detail carries `is_human` | 3 |
| Documentation | 7 |

No spec requirement is unimplemented.

**Placeholder scan.** No TBD/TODO. Every code step carries the actual code. Three points that were initially written as "adapt if the codebase differs" were checked against the source and made definite: `ToBus`'s serde tagging (`tag = "type"`, `rename_all = "snake_case"`), `HistoryItem`'s field names (`from`, `text`), and the existing `start_bus_with_guards_dir` helper. No conditional instructions remain.

**Type consistency.** `upsert_agent(name, host, cwd, session_id, is_human)` is defined in Task 1 and used with that arity in Tasks 3 and 7. `AgentRow.is_human` is defined in Task 1 and read in Tasks 3 and 7. `Guards::check(room, agent, now_ms, is_human)` is defined in Task 5 and called only there. `commands::handle(app, me, cmd, control_tx, is_human)` is widened and its only call site updated in the same task. `connect_human(port, name)` is introduced in Task 3 and used in Tasks 4, 5, and 6. `leave_all_rooms(agent)` is defined and called in Task 4. `chat::run(bus_url, room, name)` is defined and called in Task 6.

**One risk carried forward.** Task 1's migration runs against the deployed bus's named Docker volume, which holds real data. The idempotency test covers re-running it, and the populated-database test covers the upgrade path, but neither exercises the actual volume. Before deploying, take a copy: `docker run --rm -v claude-bus-data:/data -v "$PWD":/backup alpine tar czf /backup/claude-bus-data.tgz /data`.
