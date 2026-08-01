# Agent Version Reporting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the bus report which version of `claude-bus` each connected agent is running, so the sessions still holding a stale binary after an upgrade are visible instead of having to be hunted for.

**Architecture:** The agent sends `env!("CARGO_PKG_VERSION")` in its `Register` payload. The bus stores it on the `agents` row, renders it beside its own version on both agent tables, badges any agent that differs, and returns it from the `agents` MCP tool. Nothing branches on the value — it is descriptive only.

**Tech Stack:** Rust, tokio, axum, sqlx (SQLite, runtime queries not macros), tokio-tungstenite, serde, rmcp.

## Global Constraints

- **Backward compatibility is non-negotiable.** Every new protocol field is `#[serde(default)]`. Claude Code spawns a stdio MCP server once at session start and never respawns it, so agent binaries running right now must keep working against the new bus without a redeploy. That is precisely the population this feature exists to make visible — breaking them would defeat it.
- **`None` must stay distinguishable from a reported value.** The field is `Option<String>` end to end. An agent that predates this change sends nothing, stores `NULL`, and displays as `unknown` — that is the "restart this one" signal, not an error case to paper over.
- **The migration must be idempotent by construction** — `PRAGMA table_info` then conditional `ALTER TABLE`, via the existing `add_column_if_missing` helper. Never by swallowing an error whose message could change.
- **Nothing branches on version.** No gating, no minimum, no capability negotiation, no warning that changes behavior. This is a visibility feature; a task that adds a decision based on the version has exceeded the spec.
- **This is descriptive, not a control.** Do not describe it as a security or compatibility mechanism in comments or docs.
- Every event write uses `let _ = app.store.append_event(...)` — a logging failure must never fail the operation being logged.
- Rust formatting: `cargo +nightly fmt` (nightly specifically). `cargo clippy --all-targets` must end clean.
- Only capitalize the first letter of multi-letter acronyms (`RagService`, not `RAGService`).
- No new crate dependencies.
- Baseline before Task 1: **248 tests passing**. Every task must leave the suite green.
- CI runs `cargo +nightly fmt --check`, `cargo +stable clippy --all-targets --all-features -- -D warnings`, and `cargo +stable test --locked`. Toolchain `1.97.1` is installed locally and is what CI's `@stable` currently resolves to — use `cargo +1.97.1 clippy ...` to check, because the local default stable is older and will not catch the same lints.

---

## File Structure

| File | Responsibility | Tasks |
| --- | --- | --- |
| `src/proto.rs` | `Register.version`, `AgentInfo.version` | 1, 3 |
| `schema.sql` | `agents.version` on a fresh database | 1 |
| `src/store/mod.rs` | Migration call, `AgentRow.version`, `upsert_agent` | 1 |
| `src/bus/mod.rs` | Threading `version` from `Register` to the store | 1 |
| `src/agent/bridge.rs`, `src/chat.rs` | Sending the crate version | 1 |
| `src/web/mod.rs` | Version columns, the bus's own version, the mismatch badge | 2 |
| `src/web/html.rs` | Badge CSS | 2 |
| `src/bus/commands.rs` | `AgentInfo.version` in the `ListAgents` reply | 3 |
| `docs/DEPLOY.md` | Documenting what the column means | 3 |

---

### Task 1: A version reaches the agents row

**Files:**
- Modify: `src/proto.rs`, `schema.sql`, `src/store/mod.rs`, `src/bus/mod.rs`, `src/agent/bridge.rs`, `src/chat.rs`
- Test: `tests/store.rs` (append), `tests/bus.rs` (append), `tests/common/mod.rs`

**Interfaces:**
- Produces: `ToBus::Register { name, host, cwd, session_id, human, version }` where `version: Option<String>` is `#[serde(default)]`; `AgentRow.version: Option<String>`; `upsert_agent(name, host, cwd, session_id, is_human, version)` where `version: Option<&str>`.

- [ ] **Step 1: Write the failing tests**

Append to `tests/store.rs`:

```rust
#[tokio::test]
async fn an_agents_version_round_trips_through_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store
        .upsert_agent("caas", "hardac", "/w", None, false, Some("0.1.2"))
        .await
        .unwrap();
    let agents = store.agents().await.unwrap();
    let caas = agents.iter().find(|a| a.name == "caas").unwrap();
    assert_eq!(caas.version.as_deref(), Some("0.1.2"));
}

#[tokio::test]
async fn an_agent_that_reports_no_version_stores_null() {
    // This is the population the feature exists to surface: a binary that predates
    // the field. `None` must survive as `None`, not become an empty string.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store
        .upsert_agent("old", "hardac", "/w", None, false, None)
        .await
        .unwrap();
    let agents = store.agents().await.unwrap();
    assert_eq!(agents.iter().find(|a| a.name == "old").unwrap().version, None);
}

#[tokio::test]
async fn re_registering_updates_a_stale_version() {
    // A session restarted onto a new binary must stop reporting the old one.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    store
        .upsert_agent("caas", "hardac", "/w", None, false, Some("0.1.0"))
        .await
        .unwrap();
    store
        .upsert_agent("caas", "hardac", "/w", None, false, Some("0.1.2"))
        .await
        .unwrap();
    let agents = store.agents().await.unwrap();
    assert_eq!(
        agents.iter().find(|a| a.name == "caas").unwrap().version.as_deref(),
        Some("0.1.2")
    );
}

#[tokio::test]
async fn the_migration_adds_the_version_column_to_an_older_database() {
    // The deployed bus's volume holds an `agents` table without this column.
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
               last_seen INTEGER NOT NULL, online INTEGER NOT NULL DEFAULT 0,
               is_human INTEGER NOT NULL DEFAULT 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agents (name, host, cwd, connected_at, last_seen, online, is_human)
             VALUES ('caas', 'hardac', '/w', 1, 1, 1, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    }

    let store = Store::open(dir.path()).await.unwrap();

    let agents = store.agents().await.unwrap();
    let caas = agents.iter().find(|a| a.name == "caas").unwrap();
    assert_eq!(caas.version, None, "a pre-existing row reports no version");
    assert_eq!(caas.host, "hardac", "existing data must survive the migration");
}

#[test]
fn a_register_payload_without_the_version_field_still_deserializes() {
    // Exactly what an already-running agent binary sends. If this ever fails,
    // shipping the change disconnects every live agent — including the stale ones
    // this feature is meant to reveal.
    let old = r#"{"type":"register","name":"caas","host":"hardac","cwd":"/w","session_id":null,"human":false}"#;
    let parsed: claude_bus::proto::ToBus = serde_json::from_str(old).unwrap();
    match parsed {
        claude_bus::proto::ToBus::Register { name, version, .. } => {
            assert_eq!(name, "caas");
            assert_eq!(version, None, "an absent field must stay absent, not default to a string");
        }
        other => panic!("expected Register, got {other:?}"),
    }
}
```

Append to `tests/bus.rs`:

```rust
#[tokio::test]
async fn a_registering_agent_reports_its_version_to_the_bus() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect_versioned(port, "caas", Some("9.9.9")).await;
    next_event(&mut a).await; // Registered

    let store = Store::open(&store_dir).await.unwrap();
    let agents = store.agents().await.unwrap();
    assert_eq!(
        agents.iter().find(|a| a.name == "caas").unwrap().version.as_deref(),
        Some("9.9.9")
    );
}

#[tokio::test]
async fn an_agent_that_sends_no_version_is_recorded_as_unknown() {
    let (_d, port, store_dir) = start_bus_with_dir().await;
    let mut a = connect_versioned(port, "old", None).await;
    next_event(&mut a).await;

    let store = Store::open(&store_dir).await.unwrap();
    let agents = store.agents().await.unwrap();
    assert_eq!(agents.iter().find(|a| a.name == "old").unwrap().version, None);
}
```

Add this helper to `tests/common/mod.rs`, next to `connect_human`, and export it the same way:

```rust
/// Like `connect`, but with an explicit reported version — `None` stands in for an
/// agent binary predating the field.
pub async fn connect_versioned(port: u16, name: &str, version: Option<&str>) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();
    let reg = ToBus::Register {
        name: name.into(),
        host: "testhost".into(),
        cwd: format!("/w/{name}"),
        session_id: Some(format!("sess-{name}")),
        human: false,
        version: version.map(String::from),
    };
    ws.send(Message::text(serde_json::to_string(&reg).unwrap()))
        .await
        .unwrap();
    ws
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test store an_agents_version_round_trips`
Expected: FAIL — `upsert_agent` takes 5 arguments, `AgentRow` has no field `version`.

- [ ] **Step 3: Add the protocol field**

In `src/proto.rs`, in the `Register` variant, after `human`:

```rust
        /// The agent binary's crate version. Absent on the wire means `None`, which is
        /// the signal worth surfacing rather than an error: Claude Code spawns a stdio
        /// MCP server once at session start and never respawns it, so a binary that
        /// predates this field keeps registering without one indefinitely.
        #[serde(default)]
        version: Option<String>,
```

- [ ] **Step 4: Add the column to the fresh-database schema**

In `schema.sql`, add `version` as the last column of `agents`:

```sql
CREATE TABLE IF NOT EXISTS agents (
  name         TEXT PRIMARY KEY,
  host         TEXT NOT NULL,
  cwd          TEXT NOT NULL,
  session_id   TEXT,
  connected_at INTEGER NOT NULL,
  last_seen    INTEGER NOT NULL,
  online       INTEGER NOT NULL DEFAULT 0,
  is_human     INTEGER NOT NULL DEFAULT 0,
  version      TEXT
);
```

Nullable with no default: `NULL` is the honest representation of "did not say", and a default would erase the distinction the feature depends on.

- [ ] **Step 5: Migrate, widen `upsert_agent`, extend `AgentRow`**

In `src/store/mod.rs`, add a third call inside `migrate`, after the existing two:

```rust
        self.add_column_if_missing("agents", "version", "TEXT").await?;
```

Add `pub version: Option<String>,` as the last field of `AgentRow`. Change the query at roughly line 190 to `SELECT name, host, cwd, session_id, online, is_human, version FROM agents ORDER BY name` and add `version: r.get("version"),` to the `AgentRow` construction — `sqlx` maps SQL `NULL` to `None` for `Option<String>` without extra handling.

Widen `upsert_agent`:

```rust
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
```

`version` is in the `DO UPDATE SET` list deliberately: a session restarted onto a new binary must overwrite the old value rather than keep reporting it.

- [ ] **Step 6: Thread it through registration**

In `src/bus/mod.rs`, the `Register` arm destructures at roughly line 391. Add `version,` to that pattern. At the `upsert_agent` call (roughly line 432), pass `version.as_deref()` as the new final argument — the pattern binds `&Option<String>`, so `as_deref()` yields the `Option<&str>` the store wants.

Add the value to the `agent_registered` event's `json!` detail, alongside the existing `is_human`:

```rust
                                "version": version,
```

- [ ] **Step 7: Make the clients report their version**

In `src/agent/bridge.rs`, in the `ToBus::Register` literal at roughly line 68, add:

```rust
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
```

Do the same in the `ToBus::Register` literal in `src/chat.rs` at roughly line 59. `chat` registers rather than observes, so it gets an `agents` row like any participant and should report a version like one.

`env!("CARGO_PKG_VERSION")` is already used at `src/agent/handler.rs:101` for the MCP `server_info.version`. This is a second consumer of the same constant — do not introduce a different source for the version.

- [ ] **Step 8: Fix every remaining call site**

Run: `cargo build --all-targets 2>&1 | grep -nE "upsert_agent|Register"`

- `upsert_agent` sites in `tests/store.rs` and `tests/web.rs`: add `, None` as the final argument. These are fixtures whose version is irrelevant; `None` also exercises the absent case.
- `ToBus::Register` literals in `tests/bus.rs` (roughly line 38) and `tests/common/mod.rs` (`connect` at roughly 407, `connect_human` at roughly 426): add `version: None,`. Leaving the existing helpers at `None` is deliberate — most tests do not care, and it keeps the new `connect_versioned` helper the only place that asserts on it.

- [ ] **Step 9: Run the tests**

Run: `cargo test`
Expected: PASS, count up by 7 from 248 (five in `tests/store.rs`, two in `tests/bus.rs`).

- [ ] **Step 10: Format, lint, and commit**

```bash
cargo +nightly fmt
cargo +1.97.1 clippy --all-targets --all-features -- -D warnings
git add src/proto.rs schema.sql src/store/mod.rs src/bus/mod.rs src/agent/bridge.rs src/chat.rs tests/store.rs tests/bus.rs tests/common/mod.rs tests/web.rs
git commit -m "feat: agents report their version when they register"
```

---

### Task 2: The dashboard shows versions and flags the odd ones out

**Files:**
- Modify: `src/web/mod.rs`, `src/web/html.rs`
- Test: `tests/web.rs` (append)

**Interfaces:**
- Consumes: `AgentRow.version: Option<String>` from Task 1.
- Produces: no new API.

Both agent tables — the one on `/` and the one on `/agents` — gain a version column, and the page states the bus's own version so the two can be compared without leaving it. An agent whose version differs from the bus's, including one reporting none, gets a badge.

- [ ] **Step 1: Write the failing test**

Append to `tests/web.rs`:

```rust
#[tokio::test]
async fn the_agents_page_shows_versions_and_flags_mismatches() {
    let dir = tempfile::tempdir().unwrap();
    let current = env!("CARGO_PKG_VERSION");
    {
        let store = Store::open(dir.path()).await.unwrap();
        // Matches the bus: should NOT be flagged.
        store
            .upsert_agent("current", "hardac", "/w", None, false, Some(current))
            .await
            .unwrap();
        // Behind the bus: should be flagged.
        store
            .upsert_agent("stale", "hardac", "/w", None, false, Some("0.0.1"))
            .await
            .unwrap();
        // Predates the field entirely: should be flagged, and shown as unknown.
        store
            .upsert_agent("ancient", "hardac", "/w", None, false, None)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/agents").await;
    assert!(body.contains("0.0.1"), "a reported version must be shown: {body}");
    assert!(
        body.contains("unknown"),
        "an agent that reported nothing must read as unknown, not blank: {body}"
    );
    assert!(
        body.contains(current),
        "the bus's own version must be on the page to compare against: {body}"
    );
    // Exactly the two that differ from the bus carry the marker.
    assert_eq!(
        body.matches("class=\"stale\"").count(),
        2,
        "only the differing agents should be flagged: {body}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test web the_agents_page_shows_versions`
Expected: FAIL — no version column rendered.

- [ ] **Step 3: Render the column and the badge**

In `src/web/mod.rs`, add next to the existing `human_mark` helper:

```rust
/// How an agent's reported version renders, and whether it differs from this bus.
///
/// A differing version is the whole signal: Claude Code never respawns a stdio MCP
/// server, so a session started before an upgrade keeps its old binary until someone
/// restarts it, and this is what makes those sessions findable. `None` means a binary
/// predating the field, which is also worth flagging.
///
/// The badge says "differs from this bus", not "broken" — an agent built from a branch
/// would be flagged too, and the version shown beside it tells the reader which case
/// they are looking at.
fn version_cell(version: Option<&str>) -> String {
    let current = env!("CARGO_PKG_VERSION");
    match version {
        Some(v) if v == current => esc(v),
        Some(v) => format!("{} <span class=\"stale\">differs</span>", esc(v)),
        None => "unknown <span class=\"stale\">differs</span>".to_string(),
    }
}
```

In the `agents()` handler, add a `version` header to the table and a cell to each row. The header becomes:

```rust
    let mut b = String::from("<h1>agents</h1><table><tr><th>name<th>host<th>version<th>state</tr>");
```

and the row `format!` gains a `<td>{v}</td>` between host and state, with `v = version_cell(a.version.as_deref()),` in the argument list.

Apply the identical change to `overview()`'s agents table — same header, same cell, same helper.

State the bus's own version on both pages. In `agents()`, after the table is closed:

```rust
    b.push_str(&format!(
        "<p class=\"note\">this bus is running {}</p>",
        esc(env!("CARGO_PKG_VERSION"))
    ));
```

Add the same line after the agents table in `overview()`.

Add to the `CSS` const in `src/web/html.rs`, next to the existing `.human` rule:

```
.stale{font-size:.8rem;color:#8a5a00;border:1px solid #f0d9a8;border-radius:.6rem;padding:0 .35rem;margin-left:.4rem}\
```

- [ ] **Step 4: Run the tests**

Run: `cargo test`
Expected: PASS, count up by 1.

- [ ] **Step 5: Format, lint, and commit**

```bash
cargo +nightly fmt
cargo +1.97.1 clippy --all-targets --all-features -- -D warnings
git add src/web/mod.rs src/web/html.rs tests/web.rs
git commit -m "feat: show agent versions and flag the ones that differ from the bus"
```

---

### Task 3: The `agents` tool reports version, and the docs explain it

**Files:**
- Modify: `src/proto.rs`, `src/bus/commands.rs`, `docs/DEPLOY.md`
- Test: `tests/bus.rs` (append)

**Interfaces:**
- Consumes: `AgentRow.version` from Task 1.
- Produces: `AgentInfo { name, host, online, version }`.

This is what lets a switchboard agent answer "which sessions need restarting?" over the bus instead of a human reading the web page.

- [ ] **Step 1: Write the failing test**

Append to `tests/bus.rs`:

```rust
#[tokio::test]
async fn the_agents_tool_reports_each_agents_version() {
    let (_d, port) = start_bus().await;
    let mut versioned = connect_versioned(port, "fresh", Some("9.9.9")).await;
    next_event(&mut versioned).await;
    let mut ancient = connect_versioned(port, "ancient", None).await;
    next_event(&mut ancient).await;

    send(&mut versioned, &ToBus::ListAgents { req_id: 1 }).await;
    match reply_to(&mut versioned, 1).await {
        FromBus::Reply { result: ReplyResult::Agents { agents }, .. } => {
            let fresh = agents.iter().find(|a| a.name == "fresh").expect("fresh listed");
            let ancient = agents.iter().find(|a| a.name == "ancient").expect("ancient listed");
            assert_eq!(fresh.version.as_deref(), Some("9.9.9"));
            assert_eq!(
                ancient.version, None,
                "an agent that reported nothing must stay None over the wire"
            );
        }
        other => panic!("expected an Agents reply, got {other:?}"),
    }
}
```

`reply_to` and `connect_versioned` already exist in `tests/bus.rs` and `tests/common/mod.rs` respectively; do not redefine either.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test bus the_agents_tool_reports`
Expected: FAIL — no field `version` on `AgentInfo`.

- [ ] **Step 3: Add the field and populate it**

In `src/proto.rs`, add to `AgentInfo`:

```rust
    /// The agent's reported crate version, or `None` for a binary predating the field.
    #[serde(default)]
    pub version: Option<String>,
```

In `src/bus/commands.rs`, in the `ListAgents` arm at roughly line 265, add `version: a.version,` to the `AgentInfo` construction. Place it after `host: a.host,` so the field order matches the struct.

- [ ] **Step 4: Run the tests**

Run: `cargo test`
Expected: PASS, count up by 1.

- [ ] **Step 5: Document it**

In `docs/DEPLOY.md`, add a section after *Who agents will act for*:

````markdown
## Which agents are running which version

`/agents` and the overview show each agent's `claude-bus` version alongside the version
the bus itself is running. An agent whose version differs is marked.

This matters because Claude Code spawns an agent's MCP server once at session start and
never respawns it. Upgrading the bus and reinstalling the binary does not touch a session
that is already open — it keeps its old agent until you restart it. The mark is how you
find those sessions.

`unknown` means the agent is running a binary from before agents reported a version at
all, which is the strongest signal it needs restarting.

The `agents` tool returns the same value, so an agent can be asked to survey the fleet
rather than you reading the page.

The mark means "differs from this bus", not "broken". An agent built from a branch would
be marked too; the version shown beside the mark tells you which case you are looking at.
````

- [ ] **Step 6: Format, lint, and commit**

```bash
cargo +nightly fmt
cargo +1.97.1 clippy --all-targets --all-features -- -D warnings
git add src/proto.rs src/bus/commands.rs docs/DEPLOY.md tests/bus.rs
git commit -m "feat: the agents tool reports each agent's version"
```

---

## Self-Review

**Spec coverage.** Each spec section against a task:

| Spec section | Task |
| --- | --- |
| §1 `Register.version` as `Option<String>`, `#[serde(default)]` | 1 |
| §1 agent sends `env!("CARGO_PKG_VERSION")`, reusing the `handler.rs` constant | 1 |
| §1 `Observe` gains nothing | 1 (untouched; `chat` uses `Register`, so it does get the field) |
| §2 nullable `agents.version` via `add_column_if_missing` | 1 |
| §2 `AgentRow.version`, `upsert_agent` takes it, re-registration overwrites | 1 |
| §3 version column on both agent tables | 2 |
| §3 the bus renders its own version on both pages | 2 |
| §3 badge for differing versions, new CSS class beside `.off`/`.human` | 2 |
| §4 `AgentInfo.version` on the `agents` tool | 3 |
| "Nothing branches on version" | all — no task adds a decision on the value |
| Documentation | 3 |

No spec requirement is unimplemented.

**One thing the spec did not anticipate.** The spec says `Observe` gains nothing, and that is respected — but `chat` registers rather than observes, so it necessarily supplies the new field. Task 1 Step 7 has it report the real version rather than `None`, since a `chat` session is a genuine participant with an `agents` row and its version is as worth knowing as any other.

**Placeholder scan.** No TBD/TODO. Every code step carries the actual code. Four facts were checked against the source rather than assumed: `upsert_agent`'s current arity is 5 and its call sites are `src/bus/mod.rs:432` plus fixtures in `tests/store.rs` and `tests/web.rs`; `ToBus::Register` is constructed at `src/agent/bridge.rs:68`, `src/chat.rs:59`, `tests/bus.rs:38`, and `tests/common/mod.rs:407`/`426`; `AgentInfo` is constructed only at `src/bus/commands.rs:265`; and `add_column_if_missing` already exists from the message-origin work, so Task 1 adds a call rather than a helper.

**Type consistency.** `version` is `Option<String>` on `Register`, `AgentRow`, and `AgentInfo`; `Option<&str>` as `upsert_agent`'s parameter and `version_cell`'s. The conversions are explicit at each boundary: `version.as_deref()` at the `upsert_agent` call in Task 1 Step 6, `a.version.as_deref()` at the `version_cell` call in Task 2 Step 3, and a move (`version: a.version`) into `AgentInfo` in Task 3 Step 3. `connect_versioned(port, name, Option<&str>)` is defined in Task 1 and used in Tasks 1 and 3.

**Test count.** 248 baseline → 255 (T1) → 256 (T2) → 257 (T3).

**Deployment note.** Unlike the message-origin work, this migration adds a nullable column with no default and no backfill, so it cannot fail on existing rows. The pre-deploy volume backup is still worth taking out of habit — note the volume is `claude-message-bus_claude-bus-data`, not `claude-bus-data`:

```bash
make bus-down
docker run --rm -v claude-message-bus_claude-bus-data:/data -v "$PWD":/backup \
  alpine tar czf /backup/claude-bus-data-3.tgz /data
make deploy
```

Then restart agent sessions — and note the feature makes its own rollout observable: any session not yet restarted will show `unknown` or an older version on `/agents`.
