# DM Target Validation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The `agents` tool stops reporting a name that does not exist, and `Send` refuses a DM to an agent that has never registered.

**Architecture:** Two independent changes to the Rust bus. One is a format string in the MCP tool layer; the other is an existence check in the send path, before the delivery guards, mirroring the existing `room_exists` precedent.

**Tech Stack:** Rust, axum 0.8, sqlx/SQLite, rmcp.

**Spec:** `docs/superpowers/specs/2026-08-16-dm-target-validation-design.md`

## Global Constraints

- Commit prefixes: `release_commits = "^(feat|fix)[(!:]"` in `release-plz.toml` cuts a release on `feat:`/`fix:`. **This is a bug fix and should ship**, so Task 2 uses `fix:`. Task 1 uses `fix:` as well — it is the same defect. Nothing else in this plan cuts a release.
- `cargo +nightly fmt`; clippy clean with `cargo +stable clippy --all-targets --all-features -- -D warnings`.
- **Nothing may delete from `messages` or `events`.**
- Gate: `cargo +nightly fmt && cargo +stable clippy --all-targets --all-features -- -D warnings && cargo +stable test --locked`.
- Nothing under `ui/` changes in this plan. `ui/src/types/` is ts-rs output — never hand-edit it.
- Every behavioural test must be confirmed to fail before the change exists. Watch it fail; do not assert that it would.

## Facts verified while writing this plan

Each was checked against the source, not assumed:

- **The composition exists in exactly one place**: `src/agent/handler.rs:360`. The web UI renders name and host into separate table cells (`src/web/mod.rs:279,283`), and the `rooms` tool prints `r.members.join(", ")` verbatim — both already correct.
- **The insertion point for validation**: `src/bus/commands.rs:103` is `let room = rooms::resolve(&target, me);`, and the guard check begins at line 130 with `let cleared_pause = match app.guards.check(...)`. The new check goes between them.
- **`Store::room_exists` at `src/store/mod.rs:385`** is the precedent to mirror — a six-line `SELECT 1 ... fetch_optional ... Ok(row.is_some())`.
- **The refusal pattern** in this arm is `control_tx.try_send(FromBus::Error { req_id: Some(req_id), message })` followed by `return;`.
- **The existing `agents`-tool test** (`tests/agent_contract.rs::the_agents_tool_reports_each_agents_version`) asserts only on version strings and the literal `unknown` — **it does not assert the `@` form**, so Task 1 does not break it.

---

### Task 1: The `agents` tool reports the name, not a composition

**Files:**
- Modify: `src/agent/handler.rs:358-366`
- Test: `tests/agent_contract.rs`

**Interfaces:** none consumed or produced; this is a rendering change.

- [ ] **Step 1: Write the failing test**

Append to `tests/agent_contract.rs`, following the shape of
`the_agents_tool_reports_each_agents_version` directly above it (it uses
`InProcessAgent`, `initialize`, `call_tool`, and `wait_until_online` — reuse them
rather than inventing a second harness):

```rust
/// The `agents` tool used to render `format!("{}@{}", a.name, a.host)`, which is
/// a string no agent is registered under. An agent following the documented
/// discovery path read that name and sent to it; `Send` accepted it, derived a
/// DM room from it, and enrolled a member that could never connect. Messages
/// queued forever behind a 204.
///
/// The name field must be exactly what a caller can address.
#[tokio::test]
async fn the_agents_tool_reports_the_addressable_name_not_name_at_host() {
    let (_dir, port) = common::start_bus().await;

    let mut a = InProcessAgent::start(format!("ws://127.0.0.1:{port}/ws"), "solo");
    initialize(&mut a).await;
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    wait_until_online(port, "solo").await;

    let text = call_tool(&mut a, 60, "agents", serde_json::json!({})).await;

    assert!(
        text.contains("solo"),
        "the agent must appear at all: {text}"
    );
    assert!(
        !text.contains("solo@"),
        "must not compose a name@host that nothing is registered under — that string \
         is what a caller will paste into `send`: {text}"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo +stable test --locked --test agent_contract the_agents_tool_reports_the_addressable_name`
Expected: FAIL on the second assertion — the output currently contains `solo@`.

- [ ] **Step 3: Change the format**

In `src/agent/handler.rs`, the `"agents"` arm currently renders:

```rust
                            format!(
                                "{}@{} — {} — {}",
                                a.name,
                                a.host,
```

Change the format string to put the name and host in separate fields:

```rust
                            format!(
                                "{} — {} — {} — {}",
                                a.name,
                                a.host,
```

Add a comment above the `format!` recording why, because the old form looks
harmless and someone will be tempted to "tidy" it back:

```rust
                        // `name` and `host` are separate fields, deliberately. Composing
                        // them reads as a single name — and it is the shape
                        // `Registry::attach` really does hand out on a cross-host
                        // collision, so there is nothing to tell the two apart. An agent
                        // that read the composed form and sent to it created a DM room
                        // enrolling a member that had never registered, and its messages
                        // queued forever.
                        //
                        // When a name genuinely is qualified this renders
                        // `foo@bar — bar — …`. The repetition is the honest outcome; a
                        // rule to suppress it would reintroduce the ambiguity.
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo +stable test --locked --test agent_contract`
Expected: PASS, including `the_agents_tool_reports_each_agents_version`, which
asserts only on version strings and is unaffected.

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
git add src tests
git commit -F - <<'EOF'
fix: report the addressable agent name from the agents tool

The tool rendered `{name}@{host}`, which is a string no agent on this bus is
registered under. An agent following the documented discovery path read that and
sent to it — and `Send` accepted it, derived a DM room from it, and enrolled a
member that could never connect, so its messages queued forever behind a 204.

Nobody typed the name by hand. The tool that exists to answer "what is this agent
called" answered it wrongly.

Name and host are separate fields now. When a name genuinely is qualified — which
`Registry::attach` does produce on a cross-host collision — this renders
`foo@bar — bar — …`; the repetition is the honest outcome, and a rule to suppress
it would reintroduce exactly the ambiguity being removed.
EOF
```

---

### Task 2: `Send` refuses an agent target that has never registered

**Files:**
- Modify: `src/store/mod.rs` (beside `room_exists` at :385)
- Modify: `src/bus/commands.rs` (between :103 and :130)
- Test: `tests/bus.rs`

**Interfaces:**
- Produces: `Store::agent_exists(&self, name: &str) -> anyhow::Result<bool>`

- [ ] **Step 1: Write the failing tests**

Append to `tests/bus.rs`. Read the file's existing helpers first — `start_bus`,
`connect`, `send`, `next_event` — and follow them; do not add a second harness.

```rust
#[tokio::test]
async fn a_dm_to_an_agent_that_never_registered_is_refused_and_creates_no_room() {
    // The bug this exists for: the `agents` tool used to report `name@host`, an
    // agent sent to that, and the bus created a room enrolling a member that
    // could never connect. The room's ABSENCE matters as much as the error —
    // a refusal that still created the room would leave the same ghost behind.
    let (_d, port) = start_bus().await;
    let mut a = connect(port, "caas").await;
    let _ = next_event(&mut a).await; // Registered

    send(
        &mut a,
        &ToBus::Send {
            req_id: 1,
            target: Target::Agent {
                name: "homelab-health@hardac".into(),
            },
            text: "hello".into(),
            done: false,
        },
    )
    .await;

    match next_event(&mut a).await {
        FromBus::Error { req_id, message } => {
            assert_eq!(req_id, Some(1));
            assert!(
                message.contains("homelab-health@hardac"),
                "the error must name the target that was refused: {message}"
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }

    let rooms: serde_json::Value = common::get_json(port, "/api/rail").await;
    assert!(
        rooms["rooms"].as_array().unwrap().is_empty(),
        "a refused DM must create no room: {rooms}"
    );
}

#[tokio::test]
async fn the_refusal_suggests_the_bare_name_when_the_target_looks_qualified() {
    // The observed failure was a NEARLY right name. "no such agent" would leave
    // the sender no better off; naming the candidate closes the loop.
    let (_d, port) = start_bus().await;
    let mut health = connect(port, "homelab-health").await;
    let _ = next_event(&mut health).await;
    let mut a = connect(port, "caas").await;
    let _ = next_event(&mut a).await;

    send(
        &mut a,
        &ToBus::Send {
            req_id: 2,
            target: Target::Agent {
                name: "homelab-health@hardac".into(),
            },
            text: "hello".into(),
            done: false,
        },
    )
    .await;

    match next_event(&mut a).await {
        FromBus::Error { message, .. } => assert!(
            message.contains("did you mean") && message.contains("\"homelab-health\""),
            "must name the real agent behind the qualified string: {message}"
        ),
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_dm_to_a_known_but_offline_agent_still_queues() {
    // THE REGRESSION THAT WOULD MATTER MOST. Queuing for an offline agent is the
    // entire point of the bus. "Unknown" must mean never registered, never
    // "not currently connected" — a liveness check here would destroy it.
    let (_d, port) = start_bus().await;
    let mut gone = connect(port, "gone").await;
    let _ = next_event(&mut gone).await;
    drop(gone);
    assert!(
        common::wait_until(|| async { !common::agent_is_online(port, "gone").await }).await,
        "the agent never went offline within the deadline"
    );

    let mut a = connect(port, "caas").await;
    let _ = next_event(&mut a).await;
    send(
        &mut a,
        &ToBus::Send {
            req_id: 3,
            target: Target::Agent { name: "gone".into() },
            text: "hello".into(),
            done: false,
        },
    )
    .await;

    match next_event(&mut a).await {
        FromBus::Reply {
            result: ReplyResult::Sent { queued_for, .. },
            ..
        } => assert_eq!(queued_for, vec!["gone".to_string()]),
        other => panic!("expected Sent, got {other:?}"),
    }
}
```

The three helpers above are verified, so use them as written:
`wait_until<F, Fut>(f: F)` takes a closure returning a future
(`tests/common/mod.rs:219`), `agent_is_online(port: u16, name: &str) -> bool` is
async (`:425`), and `get_json(port: u16, path: &str) -> serde_json::Value` takes
the port by value, not an address (`:244`).

One thing to know about `agent_is_online`: it connects a throwaway agent named
`probe` to ask the bus, so calling it **registers an agent called `probe`**. That
is harmless here — it creates no rooms, and the only test asserting an empty rail
does not call it — but do not add it to that test without accounting for it.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo +stable test --locked --test bus a_dm_to_an_agent_that_never`
Expected: FAIL — currently the send succeeds and a room is created, so the test
panics with `expected Error, got Reply{...}`.

Confirm `a_dm_to_a_known_but_offline_agent_still_queues` **passes** before the
change. It is a regression guard, not a new behaviour, and it should be green at
both ends. Say so in your report.

- [ ] **Step 3: Add the store lookup**

In `src/store/mod.rs`, beside `room_exists`:

```rust
    /// Whether any agent has ever registered under this exact name.
    ///
    /// Existence, deliberately not liveness. Queuing for an offline agent is the
    /// whole point of the bus, so a `Send` target is checked against this rather
    /// than against the registry — an agent that has gone away still has its row.
    pub async fn agent_exists(&self, name: &str) -> anyhow::Result<bool> {
        let row = sqlx::query("SELECT 1 FROM agents WHERE name = ?1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }
```

- [ ] **Step 4: Validate the target**

In `src/bus/commands.rs`, immediately after `let room = rooms::resolve(&target, me);`
and **before** the `app.guards.check(...)` call:

```rust
            // An unknown DM target is refused here, before the delivery guards.
            // A malformed request must not consume exchange-cap budget, and must
            // not come back as `RateLimited` — that would send the caller away to
            // retry something that can never work.
            //
            // Existence, not liveness: an offline agent still has its row, and
            // queuing for it is the point of the bus. Only `Target::Agent` is
            // checked — rooms auto-create legitimately, and that is how a room
            // comes into being at all.
            if let Target::Agent { name } = &target {
                let known = match app.store.agent_exists(name).await {
                    Ok(known) => known,
                    Err(e) => {
                        eprintln!("could not check whether agent {name} exists: {e}");
                        let _ = control_tx.try_send(FromBus::Error {
                            req_id: Some(req_id),
                            message: "could not verify the target agent".to_string(),
                        });
                        return;
                    }
                };
                if !known {
                    // The observed failure was a nearly-right name, so name the
                    // candidate rather than only the mistake.
                    let suggestion = match name.split_once('@') {
                        Some((bare, _)) if app.store.agent_exists(bare).await.unwrap_or(false) => {
                            format!("; did you mean {bare:?}?")
                        }
                        _ => "; call `agents` for the list".to_string(),
                    };
                    let _ = app
                        .store
                        .append_event(
                            "send_refused",
                            Some(me),
                            None,
                            json!({ "target": name, "reason": "unknown_agent" }),
                        )
                        .await;
                    let _ = control_tx.try_send(FromBus::Error {
                        req_id: Some(req_id),
                        message: format!("no agent named {name:?}{suggestion}"),
                    });
                    return;
                }
            }
```

The event's `room` is `None` deliberately: no room exists, and naming one would
imply otherwise in the events dock.

- [ ] **Step 5: Run to verify they pass**

Run: `cargo +stable test --locked --test bus`
Expected: PASS, all three new tests plus the existing suite.

**Then confirm the offline test would catch a liveness mistake**: temporarily
change `agent_exists` to check the registry's online set instead of the table,
re-run, and confirm `a_dm_to_a_known_but_offline_agent_still_queues` fails.
Restore. Report what you observed — that test is the one guarding the property
most likely to be broken by a well-meaning "improvement".

- [ ] **Step 6: Commit**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
git add src tests
git commit -F - <<'EOF'
fix: refuse a DM to an agent that has never registered

`Send` derived a DM room from the target verbatim and enrolled it with no check
that any such agent exists, so one bad string created a room whose member could
never connect. Messages queued for it forever, the sender got a 204 and
`queued_for: ["<the bad name>"]`, and nothing anywhere reported a problem.

The check runs before the delivery guards: a malformed request must not consume
exchange-cap budget, and must not come back as RateLimited, which would send the
caller away to retry something that can never work.

Existence, not liveness. An offline agent still has its row and queuing for it is
the entire point of the bus; a liveness check here would destroy that. Only
Target::Agent is checked — rooms auto-create legitimately.

The refusal names the likely candidate, because the observed failure was a nearly
right name and "no such agent" would leave the sender no better off. It also
writes a send_refused event: this bug's defining property was invisibility, and
an audit row is what would have surfaced it the first time.
EOF
```

---

### Task 3: Verify against the real bus

**Files:** none — verification only, no commit.

- [ ] **Step 1: Run the gate**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
cd ui && npm test && cd ..
```

The UI suite is unchanged by this plan; run it to confirm that.

- [ ] **Step 2: Build and start a scratch bus**

```bash
cd ui && npm run build && cd ..
cargo build
rm -rf /tmp/claude-bus-dmfix
./target/debug/claude-bus serve --port 7812 --data /tmp/claude-bus-dmfix &
```

Build order is load-bearing — `rust-embed` compiles the UI bundle into the
binary, and a bus already running keeps its old copy.

- [ ] **Step 3: Confirm each of these and report the result for each**

1. Connect two agents, `alpha` and `beta`. `alpha` DMs `beta@somehost` — expect a
   refusal naming `beta` as the candidate, and **no new room** in `/api/rail`.
2. `alpha` DMs `beta` — expect success and a `dm:alpha|beta` room.
3. Disconnect `beta`. `alpha` DMs `beta` again — expect success with
   `queued_for: ["beta"]`. **This is the check that matters**: the fix must not
   have broken queuing for an offline agent.
4. `alpha` DMs `nobody` — expect a refusal pointing at the `agents` tool, since
   there is no bare-name candidate to suggest.
5. A `send_refused` event appears in the events dock for each refusal, with the
   target in its detail and no room attached.
6. Run the `agents` MCP tool against this bus and confirm the output contains no
   composed `@` for an agent whose registered name has none.

- [ ] **Step 4: Commit nothing; report**

Report each check's result, including anything that looked wrong but you could
not attribute.

---

## Self-Review

**Spec coverage:**

| Spec requirement | Task |
|---|---|
| `agents` tool reports name and host separately | 1 |
| Genuinely qualified names still render honestly | 1 (comment + no suppression rule) |
| `Send` refuses an agent target with no row | 2 |
| Check runs **before** the delivery guards | 2 (step 4 placement + comment) |
| Existence, not liveness | 2 (`agent_exists` + the offline regression test) |
| `Target::Room` untouched | 2 (`if let Target::Agent` only) |
| Refusal names the likely candidate | 2 (step 4 `suggestion`) |
| Refusal writes a `send_refused` event | 2 |
| Ghost room left alone | — nothing built; it is hidden via the existing feature |
| Manual pass incl. the offline case | 3 |

**Placeholder scan:** no TBD/TODO, no "add error handling", and **no "check this
before using it"** — the two test-harness details I had hedged on
(`wait_until`'s signature, `agent_is_online`'s shape) were resolved while
reviewing, along with `get_json`'s. All three were already correct as written;
the plan now records that rather than sending an implementer to re-derive it.
Writing "verify before use" into a plan does not remove an error, it relocates it
to someone with less context.

That pass also turned up one thing worth knowing that I had not: `agent_is_online`
registers a throwaway `probe` agent as a side effect of asking. Harmless for these
tests, and now stated.

**Type consistency:** `Store::agent_exists(&str) -> anyhow::Result<bool>` is
defined in Task 2 step 3 and called twice in step 4 (once for the target, once
for the bare-name candidate). `send_refused` is the event kind in both the code
and the Task 3 check. No type is referenced that no task defines.

**One risk restated:** `a_dm_to_a_known_but_offline_agent_still_queues` is the
only guard on the property most likely to be broken by someone "improving" this
check into a liveness test. Task 2 step 5 requires proving it can fail, by
temporarily making the check liveness-based. That proof is the point of the step.
