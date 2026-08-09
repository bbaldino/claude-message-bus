# Console Data Layer (2a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the data the redesigned console needs — volume aggregates, room state flags, the read endpoints, opt-in presence and event pushes, and a typed TypeScript client with a live store — with no redesigned UI.

**Architecture:** New store methods derive volume buckets and room flags in SQL. `src/web/api.rs` grows four read endpoints returning DTOs with `ts-rs`-generated TypeScript types. Two opt-in observer subscriptions add presence and event pushes, with events fanned out from a broadcast channel inside `Store::append_event` so no event kind can bypass it. `ui/src/data/` gets a typed fetch client, an observer socket, and an observable store.

**Tech Stack:** Rust, axum 0.8, sqlx/SQLite, `ts-rs`, React, TypeScript, vitest.

**Spec:** `docs/superpowers/specs/2026-08-06-console-data-layer-design.md`
**Design handoff:** `docs/ui-design-pass/handoff/README.md`

## Global Constraints

- **Add Rust deps with `cargo add`, Node deps with `npm install`** — never hand-edited versions.
- Format Rust with **nightly** rustfmt: `cargo +nightly fmt`. CI runs `cargo +nightly fmt --check`.
- Rust lints blocking: `cargo +stable clippy --all-targets --all-features -- -D warnings`.
- Rust tests: `cargo +stable test --locked`.
- Prettier config is exactly `{ "singleQuote": true, "semi": false, "printWidth": 100 }`.
- Frontend gate, from `ui/`: `npm run typecheck && npm run format:check && npm test && npm run build`.
- **Every commit is `chore:`** — never `feat:` or `fix:`. `release_commits = "^(feat|fix)[(!:]"` publishes a version on those, and this phase ships no user-visible UI. The releasing commit comes when the console does.
- Never delete from the `messages` or `events` tables.
- Only the first letter of a multi-letter acronym is capitalised in type names.
- API responses are camelCase on the wire; DTOs live in `src/web/api.rs`, never serialized store rows.
- Endpoints return an HTTP error status on store failure. Never `unwrap_or_default()` into an empty success.

## A correction the plan applies deliberately

The handoff derives the `needs you` flag from a `rate_limited` event, asserting that
"`rate_limited` is not rate limiting — it fires when a room hits its back-and-forth cap."

That is right about the behaviour and wrong about this bus's event kinds:

| Kind | Emitted by | Payload |
|---|---|---|
| `rate_limited` | the rate limiter (minimum interval between sends) | `{ retry_in_ms }` |
| `room_paused` | the exchange cap — the thing that needs a human | `{ count }` |

The handoff's own subtitle ("hit 20 exchanges · waiting on you") matches `room_paused`'s
`count`. **This plan derives `needs_you` from `room_paused`, cleared by `resumed`.**

---

### Task 1: Volume buckets in the store

The design's most reusable primitive: messages per five-minute slot, for a room or an agent.

**Files:**
- Modify: `src/store/mod.rs` (add `BucketScope` and `message_buckets` after `history`)
- Test: `tests/store.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub enum BucketScope<'a> { Room(&'a str), Agent(&'a str) }`
  - `Store::message_buckets(&self, scope: BucketScope<'_>, now_ms: i64, bucket_ms: i64, buckets: usize) -> anyhow::Result<Vec<i64>>` — oldest slot first, length always `buckets`

- [ ] **Step 1: Write the failing tests**

Append to `tests/store.rs`:

```rust
#[tokio::test]
async fn message_buckets_place_messages_in_the_right_five_minute_slots() {
    use claude_bus::store::BucketScope;
    let (_d, store) = temp_store().await;
    let now = 1_785_000_000_000i64;
    let five_min = 300_000i64;

    // Two messages in the newest slot, one three slots back, none elsewhere.
    for at in [now - 1_000, now - 2_000, now - (3 * five_min) - 1_000] {
        store
            .append_message_at("protocol", "caas", "hi", false, false, at)
            .await
            .unwrap();
    }

    let b = store
        .message_buckets(BucketScope::Room("protocol"), now, five_min, 12)
        .await
        .unwrap();

    assert_eq!(b.len(), 12, "always returns exactly `buckets` slots");
    assert_eq!(b[11], 2, "newest slot is last");
    assert_eq!(b[8], 1, "three slots back");
    assert_eq!(b.iter().sum::<i64>(), 3, "and nothing anywhere else");
}

#[tokio::test]
async fn message_buckets_ignore_messages_older_than_the_window() {
    use claude_bus::store::BucketScope;
    let (_d, store) = temp_store().await;
    let now = 1_785_000_000_000i64;
    let five_min = 300_000i64;

    store
        .append_message_at("protocol", "caas", "ancient", false, false, now - (13 * five_min))
        .await
        .unwrap();

    let b = store
        .message_buckets(BucketScope::Room("protocol"), now, five_min, 12)
        .await
        .unwrap();

    assert_eq!(b.iter().sum::<i64>(), 0, "outside the window contributes nothing");
}

#[tokio::test]
async fn message_buckets_scope_to_one_agent() {
    use claude_bus::store::BucketScope;
    let (_d, store) = temp_store().await;
    let now = 1_785_000_000_000i64;
    let five_min = 300_000i64;

    store.append_message_at("protocol", "caas", "a", false, false, now - 1_000).await.unwrap();
    store.append_message_at("protocol", "dashboard", "b", false, false, now - 1_000).await.unwrap();

    let caas = store
        .message_buckets(BucketScope::Agent("caas"), now, five_min, 12)
        .await
        .unwrap();

    assert_eq!(caas.iter().sum::<i64>(), 1, "only that agent's own messages");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --test store message_buckets`
Expected: FAIL to compile — `no function or associated item named 'message_buckets'`, and `append_message_at` also missing.

- [ ] **Step 3: Add the test seam for timestamps**

`append_message` stamps `now_ms()` internally, so a test cannot place a message in a chosen slot. Add an explicit-timestamp variant beside it in `src/store/mod.rs`, and make the existing method delegate:

```rust
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
        .bind(done)
        .bind(created_at)
        .bind(human)
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid())
    }
```

Then rewrite the body of the existing `append_message` to `self.append_message_at(room, from, body, done, human, now_ms()).await`, leaving its signature and doc comment unchanged.

- [ ] **Step 4: Implement the buckets**

Add to `src/store/mod.rs`:

```rust
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
            .fetch_all(&self.pool)
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
```

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test --test store message_buckets`
Expected: PASS (3 tests)

- [ ] **Step 6: Full gate and commit**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
git add src/store/mod.rs tests/store.rs
git commit -F - <<'EOF'
chore: count messages per time slot for the volume strip

Grouped in SQL rather than by fetching and counting in Rust: the rail draws one
strip per room and per agent on every poll, and shipping an hour of message
bodies to count them is what this avoids.

Adds append_message_at so tests can place a message in a chosen slot. Bucket
logic is time-dependent, and a test that cannot choose when a message happened
can only assert that something landed somewhere.
EOF
```

---

### Task 2: Room state flags

The two flags the rail shows, derived rather than stored.

**Files:**
- Modify: `src/store/mod.rs` (add `RoomFlag` and `room_flag` after `message_buckets`)
- Test: `tests/store.rs`

**Interfaces:**
- Consumes: nothing from Task 1
- Produces:
  - `pub enum RoomFlag { NeedsYou { exchanges: i64 }, Blocked { queued: i64, waiting_on: Vec<String> } }`
  - `Store::room_flag(&self, room: &str, online: &[String]) -> anyhow::Result<Option<RoomFlag>>`

- [ ] **Step 1: Write the failing tests**

Append to `tests/store.rs`:

```rust
#[tokio::test]
async fn a_paused_room_needs_you() {
    use claude_bus::store::RoomFlag;
    let (_d, store) = temp_store().await;
    store.join_room("protocol", "caas").await.unwrap();
    store
        .append_event("room_paused", Some("caas"), Some("protocol"), serde_json::json!({"count": 20}))
        .await
        .unwrap();

    let flag = store.room_flag("protocol", &["caas".to_string()]).await.unwrap();

    assert!(
        matches!(flag, Some(RoomFlag::NeedsYou { exchanges: 20 })),
        "room_paused carries the exchange count, got {flag:?}"
    );
}

#[tokio::test]
async fn a_resumed_room_no_longer_needs_you() {
    use claude_bus::store::RoomFlag;
    let (_d, store) = temp_store().await;
    store.join_room("protocol", "caas").await.unwrap();
    store
        .append_event("room_paused", Some("caas"), Some("protocol"), serde_json::json!({"count": 20}))
        .await
        .unwrap();
    store
        .append_event("resumed", Some("bbaldino"), Some("protocol"), serde_json::json!({}))
        .await
        .unwrap();

    let flag = store.room_flag("protocol", &["caas".to_string()]).await.unwrap();

    assert!(
        !matches!(flag, Some(RoomFlag::NeedsYou { .. })),
        "the later resumed clears it, got {flag:?}"
    );
}

#[tokio::test]
async fn rate_limited_does_not_need_you() {
    use claude_bus::store::RoomFlag;
    let (_d, store) = temp_store().await;
    store.join_room("protocol", "caas").await.unwrap();
    // rate_limited is the send-interval limiter, NOT the exchange cap. The design
    // handoff conflates them; this test pins the distinction.
    store
        .append_event("rate_limited", Some("caas"), Some("protocol"), serde_json::json!({"retry_in_ms": 420}))
        .await
        .unwrap();

    let flag = store.room_flag("protocol", &["caas".to_string()]).await.unwrap();

    assert!(
        !matches!(flag, Some(RoomFlag::NeedsYou { .. })),
        "rate limiting is not a request for a human, got {flag:?}"
    );
}

#[tokio::test]
async fn a_room_whose_members_are_all_offline_with_unread_is_blocked() {
    use claude_bus::store::RoomFlag;
    let (_d, store) = temp_store().await;
    store.join_room("protocol", "caas").await.unwrap();
    store.join_room("protocol", "dashboard").await.unwrap();
    store.append_message("protocol", "bbaldino", "anyone?", false, true).await.unwrap();
    store.append_message("protocol", "bbaldino", "still there?", false, true).await.unwrap();

    // Nobody online.
    let flag = store.room_flag("protocol", &[]).await.unwrap();

    match flag {
        Some(RoomFlag::Blocked { queued, waiting_on }) => {
            assert_eq!(queued, 4, "two messages unread by each of two members");
            assert_eq!(waiting_on, vec!["caas".to_string(), "dashboard".to_string()]);
        }
        other => panic!("expected Blocked, got {other:?}"),
    }
}

#[tokio::test]
async fn a_room_with_one_member_online_is_not_blocked() {
    use claude_bus::store::RoomFlag;
    let (_d, store) = temp_store().await;
    store.join_room("protocol", "caas").await.unwrap();
    store.join_room("protocol", "dashboard").await.unwrap();
    store.append_message("protocol", "bbaldino", "anyone?", false, true).await.unwrap();

    let flag = store.room_flag("protocol", &["caas".to_string()]).await.unwrap();

    assert!(
        !matches!(flag, Some(RoomFlag::Blocked { .. })),
        "blocked means ALL members are offline, got {flag:?}"
    );
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --test store _needs_you _blocked _not_blocked rate_limited`
Expected: FAIL to compile — `no function or associated item named 'room_flag'`

- [ ] **Step 3: Implement**

Add to `src/store/mod.rs`:

```rust
/// A room's state, derived from the event stream and membership rather than
/// stored as a column. Only two exist, deliberately — an earlier design draft
/// had four and they blurred together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomFlag {
    /// The exchange cap tripped and the room cannot continue without a person.
    NeedsYou { exchanges: i64 },
    /// Messages are waiting for members who are all offline.
    Blocked { queued: i64, waiting_on: Vec<String> },
}

impl Store {
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
        .fetch_optional(&self.pool)
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

        let mut queued = 0i64;
        let mut waiting_on = Vec::new();
        for m in &members {
            let n = self.unread_count(room, m).await?;
            if n > 0 {
                queued += n;
                waiting_on.push(m.clone());
            }
        }
        if waiting_on.is_empty() {
            return Ok(None);
        }
        Ok(Some(RoomFlag::Blocked { queued, waiting_on }))
    }
}
```

Note on the payload: the handoff's subtitle reads "waiting on caas · 2 queued, 0 delivered". `delivered` is not carried, because `Blocked` is *defined* as every member being offline, which makes delivered necessarily zero — the client renders the literal rather than the server sending a constant.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test --test store _needs_you _blocked _not_blocked rate_limited`
Expected: PASS (5 tests)

- [ ] **Step 5: Full gate and commit**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
git add src/store/mod.rs tests/store.rs
git commit -F - <<'EOF'
chore: derive the two room state flags

needs_you comes from room_paused, not rate_limited. The design handoff names
rate_limited and describes the exchange cap, but in this bus rate_limited is the
send-interval limiter carrying retry_in_ms, while room_paused carries the
exchange count the handoff's own subtitle quotes. A test pins the distinction.

blocked surfaces a failure mode with no existing UI anywhere: messages waiting
for members who are all offline. It requires every member offline, so delivered
is necessarily zero and is not carried in the payload.
EOF
```

---

### Task 3: The rail and meta endpoints

**Files:**
- Modify: `src/web/api.rs` (add DTOs and two handlers), `src/web/mod.rs` (two routes)
- Test: `tests/web.rs`
- Generated: `ui/src/types/*.ts`

**Interfaces:**
- Consumes: `Store::message_buckets`, `BucketScope`, `Store::room_flag`, `RoomFlag` (Tasks 1–2); `Registry::online() -> Vec<String>`
- Produces: `GET /api/rail`, `GET /api/meta`; generated types `RailSummary`, `RailRoom`, `RailAgent`, `RoomFlagDto`, `Meta`

- [ ] **Step 1: Write the failing test**

Append to `tests/web.rs`:

```rust
#[tokio::test]
async fn the_rail_summarises_rooms_and_agents() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("caas", "hardac", "/w/caas", None, false, Some("0.3.3"))
            .await
            .unwrap();
        store.join_room("protocol", "caas").await.unwrap();
        store.append_message("protocol", "caas", "hello", false, false).await.unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/api/rail").await;

    assert!(body.contains("\"rooms\""), "got: {body}");
    assert!(body.contains("\"agents\""), "got: {body}");
    assert!(body.contains("\"protocol\""), "the room must appear: {body}");
    assert!(body.contains("\"caas\""), "the agent must appear: {body}");
    // 12 five-minute buckets, oldest first, always full length.
    assert!(body.contains("\"buckets\":["), "buckets must be present: {body}");
    // The agent is offline (mark_all_offline runs at startup), and the room has
    // an unread message for it, so the room is blocked.
    assert!(body.contains("\"blocked\""), "flag must be derived: {body}");
}

#[tokio::test]
async fn meta_reports_the_host_and_version() {
    let dir = tempfile::tempdir().unwrap();
    let port = start(dir.path()).await;

    let body = get(port, "/api/meta").await;

    assert!(body.contains("\"version\""), "got: {body}");
    assert!(
        body.contains(env!("CARGO_PKG_VERSION")),
        "must report the running version: {body}"
    );
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --test web the_rail_summarises meta_reports`
Expected: FAIL — the routes 404, so the body assertions do not hold

- [ ] **Step 3: Implement the DTOs and handlers**

Add to `src/web/api.rs`:

```rust
/// A room's derived state for the rail. Data, not sentences: the client composes
/// the subtitle, because the handoff specifies copy as final and design-owned.
#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum RoomFlagDto {
    NeedsYou { exchanges: i64 },
    Blocked { queued: i64, waiting_on: Vec<String> },
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct RailRoom {
    pub name: String,
    pub members: Vec<String>,
    pub last_activity: Option<i64>,
    /// Twelve five-minute slots, oldest first.
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
    Json(Meta {
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}
```

Add `use axum::http::StatusCode;` to the imports at the top of `src/web/api.rs` if not already present.

- [ ] **Step 4: Register the routes**

In `src/web/mod.rs`, inside `routes()`:

```rust
        .route("/api/rail", get(api::rail))
        .route("/api/meta", get(api::meta))
```

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test --test web the_rail_summarises meta_reports`
Expected: PASS

- [ ] **Step 6: Confirm the generated types**

Run: `cargo test && ls ui/src/types/`
Expected: `RailSummary.ts`, `RailRoom.ts`, `RailAgent.ts`, `RoomFlagDto.ts`, `Meta.ts` alongside the existing `Agent.ts`.

Then from `ui/`: `npm run typecheck && npm run format:check` — both must pass. The generated directory is `.prettierignore`d already.

- [ ] **Step 7: Full gate and commit**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
git add src/web/api.rs src/web/mod.rs tests/web.rs ui/src/types
git commit -F - <<'EOF'
chore: add GET /api/rail and /api/meta

One call for everything the always-visible rail renders. Polled rather than
pushed: buckets are five minutes wide, so a push carries nothing a ~25s poll
does not, and the design forbids animating the strip anyway.

Flags carry data, not sentences — a blocked flag ships queued and waiting_on and
the client writes the subtitle, because the handoff specifies copy as final and
design-owned.

Store failures return 500. An empty rail renders as "everything is quiet", which
is the opposite of the truth when the database is down.
EOF
```

---

### Task 4: Transcript and events endpoints

**Files:**
- Modify: `src/web/api.rs` (two DTOs, two handlers), `src/web/mod.rs` (two routes)
- Modify: `src/store/events.rs` (add a filtered query)
- Test: `tests/web.rs`

**Interfaces:**
- Consumes: `Store::history`, `Store::events`, `Store::events_for_room`
- Produces: `GET /api/rooms/{name}/messages?limit=&before=`, `GET /api/events?room=&kind=&limit=`; generated types `Message`, `Event`

- [ ] **Step 1: Write the failing tests**

Append to `tests/web.rs`:

```rust
#[tokio::test]
async fn the_transcript_returns_a_rooms_messages() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store.append_message("protocol", "caas", "first", false, false).await.unwrap();
        store.append_message("protocol", "bbaldino", "second", true, true).await.unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/api/rooms/protocol/messages?limit=10").await;

    assert!(body.contains("\"first\""), "got: {body}");
    assert!(body.contains("\"second\""), "got: {body}");
    assert!(body.contains("\"human\":true"), "human authority must survive: {body}");
    assert!(body.contains("\"done\":true"), "the done marker must survive: {body}");
}

#[tokio::test]
async fn the_events_endpoint_scopes_to_a_room_or_the_whole_bus() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .append_event("room_joined", Some("caas"), Some("protocol"), serde_json::json!({}))
            .await
            .unwrap();
        store
            .append_event("room_joined", Some("dash"), Some("other"), serde_json::json!({}))
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let scoped = get(port, "/api/events?room=protocol&limit=50").await;
    assert!(scoped.contains("\"protocol\""), "got: {scoped}");
    assert!(!scoped.contains("\"other\""), "must not leak other rooms: {scoped}");

    let whole = get(port, "/api/events?limit=50").await;
    assert!(whole.contains("\"protocol\""), "got: {whole}");
    assert!(whole.contains("\"other\""), "whole-bus scope sees everything: {whole}");
}

#[tokio::test]
async fn the_events_endpoint_filters_by_kind() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .append_event("room_joined", Some("caas"), Some("protocol"), serde_json::json!({}))
            .await
            .unwrap();
        store
            .append_event("ack", Some("caas"), Some("protocol"), serde_json::json!({}))
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/api/events?kind=ack&limit=50").await;

    assert!(body.contains("\"ack\""), "got: {body}");
    assert!(!body.contains("room_joined"), "kind must narrow the fetch: {body}");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --test web the_transcript the_events_endpoint`
Expected: FAIL — the routes 404

- [ ] **Step 3: Add the filtered event query**

Add to `src/store/events.rs`:

```rust
    /// Events narrowed by room and/or kind, newest first.
    ///
    /// The kind filter is applied here rather than in the browser because the
    /// whole-bus scope has no natural bound — the table only grows. Live pushed
    /// events are filtered client-side instead, so toggling a checkbox re-renders
    /// rather than round-trips.
    pub async fn events_filtered(
        &self,
        room: Option<&str>,
        kind: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<EventRow>> {
        let rows = sqlx::query(
            "SELECT * FROM events
             WHERE (?1 IS NULL OR room = ?1)
               AND (?2 IS NULL OR kind = ?2)
             ORDER BY id DESC LIMIT ?3",
        )
        .bind(room)
        .bind(kind)
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.iter().map(event_row).collect())
    }
```

If `event_row` is a private helper in that module, reuse it; the existing `events()` method shows the mapping to copy.

- [ ] **Step 4: Implement the handlers**

Add to `src/web/api.rs`:

```rust
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

#[derive(serde::Deserialize)]
pub(crate) struct TranscriptQuery {
    limit: Option<i64>,
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
    let rows = app
        .store
        .history(&name, q.limit.unwrap_or(100))
        .await
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
        .events_filtered(q.room.as_deref(), q.kind.as_deref(), q.limit.unwrap_or(200))
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
```

Ensure `use axum::extract::{Path, Query};` is imported at the top of `src/web/api.rs`.

- [ ] **Step 5: Register the routes**

In `src/web/mod.rs`, inside `routes()`:

```rust
        .route("/api/rooms/{name}/messages", get(api::room_messages))
        .route("/api/events", get(api::events))
```

- [ ] **Step 6: Run to verify they pass**

Run: `cargo test --test web the_transcript the_events_endpoint`
Expected: PASS (3 tests)

- [ ] **Step 7: Full gate and commit**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
cd ui && npm run typecheck && npm run format:check && cd ..
git add src/web/api.rs src/web/mod.rs src/store/events.rs tests/web.rs ui/src/types
git commit -F - <<'EOF'
chore: add the transcript and events endpoints

Omitting room from /api/events gives the whole-bus scope — the capability that
lets the standalone events page be deleted.

kind narrows the fetch at the source because the whole-bus scope has no natural
bound. Live pushed events are filtered in the browser instead, so toggling a
checkbox re-renders rather than round-trips.
EOF
```

---

### Task 5: Opt-in presence and event subscriptions

**Files:**
- Modify: `src/proto.rs` (two `ToBus` variants, two `FromBus` variants)
- Modify: `src/bus/registry.rs` (`ObserverConn` fields, subscribe methods, two fan-out methods)
- Modify: `src/store/events.rs` (broadcast channel in `append_event`)
- Modify: `src/store/mod.rs` (hold the broadcast sender)
- Modify: `src/bus/mod.rs` (`handle_observer` arms; presence notify at attach/detach; spawn the event relay)
- Test: `tests/bus.rs`

**Interfaces:**
- Consumes: existing `Registry::attach_observer`, `detach_observer`, `watch`, `notify_watchers`
- Produces:
  - `ToBus::WatchPresence { req_id: u64 }`, `ToBus::WatchEvents { req_id: u64, room: Option<String> }`
  - `FromBus::Presence { name: String, host: String, online: bool, last_seen: i64 }`
  - `FromBus::Event { id: i64, kind: String, agent: Option<String>, room: Option<String>, detail: serde_json::Value, created_at: i64 }`
  - `Store::subscribe_events() -> tokio::sync::broadcast::Receiver<EventRow>`

- [ ] **Step 1: Write the failing tests**

Append to `tests/bus.rs`:

```rust
#[tokio::test]
async fn a_subscribed_observer_receives_presence_and_events() {
    let (_d, port) = common::start_bus().await;
    let mut obs = common::connect_observer(port, "console").await;
    common::send(&mut obs, &ToBus::WatchPresence { req_id: 1 }).await;
    common::send(&mut obs, &ToBus::WatchEvents { req_id: 2, room: None }).await;
    // Drain the two replies.
    common::next_event(&mut obs).await;
    common::next_event(&mut obs).await;

    // A registering agent produces both a presence change and an event.
    let _agent = common::connect(port, "caas").await;

    let mut saw_presence = false;
    let mut saw_event = false;
    for _ in 0..8 {
        match common::next_event(&mut obs).await {
            FromBus::Presence { name, online, .. } if name == "caas" && online => {
                saw_presence = true
            }
            FromBus::Event { kind, .. } if kind == "agent_registered" => saw_event = true,
            _ => {}
        }
        if saw_presence && saw_event {
            break;
        }
    }
    assert!(saw_presence, "a subscribed observer must see presence");
    assert!(saw_event, "a subscribed observer must see events");
}

#[tokio::test]
async fn an_unsubscribed_observer_receives_neither() {
    let (_d, port) = common::start_bus().await;
    // Watch a room only — exactly what `claude-bus tail` does.
    let mut obs = common::connect_observer(port, "tail").await;
    common::send(&mut obs, &ToBus::Watch { req_id: 1, room: "protocol".into() }).await;
    common::next_event(&mut obs).await; // Watching reply

    let _agent = common::connect(port, "caas").await;

    // Nothing presence- or event-shaped may arrive. This is the guarantee that
    // made the subscriptions opt-in rather than a firehose.
    common::pump_for(&mut obs, std::time::Duration::from_millis(400)).await;
    // pump_for drains without asserting; re-drive with a bounded read:
    let mut leaked = false;
    for _ in 0..3 {
        match tokio::time::timeout(
            std::time::Duration::from_millis(150),
            common::next_event(&mut obs),
        )
        .await
        {
            Ok(FromBus::Presence { .. }) | Ok(FromBus::Event { .. }) => leaked = true,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(!leaked, "an unsubscribed observer must see neither — this is tail's guarantee");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --test bus subscribed_observer unsubscribed_observer`
Expected: FAIL to compile — `no variant named 'WatchPresence'`

- [ ] **Step 3: Add the protocol variants**

In `src/proto.rs`, add to `ToBus`:

```rust
    /// Subscribe to agent connect/disconnect. Observer-only, opt-in: a `tail`
    /// watching one room must not start receiving fleet-wide traffic.
    WatchPresence { req_id: u64 },
    /// Subscribe to the event stream. `room: None` is the whole bus.
    WatchEvents {
        req_id: u64,
        room: Option<String>,
    },
```

And to `FromBus`:

```rust
    /// An agent connected or disconnected. Only sent to observers that asked.
    Presence {
        name: String,
        host: String,
        online: bool,
        last_seen: i64,
    },
    /// A bus event, as appended to the audit log. Only sent to observers that asked.
    Event {
        id: i64,
        kind: String,
        agent: Option<String>,
        room: Option<String>,
        detail: serde_json::Value,
        created_at: i64,
    },
```

Add `ToBus::WatchPresence { req_id } | ToBus::WatchEvents { req_id, .. } => Some(*req_id)` to the `req_id_of` match in `src/bus/mod.rs`, so a rejection can still correlate.

- [ ] **Step 4: Extend the observer registry**

In `src/bus/registry.rs`, add to `ObserverConn`:

```rust
    presence: bool,
    /// `None` = not subscribed. `Some(None)` = whole bus. `Some(Some(room))` = one room.
    events: Option<Option<String>>,
```

Initialise both to `false` / `None` in `attach_observer`, and add:

```rust
    /// Start sending presence changes to this observer.
    pub async fn watch_presence(&self, id: ObserverId) {
        if let Some(o) = self.observers.lock().await.get_mut(&id) {
            o.presence = true;
        }
    }

    /// Start sending events to this observer, optionally scoped to one room.
    pub async fn watch_events(&self, id: ObserverId, room: Option<String>) {
        if let Some(o) = self.observers.lock().await.get_mut(&id) {
            o.events = Some(room);
        }
    }

    /// Fan a presence change to every observer that subscribed. Like
    /// `notify_watchers`, observers are spectators: a full queue is dropped.
    pub async fn notify_presence(&self, msg: FromBus) {
        let observers = self.observers.lock().await;
        for o in observers.values().filter(|o| o.presence) {
            let _ = o.tx.try_send(msg.clone());
        }
    }

    /// Fan an event to every observer whose subscription scope matches.
    pub async fn notify_event(&self, room: Option<&str>, msg: FromBus) {
        let observers = self.observers.lock().await;
        for o in observers.values() {
            let matches = match &o.events {
                None => false,
                Some(None) => true,
                Some(Some(want)) => room == Some(want.as_str()),
            };
            if matches {
                let _ = o.tx.try_send(msg.clone());
            }
        }
    }
```

- [ ] **Step 5: Broadcast events from the store**

In `src/store/mod.rs`, add a field to `Store` and initialise it in `open`:

```rust
    events_tx: tokio::sync::broadcast::Sender<crate::store::events::EventRow>,
```

```rust
        // 256 is ample for a personal bus; a lagging receiver drops rather than
        // blocking the write path, which is the correct trade for an audit tail.
        let (events_tx, _) = tokio::sync::broadcast::channel(256);
```

and add:

```rust
    /// Subscribe to events as they are appended.
    ///
    /// A broadcast channel rather than a registry callback, so `Store` stays
    /// unaware of the bus. Hooking `append_event` — the single funnel every kind
    /// already passes through — is what stops a kind added later from silently
    /// failing to appear in the dock.
    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<crate::store::events::EventRow> {
        self.events_tx.subscribe()
    }
```

In `src/store/events.rs`, at the end of `append_event` after the insert succeeds and before returning the id:

```rust
        // Ignored deliberately: an error here means nobody is listening, which is
        // the normal case for the CLI and for tests.
        let _ = self.events_tx.send(EventRow {
            id,
            created_at,
            kind: kind.to_string(),
            agent: agent.map(str::to_string),
            room: room.map(str::to_string),
            detail: detail.clone(),
        });
```

`EventRow` must derive `Clone` for the broadcast channel; add it if absent.

- [ ] **Step 6: Wire the bus side**

In `src/bus/mod.rs`, add the two arms to `handle_observer`:

```rust
        ToBus::WatchPresence { req_id } => {
            app.registry.watch_presence(id).await;
            let _ = control_tx.try_send(FromBus::Reply {
                req_id,
                result: ReplyResult::Watching {
                    room: "<presence>".to_string(),
                },
            });
        }

        ToBus::WatchEvents { req_id, room } => {
            app.registry.watch_events(id, room.clone()).await;
            let _ = control_tx.try_send(FromBus::Reply {
                req_id,
                result: ReplyResult::Watching {
                    room: room.unwrap_or_else(|| "<all>".to_string()),
                },
            });
        }
```

After `attach` succeeds in the register path, notify presence:

```rust
                    app.registry
                        .notify_presence(FromBus::Presence {
                            name: effective.clone(),
                            host: host.clone(),
                            online: true,
                            last_seen: crate::store::now_ms(),
                        })
                        .await;
```

And in the disconnect path, after `set_online(false)` and `detach`:

```rust
        app.registry
            .notify_presence(FromBus::Presence {
                name: name.clone(),
                host: String::new(),
                online: false,
                last_seen: crate::store::now_ms(),
            })
            .await;
```

Finally, in `serve_on_full` (or wherever `App` is constructed, before `axum::serve`), spawn the relay:

```rust
    // Events reach observers through the store's broadcast channel, so every
    // append is fanned out regardless of which call site produced it.
    {
        let app_for_events = app.clone();
        let mut rx = app.store.subscribe_events();
        tokio::spawn(async move {
            while let Ok(e) = rx.recv().await {
                app_for_events
                    .registry
                    .notify_event(
                        e.room.as_deref(),
                        FromBus::Event {
                            id: e.id,
                            kind: e.kind,
                            agent: e.agent,
                            room: e.room,
                            detail: e.detail,
                            created_at: e.created_at,
                        },
                    )
                    .await;
            }
        });
    }
```

- [ ] **Step 7: Run to verify they pass**

Run: `cargo test --test bus subscribed_observer unsubscribed_observer`
Expected: PASS (2 tests)

- [ ] **Step 8: Full gate and commit**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
git add src/proto.rs src/bus/registry.rs src/bus/mod.rs src/store/mod.rs src/store/events.rs tests/bus.rs
git commit -F - <<'EOF'
chore: let observers opt into presence and events

Two new observer subscriptions and two push variants. Opt-in rather than a
firehose so `claude-bus tail`, which watches a single room, does not begin
receiving fleet-wide traffic it has to discard — there is a test asserting an
unsubscribed observer receives neither.

Events fan out from a broadcast channel inside append_event rather than from its
call sites. That is the single funnel every kind already passes through, so a
kind added later cannot silently fail to reach the dock, and Store stays unaware
of the registry.
EOF
```

---

### Task 6: The TypeScript data layer

**Files:**
- Create: `ui/src/data/api.ts`, `ui/src/data/live.ts`, `ui/src/data/store.ts`
- Test: `ui/src/data/store.test.ts`

**Interfaces:**
- Consumes: the endpoints from Tasks 3–4 and the pushes from Task 5; generated types in `ui/src/types/`
- Produces: `fetchRail()`, `fetchMeta()`, `fetchMessages(room, limit)`, `fetchEvents(opts)`; `createLive(url)`; `createStore(deps)` exposing `getState()`, `subscribe(fn)`, `selectRoom(name)`, `start()`, `stop()`

- [ ] **Step 1: Write the failing tests**

Create `ui/src/data/store.test.ts`:

```ts
import { beforeEach, expect, test, vi } from 'vitest'
import { createStore } from './store'
import type { RailSummary } from '../types/RailSummary'

const emptyRail: RailSummary = { rooms: [], agents: [] }

function fakeLive() {
  const handlers: Record<string, (p: unknown) => void> = {}
  return {
    on(kind: string, fn: (p: unknown) => void) {
      handlers[kind] = fn
    },
    emit(kind: string, payload: unknown) {
      handlers[kind]?.(payload)
    },
    watchRoom: vi.fn(),
    start: vi.fn(),
    stop: vi.fn(),
  }
}

let live: ReturnType<typeof fakeLive>

beforeEach(() => {
  live = fakeLive()
})

test('a pushed event lands in the log', () => {
  const store = createStore({ live, fetchRail: async () => emptyRail })
  live.emit('event', { id: 1, kind: 'room_joined', agent: 'caas', room: 'protocol', detail: {}, createdAt: 1 })
  expect(store.getState().events[0].kind).toBe('room_joined')
})

test('a presence push flips an agent online', () => {
  const store = createStore({ live, fetchRail: async () => emptyRail })
  store.setState({
    rail: { rooms: [], agents: [{ name: 'caas', host: 'h', version: null, online: false, isHuman: false, lastSeen: 1, buckets: [] }] },
  })
  live.emit('presence', { name: 'caas', host: 'h', online: true, lastSeen: 2 })
  expect(store.getState().rail?.agents[0].online).toBe(true)
})

test('a dropped socket surfaces as disconnected', () => {
  const store = createStore({ live, fetchRail: async () => emptyRail })
  live.emit('connection', 'disconnected')
  expect(store.getState().connection).toBe('disconnected')
})

test('subscribers are notified when state changes', () => {
  const store = createStore({ live, fetchRail: async () => emptyRail })
  const seen = vi.fn()
  store.subscribe(seen)
  live.emit('connection', 'reconnecting')
  expect(seen).toHaveBeenCalled()
})
```

- [ ] **Step 2: Run to verify they fail**

Run from `ui/`: `npm test`
Expected: FAIL — `Failed to resolve import './store'`

- [ ] **Step 3: Write `api.ts`**

```ts
import type { RailSummary } from '../types/RailSummary'
import type { Meta } from '../types/Meta'
import type { Message } from '../types/Message'
import type { Event } from '../types/Event'

async function getJson<T>(path: string): Promise<T> {
  const res = await fetch(path)
  // Never swallow a failure into an empty result: an empty rail renders as
  // "everything is quiet", which is the opposite of the truth when the API is down.
  if (!res.ok) throw new Error(`${path} returned ${res.status}`)
  return (await res.json()) as T
}

export const fetchRail = () => getJson<RailSummary>('/api/rail')
export const fetchMeta = () => getJson<Meta>('/api/meta')

export const fetchMessages = (room: string, limit = 100) =>
  getJson<Message[]>(`/api/rooms/${encodeURIComponent(room)}/messages?limit=${limit}`)

export const fetchEvents = (opts: { room?: string; kind?: string; limit?: number } = {}) => {
  const p = new URLSearchParams()
  if (opts.room) p.set('room', opts.room)
  if (opts.kind) p.set('kind', opts.kind)
  p.set('limit', String(opts.limit ?? 200))
  return getJson<Event[]>(`/api/events?${p}`)
}
```

`encodeURIComponent` on the room name is required: DM rooms are named `dm:a|b`, and agent names legitimately contain `#`.

- [ ] **Step 4: Write `live.ts`**

```ts
export type Connection = 'live' | 'reconnecting' | 'disconnected'

type Handler = (payload: unknown) => void

/// The observer socket. Identifies with Observe, then subscribes to presence and
/// events; the watched room changes as selection does. The participant socket
/// that sends messages is a separate connection and does not exist yet.
export function createLive(url: string) {
  const handlers: Record<string, Handler[]> = {}
  let ws: WebSocket | null = null
  let watching: string | null = null
  let backoff = 500
  let stopped = false

  const emit = (kind: string, payload: unknown) => {
    for (const h of handlers[kind] ?? []) h(payload)
  }

  const send = (msg: unknown) => ws?.readyState === WebSocket.OPEN && ws.send(JSON.stringify(msg))

  function open() {
    if (stopped) return
    ws = new WebSocket(url)

    ws.onopen = () => {
      backoff = 500
      emit('connection', 'live')
      send({ type: 'observe', name: 'console' })
      send({ type: 'watch_presence', req_id: 1 })
      send({ type: 'watch_events', req_id: 2, room: null })
      if (watching) send({ type: 'watch', req_id: 3, room: watching })
    }

    ws.onmessage = (ev) => {
      const msg = JSON.parse(ev.data as string) as { type: string } & Record<string, unknown>
      if (msg.type === 'presence') emit('presence', msg)
      else if (msg.type === 'event') emit('event', msg)
      else if (msg.type === 'message') emit('message', msg)
    }

    ws.onclose = () => {
      if (stopped) return
      emit('connection', 'reconnecting')
      setTimeout(open, backoff)
      backoff = Math.min(backoff * 2, 15_000)
    }

    ws.onerror = () => emit('connection', 'disconnected')
  }

  return {
    on(kind: string, fn: Handler) {
      ;(handlers[kind] ??= []).push(fn)
    },
    watchRoom(room: string) {
      watching = room
      send({ type: 'watch', req_id: 3, room })
    },
    start: open,
    stop() {
      stopped = true
      ws?.close()
    },
  }
}
```

The wire `type` strings above are **confirmed correct**, not guessed: both `ToBus`
(`src/proto.rs:18`) and `FromBus` (`src/proto.rs:196`) carry
`#[serde(tag = "type", rename_all = "snake_case")]`, so `WatchPresence` is
`{"type":"watch_presence"}` and `FromBus::Presence` arrives as `{"type":"presence"}`.
If you change a variant name in Task 5, these literals change with it — a mismatch
fails silently, with the socket connected and no data arriving.

- [ ] **Step 5: Write `store.ts`**

```ts
import type { RailSummary } from '../types/RailSummary'
import type { Event } from '../types/Event'
import type { Message } from '../types/Message'
import type { Connection } from './live'

export type State = {
  rail: RailSummary | null
  events: Event[]
  messages: Message[]
  room: string | null
  connection: Connection
}

type Live = {
  on(kind: string, fn: (payload: unknown) => void): void
  watchRoom(room: string): void
  start(): void
  stop(): void
}

/// One store rather than per-screen hooks: presence and events feed the rail, the
/// dock, the unseen badge and the transcript at once, and separate subscriptions
/// to one stream would let them disagree about what is current.
export function createStore(deps: { live: Live; fetchRail: () => Promise<RailSummary> }) {
  let state: State = { rail: null, events: [], messages: [], room: null, connection: 'reconnecting' }
  const subs = new Set<() => void>()
  const notify = () => subs.forEach((f) => f())

  const setState = (patch: Partial<State>) => {
    state = { ...state, ...patch }
    notify()
  }

  deps.live.on('connection', (p) => setState({ connection: p as Connection }))

  deps.live.on('event', (p) => {
    setState({ events: [p as Event, ...state.events].slice(0, 500) })
  })

  deps.live.on('message', (p) => {
    // Appended, never scrolled to: the design requires a "3 new below" affordance
    // rather than yanking a reader who has scrolled up, so the scroll decision
    // belongs to the component owning the region.
    setState({ messages: [...state.messages, p as Message] })
  })

  deps.live.on('presence', (p) => {
    const { name, online } = p as { name: string; online: boolean }
    if (!state.rail) return
    setState({
      rail: {
        ...state.rail,
        agents: state.rail.agents.map((a) => (a.name === name ? { ...a, online } : a)),
      },
    })
  })

  let timer: ReturnType<typeof setInterval> | null = null

  return {
    getState: () => state,
    setState,
    subscribe(fn: () => void) {
      subs.add(fn)
      return () => subs.delete(fn)
    },
    selectRoom(name: string) {
      setState({ room: name, messages: [] })
      deps.live.watchRoom(name)
    },
    async start() {
      deps.live.start()
      const refresh = async () => {
        try {
          setState({ rail: await deps.fetchRail() })
        } catch {
          // Leave the previous rail in place; the connection pill already reports
          // trouble, and blanking the rail would read as an empty fleet.
        }
      }
      await refresh()
      timer = setInterval(refresh, 25_000)
    },
    stop() {
      if (timer) clearInterval(timer)
      deps.live.stop()
    },
  }
}
```

- [ ] **Step 6: Run the frontend gate**

Run from `ui/`: `npm test && npm run typecheck && npm run format:check && npm run build`
Expected: all pass. Run `npm run format` first if `format:check` complains.

- [ ] **Step 7: Commit**

```bash
git add ui/src/data
git commit -F - <<'EOF'
chore: add the typed client, observer socket and live store

One store rather than per-screen hooks: presence and events feed the rail, the
dock, the unseen badge and the transcript at once, and separate subscriptions to
one stream would let them disagree about what is current.

The store appends new messages but never scrolls. The design requires a "3 new
below" affordance rather than yanking a reader who has scrolled up, so the
pin-to-bottom decision belongs to the component owning the scroll region.

A failed rail refresh leaves the previous rail in place rather than blanking it —
the connection pill already reports trouble, and an empty rail reads as an empty
fleet.
EOF
```

---

### Task 7: Prove the path end to end

Swap the throwaway screen to render live rail data. Disposable — 2b deletes it.

**Files:**
- Modify: `ui/src/App.tsx`
- Test: `ui/src/App.test.tsx`

**Interfaces:**
- Consumes: `createStore`, `createLive`, `fetchRail`
- Produces: nothing downstream

- [ ] **Step 1: Rewrite the test**

Replace `ui/src/App.test.tsx` with:

```tsx
import { render, screen } from '@testing-library/react'
import { expect, test, vi } from 'vitest'
import { App } from './App'

test('renders rooms and agents from the rail', async () => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(
      JSON.stringify({
        rooms: [{ name: 'protocol', members: ['caas'], lastActivity: 1, buckets: [0, 1], flag: null }],
        agents: [
          { name: 'network-debug#2', host: 'hardac', version: '0.3.3', online: false, isHuman: false, lastSeen: 1, buckets: [0] },
        ],
      }),
      { headers: { 'content-type': 'application/json' } },
    ),
  )

  render(<App />)

  expect(await screen.findByText(/protocol/)).toBeDefined()
  expect(await screen.findByText(/network-debug#2/)).toBeDefined()
})

test('shows the error rather than an empty console when the rail fails', async () => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('', { status: 500 }))

  render(<App />)

  expect(await screen.findByText(/500/)).toBeDefined()
})
```

- [ ] **Step 2: Run to verify it fails**

Run from `ui/`: `npm test`
Expected: FAIL — the current App fetches `/api/agents` and renders a table of agents only

- [ ] **Step 3: Rewrite `App.tsx`**

```tsx
import { useEffect, useState } from 'react'
import { fetchRail } from './data/api'
import type { RailSummary } from './types/RailSummary'

// Deliberately unstyled and deliberately temporary. This proves the rail
// aggregate reaches the browser; 2b replaces it with the designed console.
export function App() {
  const [rail, setRail] = useState<RailSummary | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    fetchRail()
      .then(setRail)
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
  }, [])

  if (error) return <p>could not load the rail: {error}</p>
  if (!rail) return <p>loading…</p>

  return (
    <pre>{JSON.stringify(rail, null, 2)}</pre>
  )
}
```

- [ ] **Step 4: Verify end to end against a real binary**

```bash
cd ui && npm run build && cd ..
cargo build
./target/debug/claude-bus serve --port 7799 --data /tmp/claude-bus-2a &
sleep 3
curl -s http://127.0.0.1:7799/api/rail | head -c 200
curl -s http://127.0.0.1:7799/api/meta
kill %1
```

Expected: `/api/rail` returns JSON with `rooms` and `agents` keys; `/api/meta` reports the version. The bundle must be rebuilt before the binary, since `rust-embed` compiles it in.

- [ ] **Step 5: Full gate and commit**

```bash
cd ui && npm test && npm run typecheck && npm run format:check && npm run build && cd ..
cargo +stable test --locked
git add ui/src/App.tsx ui/src/App.test.tsx
git commit -F - <<'EOF'
chore: render the rail summary in the throwaway screen

Proof the aggregate reaches the browser, not a design. 2b deletes this when the
real console replaces it, keeping the pattern where the visible artifact is
disposable and the plumbing is what ships.
EOF
```

---

## Self-Review

**Spec coverage:**

| Spec requirement | Task |
|---|---|
| Volume buckets, server-derived | 1 |
| `needs_you` / `blocked` flags, server-derived | 2 |
| Flags carry data, not sentences | 2, 3 |
| `GET /api/rail` | 3 |
| `GET /api/meta` | 3 |
| `GET /api/rooms/{name}/messages` | 4 |
| `GET /api/events` with room + kind | 4 |
| `500` on store failure, never empty success | 3, 4 |
| camelCase wire, DTOs not store rows, ts-rs types | 3, 4 |
| `ToBus::WatchPresence` / `WatchEvents` | 5 |
| `FromBus::Presence` / `Event` | 5 |
| Opt-in, `tail` unaffected | 5 (test asserts it) |
| Events fan out from `append_event`'s funnel | 5 |
| `api.ts` typed client, non-2xx throws | 6 |
| `live.ts` observer socket, reconnect, connection state | 6 |
| `store.ts` single observable store | 6 |
| Store appends but does not scroll | 6 |
| Rust integration tests for the three derived behaviours | 1, 2, 3 |
| Protocol tests, subscribed and unsubscribed | 5 |
| vitest against the store with a mocked socket | 6 |
| End-to-end smoke check | 7 |

Deliberately absent, matching the spec: the participant websocket (2d), detail-screen
endpoints (2c), unread badges (client-side, and the rail does not carry them), and any
redesigned UI.

**Placeholder scan:** no TBD/TODO, no "add error handling", no "similar to Task N".
Every code step carries the actual content.

**Type consistency:** `BucketScope` and `message_buckets` (Task 1) are consumed by name
in Task 3. `RoomFlag::{NeedsYou, Blocked}` (Task 2) maps to `RoomFlagDto` with the same
variant names and fields in Task 3. `RailSummary { rooms, agents }` (Task 3) is the type
`fetchRail` returns in Task 6 and `App.tsx` renders in Task 7. `Connection` is defined in
`live.ts` and imported by `store.ts`. The `Live` interface in Task 6's `store.ts` matches
the object `createLive` returns and the `fakeLive` test double.

**Wire format verified while writing this plan:** both `ToBus` and `FromBus` carry
`#[serde(tag = "type", rename_all = "snake_case")]`, so the literals in `live.ts` are
confirmed rather than assumed. The coupling remains — renaming a protocol variant in
Task 5 silently breaks Task 6 — so Task 6 states it explicitly rather than leaving the
implementer to discover a connected socket delivering nothing.
