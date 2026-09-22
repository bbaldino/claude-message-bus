# HTTP Participant Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a `wasi:http`-only participant (raven) join the bus over HTTP — register, long-poll to receive, POST to send/resume — with session-level human authority proven by a bus-configured relayer secret.

**Architecture:** An HTTP adapter over the existing internals, not a parallel path. Sends and resumes go through `commands`; presence and delivered-vs-queued go through `Registry`; the exchange cap/rate limit go through `Guards`; receive is cursor-driven from the `Store` (the durable source of truth) with a `Registry` mpsc as the long-poll wakeup. A lease (token) is a `Registry` connection whose lifetime is renewed by polling.

**Tech Stack:** Rust (edition 2024), axum 0.8, sqlx (sqlite), serde/serde_json, sha2, tokio.

**Spec:** `docs/superpowers/specs/2026-09-22-http-participant-design.md`

## Global Constraints

- `cargo +nightly fmt`; clippy clean: `cargo +stable clippy --all-targets --all-features -- -D warnings`.
- Gate (run at the end of every task): `cargo +nightly fmt && cargo +stable clippy --all-targets --all-features -- -D warnings && cargo +stable test --locked`.
- Do not add dependencies unless a task says so; if one does, add it with `cargo add` (never hand-edit `Cargo.toml` versions).
- Acronyms: capitalize only the first letter of a multi-letter acronym in identifiers (this feature introduces none).
- Nothing may delete from the `messages` or `events` tables.
- Commit prefixes: `feat:` for the new capability, `fix:`/`refactor:`/`docs:` as noted. `^(feat|fix)[(!:]` cuts a release, so behaviour-preserving refactors use `refactor:`.
- `ui/src/types/` is ts-rs output — never hand-edit. The participant DTOs are a machine-to-machine API, **not** SPA types, so they deliberately do **not** derive `ts_rs::TS` (nothing to export, nothing for CI's drift check to catch). This is a conscious deviation from `web::api`'s convention; see Task 5.
- The bus binds `0.0.0.0` with no auth. Every new write endpoint carries the same-origin guard `crate::web::origin_matches_host`, exactly like the WS handshake and the hide/delete routes.

## Facts verified while writing this plan

Each was checked against the source, not assumed:

- `commands::handle(app: &App, me: &str, cmd: ToBus, control_tx: &registry::Sender, is_human: bool)` — `src/bus/commands.rs:30`. `is_human` is passed to `guards.check`; the label is `has_human_authority = is_human || app.relayers.contains(me)` at `commands.rs:246`. Only prod call site: `src/bus/mod.rs:653`.
- `Registry::attach(&self, name, host, tx: Sender) -> String` (effective name, suffixes on collision), `detach(&self, name)`, `send_to(&self, name, FromBus) -> bool` (true = delivered, false = absent/full), `is_online(&self, name) -> bool`, `notify_presence(&self, FromBus)`. `Sender = mpsc::Sender<FromBus>`, `CHANNEL_CAPACITY = 64`. `src/bus/registry.rs`.
- `Guards::check(&self, room, agent, now_ms, is_human) -> GuardVerdict` (`Allow{cleared_pause}` / `RateLimited{retry_in_ms}` / `Paused{count}`), `Guards::reset(&self, room)`. `src/bus/delivery.rs`.
- `Store`: `cursor(room, agent) -> i64`, `set_cursor(room, agent, id)` (monotonic `MAX`), `undelivered(room, agent) -> Vec<MessageRow>`, `history`, `room_members(room) -> Vec<String>`, `rooms() -> Vec<RoomRow>` (each `RoomRow{name, members, ...}`), `join_room(room, agent)`, `append_message(room, from, body, done, human) -> i64`, `append_event(kind, agent: Option<&str>, room: Option<&str>, detail)`, `upsert_agent(name, host, cwd, session_id: Option<&str>, is_human, version: Option<&str>)`. `MessageRow{ id, room, from_agent, body, done, created_at, human }`. `src/store/mod.rs`.
- `App { store: Arc<Store>, registry: Registry, guards: Guards, keepalive: Keepalive, relayers: Relayers }` (`src/bus/mod.rs:137`). Constructed in `serve_on_full(listener, data_dir, guards, keepalive, registry, relayers)` (`mod.rs:212`); routes merged at `mod.rs:310` (`.merge(crate::web::routes())`). `serve(port, data_dir, relayers)` at `mod.rs:146`; the `main.rs` serve arm builds `Relayers::new(flags(&args, "--relayer"))` at `main.rs:61`.
- `web::routes() -> Router<App>` (`src/web/mod.rs:299`); `web::origin_matches_host(origin, host) -> bool` is `pub(crate)` (`mod.rs:709`). `web::api` DTOs are camelCase (`#[serde(rename_all = "camelCase")]`) and clamp limits to `MAX_LIMIT = 1000`.
- `proto::Target` = `{ "kind": "room", "room": "..." }` | `{ "kind": "agent", "name": "..." }` (`src/proto.rs:11`); `ToBus::Send { req_id, target, text, done }`; `FromBus::Message { id, room, from, text, done, human }`.
- Test harness (`tests/common/mod.rs`): `start_bus_full(guards, keepalive, registry, relayers) -> (TempDir, u16, PathBuf)`; helpers `get_json(port, path)`, `get_status`, `post_json(port, path, body) -> u16`, `post_json_with_origin`, `wait_until`, `wait_until_bus_ready`, `agent_is_online`. Raw HTTP/1.1 over `TcpStream`, no HTTP-client dependency. `InProcessAgent` drives a real agent over WS.

---

## Task 1: Participant config plumbing

Introduce `ParticipantConfig` and thread it from the CLI through `serve` to `App`, defaulting to "no HTTP authority" so every existing path is unchanged.

**Files:**
- Create: `src/bus/participant.rs`
- Modify: `src/bus/mod.rs` (module decl, `App` field, `serve`/`serve_on*` chain, router unchanged here)
- Modify: `src/main.rs:51-62` (serve arm: parse flags, build config)
- Modify: `tests/common/mod.rs` (`start_bus_full` + a new starter)
- Test: `tests/participant.rs` (new binary), plus a unit test in `src/bus/participant.rs`

**Interfaces:**
- Produces:
  - `pub struct ParticipantConfig { pub relayer_secret: Option<String>, pub reserved_names: std::collections::HashSet<String>, pub lease_ttl: std::time::Duration }` with `Default` (`None`, empty, `Duration::from_millis(120_000)`).
  - `App.participants: participant::Leases` (Task 4 fleshes out `Leases`; this task adds an empty stub `Leases::new(cfg: ParticipantConfig)` storing the config).
  - `serve_on_full(listener, data_dir, guards, keepalive, registry, relayers, participants: ParticipantConfig)`.
  - `tests/common`: `start_bus_with_participants_dir(cfg: ParticipantConfig) -> (TempDir, u16, PathBuf)`.

- [ ] **Step 1: Create the module with the config type and a config-only `Leases` stub**

`src/bus/participant.rs`:
```rust
//! HTTP participant leases: a token-addressed connection whose lifetime is
//! renewed by polling, layered over `Registry` for presence/delivery.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

/// Bus-side configuration for HTTP participation. Default = no authority: no
/// secret, no reserved names, so a bus nobody configured behaves as before.
#[derive(Clone, Debug)]
pub struct ParticipantConfig {
    pub relayer_secret: Option<String>,
    pub reserved_names: HashSet<String>,
    pub lease_ttl: Duration,
}

impl Default for ParticipantConfig {
    fn default() -> Self {
        Self {
            relayer_secret: None,
            reserved_names: HashSet::new(),
            lease_ttl: Duration::from_millis(120_000),
        }
    }
}

/// Token → lease. Cloneable handle over shared state, like `Registry`.
#[derive(Clone)]
pub struct Leases {
    cfg: ParticipantConfig,
    inner: Arc<Mutex<HashMap<String, Lease>>>,
}

pub struct Lease {
    pub name: String,
    pub relayer: bool,
    pub last_seen: tokio::time::Instant,
    pub rx: Arc<Mutex<tokio::sync::mpsc::Receiver<crate::proto::FromBus>>>,
}

impl Leases {
    pub fn new(cfg: ParticipantConfig) -> Self {
        Self { cfg, inner: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn cfg(&self) -> &ParticipantConfig {
        &self.cfg
    }
}
```

- [ ] **Step 2: Wire the module and `App` field in `src/bus/mod.rs`**

Add `pub mod participant;` near the other `pub mod` lines. Add to `App`:
```rust
    pub(crate) participants: participant::Leases,
```
Add `use participant::ParticipantConfig;` and set the field in `serve_on_full` (Step 3).

- [ ] **Step 3: Thread `ParticipantConfig` through the serve chain**

In `serve_on_full`, add a final parameter `participants: ParticipantConfig` and set `participants: participant::Leases::new(participants)` in the `App { .. }` literal. Update the three callers that construct it:
- `serve` (`mod.rs:159`): pass `ParticipantConfig::default()` — for now; Task 1 Step 4 replaces this with the CLI-built config.
- `serve_on_with_keepalive` (`mod.rs:195`): pass `ParticipantConfig::default()`.
- Keep `serve`'s own signature as `serve(port, data_dir, relayers, participants: ParticipantConfig)` and update its `serve_on_full` call to forward it.

- [ ] **Step 4: Parse the CLI flags in `src/main.rs`**

In the `"serve"` arm (`main.rs:51-62`), after building `relayers`:
```rust
            let relayer_secret = flag(&args, "--relayer-secret");
            let reserved_names = flags(&args, "--reserve-name").into_iter().collect();
            let participants = claude_bus::bus::participant::ParticipantConfig {
                relayer_secret,
                reserved_names,
                ..Default::default()
            };
            claude_bus::bus::serve(port, std::path::PathBuf::from(data), relayers, participants)
                .await?;
```
Add `--relayer-secret <s>` and `--reserve-name <n>` to the usage text at `main.rs:37`. (`flag` returns `Option<String>`, `flags` returns `Vec<String>` — both already exist.)

- [ ] **Step 5: Update the test harness**

In `tests/common/mod.rs`, give `start_bus_full` a `participants: ParticipantConfig` parameter and forward it to `serve_on_full`; update the existing `start_bus_*` wrappers to pass `ParticipantConfig::default()`. Add:
```rust
pub async fn start_bus_with_participants_dir(
    participants: claude_bus::bus::participant::ParticipantConfig,
) -> (tempfile::TempDir, u16, std::path::PathBuf) {
    start_bus_full(
        claude_bus::bus::delivery::Guards::new(20, 0),
        claude_bus::bus::Keepalive::default(),
        claude_bus::bus::registry::Registry::new(),
        claude_bus::bus::Relayers::default(),
        participants,
    )
    .await
}
```
(`ParticipantConfig` and `Leases` must be reachable as `claude_bus::bus::participant::*` — confirm the `pub mod participant;` is `pub`.)

- [ ] **Step 6: Write the failing unit test (config default)**

In `src/bus/participant.rs` `#[cfg(test)]`:
```rust
#[test]
fn default_config_grants_no_authority() {
    let c = ParticipantConfig::default();
    assert!(c.relayer_secret.is_none());
    assert!(c.reserved_names.is_empty());
    assert_eq!(c.lease_ttl, std::time::Duration::from_millis(120_000));
}
```

- [ ] **Step 7: Run the gate**

Run: `cargo +stable test --locked` — the whole suite must still pass (this task is pure plumbing; behaviour is unchanged). Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "feat: participant config plumbing (--relayer-secret, --reserve-name)"
```

---

## Task 2: Authority refactor of `commands::handle` (behaviour-preserving)

Replace the bare `is_human: bool` with an explicit `Authority { human_present, relayer }` so the HTTP send path can grant label-only authority without touching the guards.

**Files:**
- Modify: `src/bus/commands.rs` (signature, `guards.check` arg, the authority OR)
- Modify: `src/bus/mod.rs:653` (the one call site)
- Test: existing `tests/bus.rs` / `tests/events.rs` must stay green; add one unit assertion in `commands.rs`.

**Interfaces:**
- Produces: `pub(crate) struct Authority { pub human_present: bool, pub relayer: bool }`; `commands::handle(app, me, cmd, control_tx, authority: Authority)`.
- Consumes: nothing new.

- [ ] **Step 1: Add the type and change the signature**

In `src/bus/commands.rs`:
```rust
/// What authority a connection's command carries. `human_present` means a person
/// is actually at the keyboard — it, and only it, exempts the exchange guard and
/// clears a pause. `relayer` means the connection speaks with a human's authority
/// (label only): its messages are stamped human, but the guards still apply.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Authority {
    pub human_present: bool,
    pub relayer: bool,
}
```
Change `handle`'s last parameter from `is_human: bool` to `authority: Authority`.

- [ ] **Step 2: Rewire the two uses inside the Send arm**

- `guards.check(&room, me, now_ms(), authority.human_present)` (was `is_human`) at `commands.rs:177`.
- `let has_human_authority = authority.human_present || authority.relayer || app.relayers.contains(me);` (was `is_human || app.relayers.contains(me)`) at `commands.rs:246`.

- [ ] **Step 3: Update the WS call site**

`src/bus/mod.rs:653`:
```rust
commands::handle(
    &app,
    &name,
    cmd,
    &control_tx,
    commands::Authority { human_present: is_human, relayer: false },
)
.await;
```
(`is_human` here is the connection-level flag set at Register — unchanged.)

- [ ] **Step 4: Run the failing/greenness check**

Run: `cargo +stable test --locked`. Expected: PASS — this is behaviour-preserving (`relayer: false` everywhere reproduces the old `has_human_authority = is_human || relayers.contains(me)`). If any test fails, the refactor changed behaviour; fix before proceeding.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor: make command authority explicit (human_present vs relayer)"
```

---

## Task 3: Store queries for cross-room receive

Add the two reads `/receive` needs: the participant's member rooms, and its undelivered messages merged across rooms in global-id order.

**Files:**
- Modify: `src/store/mod.rs` (two methods, before the `impl Store` block closes at `mod.rs:765`)
- Test: `tests/store.rs`

**Interfaces:**
- Produces:
  - `Store::member_rooms(&self, agent: &str) -> anyhow::Result<Vec<String>>`
  - `Store::undelivered_for_participant(&self, agent: &str, limit: i64) -> anyhow::Result<Vec<MessageRow>>` — messages across every room `agent` belongs to, `from_agent != agent`, `id >` that room's cursor, ordered by `id ASC`, capped at `limit`.

- [ ] **Step 1: Write the failing test**

In `tests/store.rs` (uses a `Store::open` on a tempdir — match the file's existing setup helper):
```rust
#[tokio::test]
async fn undelivered_for_participant_merges_rooms_in_id_order_and_respects_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let store = claude_bus::store::Store::open(dir.path()).await.unwrap();

    store.join_room("a", "raven").await.unwrap();
    store.join_room("b", "raven").await.unwrap();
    let m1 = store.append_message("a", "caas", "a1", false, false).await.unwrap();
    let m2 = store.append_message("b", "dash", "b1", false, false).await.unwrap();
    let _own = store.append_message("a", "raven", "mine", false, true).await.unwrap();

    let all = store.undelivered_for_participant("raven", 100).await.unwrap();
    let ids: Vec<i64> = all.iter().map(|m| m.id).collect();
    assert_eq!(ids, vec![m1, m2], "own message excluded, merged in id order");

    // Ack past m1 in room a; only b1 remains.
    store.set_cursor("a", "raven", m1).await.unwrap();
    let rest = store.undelivered_for_participant("raven", 100).await.unwrap();
    assert_eq!(rest.iter().map(|m| m.id).collect::<Vec<_>>(), vec![m2]);

    assert_eq!(store.member_rooms("raven").await.unwrap().len(), 2);
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo +stable test --locked undelivered_for_participant`
Expected: FAIL — `no method named member_rooms` / `undelivered_for_participant`.

- [ ] **Step 3: Implement both methods**

In `src/store/mod.rs`:
```rust
    /// Every room `agent` is a member of.
    pub async fn member_rooms(&self, agent: &str) -> anyhow::Result<Vec<String>> {
        let rows = sqlx::query("SELECT room FROM room_members WHERE agent_name = ?1")
            .bind(agent)
            .fetch_all(self.pool())
            .await?;
        Ok(rows.iter().map(|r| r.get::<String, _>("room")).collect())
    }

    /// The participant's undelivered messages across all its rooms, merged in
    /// global id order, capped. Excludes its own; respects each room's cursor.
    pub async fn undelivered_for_participant(
        &self,
        agent: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<MessageRow>> {
        let rows = sqlx::query(
            "SELECT m.id, m.room, m.from_agent, m.body, m.done, m.created_at, m.human
             FROM messages m
             JOIN room_members rm ON rm.room = m.room AND rm.agent_name = ?1
             LEFT JOIN cursors c ON c.room = m.room AND c.agent_name = ?1
             WHERE m.from_agent != ?1
               AND m.id > COALESCE(c.last_delivered_id, 0)
             ORDER BY m.id ASC
             LIMIT ?2",
        )
        .bind(agent)
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.iter().map(message_row).collect())
    }
```
(`message_row` is the existing row→`MessageRow` mapper used by `history`.)

- [ ] **Step 4: Run it, verify it passes**

Run: `cargo +stable test --locked undelivered_for_participant`. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: store reads for cross-room participant receive"
```

---

## Task 4: Lease lifecycle on `Leases`

Flesh out `Leases` with register/renew/lookup/expiry, holding a `Registry` mpsc as the wakeup/delivery channel.

**Files:**
- Modify: `src/bus/participant.rs`
- Test: `src/bus/participant.rs` `#[cfg(test)]`

**Interfaces:**
- Produces (on `Leases`):
  - `async fn open(&self, effective_name: String, relayer: bool, rx: mpsc::Receiver<FromBus>) -> String` — mints a random token, stores the lease with `last_seen = Instant::now()`, returns the token.
  - `struct LeaseHandle { pub name: String, pub relayer: bool, pub rx: Arc<Mutex<mpsc::Receiver<FromBus>>> }`.
  - `async fn touch(&self, token: &str) -> Option<LeaseHandle>` — renews `last_seen` and returns a handle, or `None` if unknown.
  - `async fn name_online(&self, name: &str) -> bool` — whether a lease with that effective name currently exists (for reserved-name takeover).
  - `async fn expired(&self) -> Vec<(String /*token*/, String /*name*/)>` — leases past `cfg.lease_ttl`; removes them from the map and returns them so the sweeper can detach + emit events.
  - `fn decide_relayer(&self, presented: Option<&str>) -> Result<bool, ()>` — `Ok(true)` if secret configured and matches; `Ok(false)` if none presented; `Err(())` if presented but wrong or no secret configured.
  - `fn is_reserved(&self, name: &str) -> bool`.
- Token generation: `sha2::Sha256` over `now_nanos : atomic_counter : name`, hex-encoded, truncated to 32 chars. (No new dep — `sha2` is already used by `store::files`.) Note in a comment: this is an unguessable-enough session handle for a LAN-trust bus, not a CSPRNG; a `rand`-based token is a hardening follow-up.

- [ ] **Step 1: Write the failing tests**
```rust
#[tokio::test]
async fn touch_renews_and_unknown_token_is_none() {
    let leases = Leases::new(ParticipantConfig::default());
    let (_tx, rx) = tokio::sync::mpsc::channel(8);
    let token = leases.open("raven".into(), true, rx).await;
    let h = leases.touch(&token).await.expect("known token");
    assert_eq!(h.name, "raven");
    assert!(h.relayer);
    assert!(leases.touch("bogus").await.is_none());
}

#[tokio::test]
async fn expired_returns_and_removes_stale_leases() {
    let cfg = ParticipantConfig { lease_ttl: std::time::Duration::from_millis(0), ..Default::default() };
    let leases = Leases::new(cfg);
    let (_tx, rx) = tokio::sync::mpsc::channel(8);
    let token = leases.open("raven".into(), false, rx).await;
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    let gone = leases.expired().await;
    assert_eq!(gone, vec![(token, "raven".to_string())]);
    assert!(leases.expired().await.is_empty(), "removed on first sweep");
}

#[test]
fn relayer_decision() {
    let cfg = ParticipantConfig { relayer_secret: Some("s3cret".into()), ..Default::default() };
    let leases = Leases::new(cfg);
    assert_eq!(leases.decide_relayer(Some("s3cret")), Ok(true));
    assert_eq!(leases.decide_relayer(None), Ok(false));
    assert_eq!(leases.decide_relayer(Some("wrong")), Err(()));
    let none = Leases::new(ParticipantConfig::default());
    assert_eq!(none.decide_relayer(Some("anything")), Err(()));
    assert_eq!(none.decide_relayer(None), Ok(false));
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo +stable test --locked participant::`. Expected: FAIL (methods missing).

- [ ] **Step 3: Implement**

Add an `AtomicU64` counter (module-level `static`), the token helper, and the methods on `Leases`. `LeaseHandle` clones `name`/`relayer` and the `Arc<Mutex<Receiver>>`. `expired` locks the map, partitions on `now.duration_since(last_seen) >= cfg.lease_ttl`, drains those, returns `(token, name)` pairs. `decide_relayer` matches on `(cfg.relayer_secret.as_deref(), presented)`.

- [ ] **Step 4: Run, verify pass**

Run: `cargo +stable test --locked participant::`. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: participant lease lifecycle (open/touch/expire/relayer decision)"
```

---

## Task 5: `web::participants` module + register endpoint

Create the HTTP module, the register endpoint, its DTOs, the same-origin guard, secret handling, and reserved-name takeover. Register the route.

**Files:**
- Create: `src/web/participants.rs`
- Modify: `src/web/mod.rs` (`mod participants;`, routes)
- Test: `tests/participant.rs`

**Interfaces:**
- Produces:
  - `web::participants::register` (axum handler): `POST /api/participants`.
  - Response DTO `RegisterResponse { name: String, token: String, relayer: bool, lease_ttl_ms: u64 }` (camelCase on the wire; **no `ts_rs::TS`** — see Global Constraints).
  - Helper `web::participants::origin_ok(&HeaderMap) -> bool` reusing `crate::web::origin_matches_host` (same shape as `bus::origin_permitted`): no `Origin` → allowed; a browser `Origin` must match `Host`.
- Consumes: `Leases::{decide_relayer, is_reserved, name_online, open}`, `Registry::{attach, detach, is_online, notify_presence}`, `Store::{upsert_agent, append_event}`.

- [ ] **Step 1: Write the failing tests**

`tests/participant.rs` (new file; `mod common;`). Add local header-aware helpers (mirroring `tests/web.rs`'s raw style): `post_json_headers(port, path, body, headers: &[(&str,&str)]) -> (u16, String)` and `get_headers(port, path, headers) -> (u16, String)`.
```rust
mod common;
use serde_json::json;

#[tokio::test]
async fn register_without_secret_is_a_plain_participant() {
    let cfg = claude_bus::bus::participant::ParticipantConfig::default();
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    let (status, body) = post_json_headers(port, "/api/participants", json!({"name":"raven"}), &[]).await;
    assert_eq!(status, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["name"], "raven");
    assert_eq!(v["relayer"], false);
    assert!(v["token"].as_str().is_some_and(|t| !t.is_empty()));
}

#[tokio::test]
async fn register_with_correct_secret_is_a_relayer() {
    let cfg = claude_bus::bus::participant::ParticipantConfig {
        relayer_secret: Some("s3cret".into()), ..Default::default() };
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    let (status, body) = post_json_headers(port, "/api/participants",
        json!({"name":"raven"}), &[("X-Relayer-Secret","s3cret")]).await;
    assert_eq!(status, 200);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["relayer"], true);
}

#[tokio::test]
async fn a_wrong_secret_is_refused_not_downgraded() {
    let cfg = claude_bus::bus::participant::ParticipantConfig {
        relayer_secret: Some("s3cret".into()), ..Default::default() };
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    let (status, _body) = post_json_headers(port, "/api/participants",
        json!({"name":"raven"}), &[("X-Relayer-Secret","wrong")]).await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn a_cross_origin_register_is_refused() {
    let cfg = claude_bus::bus::participant::ParticipantConfig::default();
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    let (status, _b) = post_json_headers(port, "/api/participants",
        json!({"name":"raven"}), &[("Origin","http://evil.example")]).await;
    assert_eq!(status, 403);
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo +stable test --locked --test participant register`. Expected: FAIL (route 404 / helpers missing).

- [ ] **Step 3: Implement the register handler**

`src/web/participants.rs`: define `RegisterResponse`, `origin_ok`, and:
```rust
pub(crate) async fn register(
    State(app): State<App>,
    headers: axum::http::HeaderMap,
    Json(body): Json<RegisterRequest>, // { name: String }
) -> axum::response::Response {
    if !origin_ok(&headers) { return StatusCode::FORBIDDEN.into_response(); }
    let secret = headers.get("x-relayer-secret").and_then(|v| v.to_str().ok());
    let relayer = match app.participants.decide_relayer(secret) {
        Ok(r) => r,
        Err(()) => return StatusCode::FORBIDDEN.into_response(),
    };
    if app.participants.is_reserved(&body.name) && !relayer {
        return StatusCode::FORBIDDEN.into_response();
    }
    // Reserved + authorised: take over any existing holder so the bare name is kept.
    if app.participants.is_reserved(&body.name) && app.registry.is_online(&body.name).await {
        app.registry.detach(&body.name).await;
    }
    let already = app.registry.is_online(&body.name).await;
    let (tx, rx) = tokio::sync::mpsc::channel(crate::bus::registry::CHANNEL_CAPACITY);
    let name = app.registry.attach(&body.name, "http", tx).await;
    let _ = app.store.upsert_agent(&name, "http", "", None, false, None).await;
    if !already {
        let _ = app.store.append_event("agent_registered", Some(&name), None,
            serde_json::json!({ "requested_name": body.name, "effective_name": &name,
                "host": "http", "transport": "http", "is_human": false })).await;
        app.registry.notify_presence(crate::proto::FromBus::Presence {
            name: name.clone(), host: "http".into(), online: true,
            last_seen: crate::store::now_ms() }).await;
    }
    let token = app.participants.open(name.clone(), relayer, rx).await;
    Json(RegisterResponse {
        name, token, relayer,
        lease_ttl_ms: app.participants.cfg().lease_ttl.as_millis() as u64,
    }).into_response()
}
```
`origin_ok`:
```rust
pub(crate) fn origin_ok(h: &axum::http::HeaderMap) -> bool {
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok());
    match get("origin") {
        None => true,
        Some(o) => crate::web::origin_matches_host(o, get("host").unwrap_or_default()),
    }
}
```

- [ ] **Step 4: Register the route** in `src/web/mod.rs` `routes()`:
```rust
        .route("/api/participants", post(participants::register))
```
Add `mod participants;` at the top of `web/mod.rs`.

- [ ] **Step 5: Run, verify pass**

Run: `cargo +stable test --locked --test participant`. Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: HTTP participant registration endpoint"
```

---

## Task 6: Send — extract `do_send`, add the send endpoint

Extract the send core into a typed function so WS and HTTP share it, then add `POST /api/participants/send`.

**Files:**
- Modify: `src/bus/commands.rs` (extract `do_send`; the `ToBus::Send` arm calls it and translates to `FromBus`)
- Modify: `src/web/participants.rs` (send handler), `src/web/mod.rs` (route)
- Test: `tests/participant.rs`; existing send tests in `tests/bus.rs` must stay green.

**Interfaces:**
- Produces:
  - `pub(crate) enum SendOutcome { Sent { room: String, msg_id: i64, delivered_to: Vec<String>, queued_for: Vec<String> }, RateLimited { retry_in_ms: i64 }, Paused { room: String, count: u32, reason: String } }`
  - `pub(crate) async fn do_send(app: &App, me: &str, target: Target, text: String, done: bool, authority: Authority) -> Result<SendOutcome, String>` — `Err` is a store/target error message (unknown agent, storage). Contains the current arm's logic: unknown-agent check, `guards.check`, `join_room`, `append_message`, member fan-out via `registry.send_to` (fills delivered/queued), observer `notify_watchers`, `message_sent` + `resumed` events.
- Consumes: `Authority` (Task 2).

- [ ] **Step 1: Write the failing test**
```rust
#[tokio::test]
async fn a_participant_send_reaches_a_connected_agent_and_reports_delivered() {
    let cfg = claude_bus::bus::participant::ParticipantConfig::default();
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    // A real WS agent to receive the DM.
    let bus = format!("ws://127.0.0.1:{port}/ws");
    let mut caas = common::InProcessAgent::start(&bus, "caas");
    common::initialize(&mut caas).await;
    assert!(common::agent_is_online(port, "caas").await);

    let (_s, body) = post_json_headers(port, "/api/participants", json!({"name":"raven"}), &[]).await;
    let token = serde_json::from_str::<serde_json::Value>(&body).unwrap()["token"].as_str().unwrap().to_string();

    let (status, sbody) = post_json_headers(port, "/api/participants/send",
        json!({"target":{"kind":"agent","name":"caas"}, "text":"hi", "done":false}),
        &[("X-Participant-Token", &token)]).await;
    assert_eq!(status, 200, "{sbody}");
    let v: serde_json::Value = serde_json::from_str(&sbody).unwrap();
    assert_eq!(v["outcome"], "sent");
    assert_eq!(v["deliveredTo"], json!(["caas"]));
}
```

- [ ] **Step 2: Run, verify fail** — Run: `cargo +stable test --locked --test participant a_participant_send`. Expected: FAIL (route 404).

- [ ] **Step 3: Extract `do_send`** — move the body of the `ToBus::Send` arm (`commands.rs:~112-350`, after the unknown-agent guard) into `do_send`, returning `SendOutcome`/`Err`. The arm becomes: call `do_send`, then translate — `Sent` → `FromBus::Reply{ ReplyResult::Sent {..} }`; `RateLimited` → the existing `rate_limited` event + `FromBus::Error`; `Paused` → the existing `room_paused` event + `FromBus::Paused` + `FromBus::Error`; `Err(msg)` → `FromBus::Error`. Keep the unknown-agent branch where it is (it already emits `send_refused` + `FromBus::Error`), or fold it into `do_send` returning `Err` — either way `tests/bus.rs` must stay green.

- [ ] **Step 4: Add the send handler + route**

`src/web/participants.rs`:
```rust
pub(crate) async fn send(
    State(app): State<App>,
    headers: axum::http::HeaderMap,
    Json(body): Json<SendRequest>, // { target: Target, text: String, #[serde(default)] done: bool }
) -> axum::response::Response {
    if !origin_ok(&headers) { return StatusCode::FORBIDDEN.into_response(); }
    let Some(token) = headers.get("x-participant-token").and_then(|v| v.to_str().ok()) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(lease) = app.participants.touch(token).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let authority = crate::bus::commands::Authority { human_present: false, relayer: lease.relayer };
    match crate::bus::commands::do_send(&app, &lease.name, body.target, body.text, body.done, authority).await {
        Ok(outcome) => Json(SendOutcomeDto::from(outcome)).into_response(),
        Err(msg) => (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response(),
    }
}
```
`SendOutcomeDto` is a camelCase `#[serde(tag = "outcome", rename_all = "snake_case")]` enum mirroring `SendOutcome` with camelCase fields (`msgId`, `deliveredTo`, `queuedFor`, `retryInMs`). Route: `.route("/api/participants/send", post(participants::send))`.

- [ ] **Step 5: Run, verify pass** — Run: `cargo +stable test --locked` (participant + bus). Expected: PASS.

- [ ] **Step 6: Commit**
```bash
git add -A && git commit -m "feat: HTTP participant send via shared do_send"
```

---

## Task 7: Receive — long-poll with cursor ack

**Files:** Modify `src/web/participants.rs`, `src/web/mod.rs`. Test: `tests/participant.rs`.

**Interfaces:**
- Produces: `GET /api/participants/receive?after=&limit=&timeout=`; response `ReceiveResponse { messages: Vec<MessageDto>, cursor: i64 }` (camelCase; `MessageDto { id, room, from, body, done, human, created_at }`).
- Consumes: `Store::{member_rooms, set_cursor, undelivered_for_participant}` (Task 3), `LeaseHandle.rx`.

- [ ] **Step 1: Write the failing tests**
```rust
#[tokio::test]
async fn receive_returns_a_dm_and_advances_the_cursor() {
    let (_d, port, _p) = common::start_bus_with_participants_dir(Default::default()).await;
    let bus = format!("ws://127.0.0.1:{port}/ws");
    let mut caas = common::InProcessAgent::start(&bus, "caas");
    common::initialize(&mut caas).await;

    let (_s, rb) = post_json_headers(port, "/api/participants", json!({"name":"raven"}), &[]).await;
    let token = serde_json::from_str::<serde_json::Value>(&rb).unwrap()["token"].as_str().unwrap().to_string();
    // First send from raven creates the DM room and enrolls both sides.
    post_json_headers(port, "/api/participants/send",
        json!({"target":{"kind":"agent","name":"caas"},"text":"ping"}),
        &[("X-Participant-Token",&token)]).await;
    // caas replies into the DM room.
    // (drive caas to send back into dm:caas|raven — via its send tool; see tests/bus.rs for the call shape)

    let (status, body) = get_headers(port, "/api/participants/receive?after=0&timeout=2",
        &[("X-Participant-Token",&token)]).await;
    assert_eq!(status, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let msgs = v["messages"].as_array().unwrap();
    assert!(msgs.iter().any(|m| m["from"]=="caas"), "got caas's reply: {body}");
    let cursor = v["cursor"].as_i64().unwrap();

    // A second poll acking `cursor` returns nothing new before timeout.
    let (_s2, body2) = get_headers(port,
        &format!("/api/participants/receive?after={cursor}&timeout=1"),
        &[("X-Participant-Token",&token)]).await;
    assert!(serde_json::from_str::<serde_json::Value>(&body2).unwrap()["messages"].as_array().unwrap().is_empty());
}
```
(For the caas reply, reuse the send-tool call pattern from `tests/bus.rs`; the DM room name is `dm:caas|raven`.)

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement** — handler outline:
```rust
pub(crate) async fn receive(State(app): State<App>, headers: HeaderMap, Query(q): Query<ReceiveQuery>) -> Response {
    if !origin_ok(&headers) { return StatusCode::FORBIDDEN.into_response(); }
    let Some(lease) = token_lease(&app, &headers).await else { return StatusCode::UNAUTHORIZED.into_response(); };
    let after = q.after.unwrap_or(0);
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let timeout = std::time::Duration::from_secs(q.timeout.unwrap_or(30).clamp(1, 50));
    // Ack: advance every member room's cursor to `after` (monotonic MAX).
    for room in app.store.member_rooms(&lease.name).await.unwrap_or_default() {
        let _ = app.store.set_cursor(&room, &lease.name, after).await;
    }
    let read = |app: App, name: String| async move {
        app.store.undelivered_for_participant(&name, limit).await.unwrap_or_default()
    };
    let mut msgs = read(app.clone(), lease.name.clone()).await;
    if msgs.is_empty() {
        let mut rx = lease.rx.lock().await;
        if tokio::time::timeout(timeout, rx.recv()).await.is_ok() {
            while rx.try_recv().is_ok() {} // drain the wakeup buffer
            drop(rx);
            msgs = read(app.clone(), lease.name.clone()).await;
        }
    }
    let cursor = msgs.last().map(|m| m.id).unwrap_or(after);
    Json(ReceiveResponse { messages: msgs.into_iter().map(MessageDto::from).collect(), cursor }).into_response()
}
```
Route: `.route("/api/participants/receive", get(participants::receive))`. `token_lease` is a shared helper wrapping the `x-participant-token` → `touch` lookup used by send/receive/resume.

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Commit** — `git commit -m "feat: HTTP participant long-poll receive"`.

---

## Task 8: Resume — relayer-only

**Files:** Modify `src/web/participants.rs`, `src/web/mod.rs`. Test: `tests/participant.rs`.

**Interfaces:** `POST /api/participants/resume` body `{ room }`; `204` on success, `403` for a plain lease, `401` for an unknown token.

- [ ] **Step 1: Write the failing test**
```rust
#[tokio::test]
async fn a_plain_participant_cannot_resume() {
    let (_d, port, _p) = common::start_bus_with_participants_dir(Default::default()).await;
    let (_s, rb) = post_json_headers(port, "/api/participants", json!({"name":"raven"}), &[]).await;
    let token = serde_json::from_str::<serde_json::Value>(&rb).unwrap()["token"].as_str().unwrap().to_string();
    let (status, _b) = post_json_headers(port, "/api/participants/resume",
        json!({"room":"x"}), &[("X-Participant-Token",&token)]).await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn a_relayer_can_resume() {
    let cfg = claude_bus::bus::participant::ParticipantConfig { relayer_secret: Some("s".into()), ..Default::default() };
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    let (_s, rb) = post_json_headers(port, "/api/participants",
        json!({"name":"raven"}), &[("X-Relayer-Secret","s")]).await;
    let token = serde_json::from_str::<serde_json::Value>(&rb).unwrap()["token"].as_str().unwrap().to_string();
    let (status, _b) = post_json_headers(port, "/api/participants/resume",
        json!({"room":"x"}), &[("X-Participant-Token",&token)]).await;
    assert_eq!(status, 204);
}
```

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement**
```rust
pub(crate) async fn resume(State(app): State<App>, headers: HeaderMap, Json(body): Json<ResumeRequest>) -> Response {
    if !origin_ok(&headers) { return StatusCode::FORBIDDEN.into_response(); }
    let Some(lease) = token_lease(&app, &headers).await else { return StatusCode::UNAUTHORIZED.into_response(); };
    if !lease.relayer { return StatusCode::FORBIDDEN.into_response(); }
    app.guards.reset(&body.room).await;
    let _ = app.store.append_event("resumed", Some(&lease.name), Some(&body.room),
        serde_json::json!({ "via": "participant_resume" })).await;
    StatusCode::NO_CONTENT.into_response()
}
```
Route: `.route("/api/participants/resume", post(participants::resume))`.

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Commit** — `git commit -m "feat: relayer-only HTTP participant resume"`.

---

## Task 9: Presence hysteresis — lease expiry sweeper

Detach expired leases and emit `agent_disconnected`, so a lease that stops polling goes offline without flapping on the poll cadence.

**Files:** Modify `src/bus/mod.rs` (`serve_on_full`: spawn the sweeper). Test: `tests/participant.rs`.

**Interfaces:** Consumes `Leases::expired`, `Registry::{detach, notify_presence}`, `Store::append_event`.

- [ ] **Step 1: Write the failing test** (short TTL so expiry is fast):
```rust
#[tokio::test]
async fn a_lease_that_stops_polling_goes_offline() {
    let cfg = claude_bus::bus::participant::ParticipantConfig {
        lease_ttl: std::time::Duration::from_millis(150), ..Default::default() };
    let (_d, port, _p) = common::start_bus_with_participants_dir(cfg).await;
    post_json_headers(port, "/api/participants", json!({"name":"raven"}), &[]).await;
    assert!(common::agent_is_online(port, "raven").await);
    assert!(common::wait_until(|| async { !common::agent_is_online(port, "raven").await }).await,
        "raven should go offline after its lease expires");
}
```

- [ ] **Step 2: Run, verify fail** (raven stays online — no sweeper yet).

- [ ] **Step 3: Implement the sweeper** in `serve_on_full`, after the event-relay task, mirroring its structure (clone only what's needed):
```rust
    {
        let registry = app.registry.clone();
        let store = app.store.clone();
        let leases = app.participants.clone();
        let ttl = leases.cfg().lease_ttl;
        tokio::spawn(async move {
            // Sweep at a fraction of the TTL so expiry is detected promptly.
            let mut tick = tokio::time::interval((ttl / 4).max(std::time::Duration::from_millis(25)));
            loop {
                tick.tick().await;
                for (_token, name) in leases.expired().await {
                    registry.detach(&name).await;
                    registry.notify_presence(crate::proto::FromBus::Presence {
                        name: name.clone(), host: "http".into(), online: false,
                        last_seen: crate::store::now_ms() }).await;
                    let _ = store.append_event("agent_disconnected", Some(&name), None,
                        serde_json::json!({ "reason": "lease_expired" })).await;
                }
            }
        });
    }
```
(`app.store` is `Arc<Store>`, so `.clone()` is cheap. Confirm `notify_presence`/`FromBus::Presence` field names against `registry.rs`.)

- [ ] **Step 4: Run, verify pass.**

- [ ] **Step 5: Commit** — `git commit -m "feat: expire idle HTTP participant leases"`.

---

## Task 10: Docs, changelog, final gate

**Files:** Modify `CHANGELOG.md`, `README.md` (mention the HTTP participant surface + `--relayer-secret`/`--reserve-name`), `docs/DEPLOY.md` (if it documents flags).

- [ ] **Step 1:** Add a `CHANGELOG.md` entry under the unreleased/next section describing HTTP participation, the flags, and that authority is session-level via the secret.
- [ ] **Step 2:** Add a short README subsection: base URL, the four endpoints, the token header, and that a plain participant is bot-only while a secret-proven lease carries authority. Link the spec.
- [ ] **Step 3: Full gate** — `cargo +nightly fmt && cargo +stable clippy --all-targets --all-features -- -D warnings && cargo +stable test --locked`. Expected: all PASS.
- [ ] **Step 4: Commit** — `git commit -m "docs: document the HTTP participant surface"`.

---

## Self-review notes

- **Spec coverage:** register/receive/send/resume (Tasks 5–8); authority session-level via secret (Tasks 2, 5, 6); cursor mapping + paging loss-safety (Task 3 + Task 7's ack-then-read); presence hysteresis + expiry (Tasks 5, 9); same-origin guard (every endpoint task); reserved names (Tasks 4, 5); `--relayer-secret`/`--reserve-name` (Task 1); defaults table (Task 7 clamps, Task 1 TTL). `join` is explicitly deferred (spec + here). No HTTP `Unread` machinery (spec says the cursor replaces it) — not built.
- **Type consistency:** `Authority{human_present,relayer}` (Task 2) used identically in the WS call site and the HTTP send (Task 6). `SendOutcome` (Task 6) is the single typed result the WS arm and the HTTP DTO both format. `LeaseHandle{name,relayer,rx}` (Task 4) consumed unchanged by send/receive/resume. `undelivered_for_participant(agent,limit)` (Task 3) is what Task 7 calls after the ack.
- **Deviation logged:** participant DTOs do not derive `ts_rs::TS` (machine-to-machine, not SPA types) — stated in Global Constraints and Task 5.
- **Follow-ups (out of scope):** CSPRNG token (Task 4 note), `POST /join`, secret rotation without restart.
