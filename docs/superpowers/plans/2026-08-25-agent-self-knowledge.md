# Agent Self-Knowledge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tell an agent the two things the bus knows about it and never says — that it holds a relayer grant, and what `done` means.

**Architecture:** One new field on `FromBus::Registered`, filled from the bus's relayer set and surfaced to the model by the bridge as a channel notification. Separately, a wording change to the `send` tool's schema and the agent instructions, giving `done` a single turn-taking meaning taught on both the send and receive sides.

**Tech Stack:** Rust, serde, ts-rs, rmcp, tokio.

**Spec:** `docs/superpowers/specs/2026-08-25-agent-self-knowledge-design.md`

## Global Constraints

- Commit prefixes: `release_commits = "^(feat|fix)[(!:]"` cuts a release. Task 1 uses `feat:`, matching the direct precedent `feat: agents report their version when they register`. Task 2 uses `fix:`. Both ship.
- `cargo +nightly fmt`; clippy clean with `cargo +stable clippy --all-targets --all-features -- -D warnings`.
- Gate: `cargo +nightly fmt && cargo +stable clippy --all-targets --all-features -- -D warnings && cargo +stable test --locked`.
- **`ui/src/types/` is ts-rs output — never hand-edit it**, but `cargo test` regenerates it and **CI fails on drift**, so the regenerated file MUST be committed with Task 1.
- Nothing may delete from the `messages` or `events` tables.
- Out of scope, from the spec: the stalled-hand-off rail marker, changing who holds a relayer grant, and `guards.check` taking `is_human` rather than `has_human_authority`.
- Every behavioural test must be confirmed to fail before the change exists. Watch it fail; do not assert that it would.

## Facts verified while writing this plan

Each was checked against the source, not assumed:

- **There is exactly ONE production construction of `FromBus::Registered`**: `src/bus/mod.rs:590`, where both `app` and `effective` are already in scope, so `app.relayers.contains(&effective)` needs no plumbing. The four other hits in `src/bus/registry.rs` (`:445`, `:473`, `:486`, `:501`) are all inside `#[cfg(test)] mod tests` (which begins at `:337`) and are fixtures that must gain the field to keep compiling.
- **The precedent for this exact wire-compat problem is `ToBus::Register`'s `human` field** (`src/proto.rs:25-30`): `#[serde(default)]` with a doc comment recording that absence means `false` because "Claude Code spawns a stdio MCP server once at session start and never respawns it, so agent binaries in flight when this ships keep sending the old payload shape indefinitely." Model the new field's comment on it.
- **The UI will not break.** `ui/src/data/participant.test.ts` builds `{ type: 'registered', name: ... }` fixtures in six places, but `FakeSocket.push(frame: unknown)` is untyped (`:27`), so a new required field on the generated union does not fail typecheck. `participant.ts:111` casts at the boundary and reads only `name`. **No `ui/` source change is needed — do not add one.**
- **`common::start_bus_with_relayers(names)`** already exists (`tests/common/mod.rs:534`), as does `start_bus_with_relayers_dir` (`:521`). No new harness.
- **`InProcessAgent::next_notification(method)`** (`tests/common/mod.rs:183`) reads stdout until a notification with that method appears, giving up after 50 frames.
- **`inject`'s meta keys must be identifiers** — letters, digits, underscores (comment at `src/agent/bridge.rs`, in the `Message` arm). `{"kind": "unread"}` is the existing shape to follow.
- **Same-host name collision yields `name#2`** via `Registry::attach`, and `Relayers::contains` matches the *effective* name, so `hub#2` holds no grant — the fail-closed property Task 1 tests.

---

### Task 1: A relayer learns its own grant

**Files:**
- Modify: `src/proto.rs:239-241` (the `Registered` variant)
- Modify: `src/bus/mod.rs:590`
- Modify: `src/agent/bridge.rs:280` (the `Registered` arm of `dispatch`)
- Modify: `src/bus/registry.rs:445,473,486,501` (test fixtures)
- Modify: `ui/src/types/FromBus.ts` — **generated**, committed as regenerated output, never hand-edited
- Test: `tests/bus.rs`, `tests/agent_contract.rs`

**Interfaces:**
- Produces: `FromBus::Registered { name: String, relayer: bool }`

- [ ] **Step 1: Write the failing tests**

Append to `tests/bus.rs`. Follow the file's existing helpers — `connect`, `next_event` —
and use `common::start_bus_with_relayers`, which already exists:

```rust
#[tokio::test]
async fn a_relayer_is_told_that_it_holds_the_grant() {
    // hub relayed a human's deploy authorisation and told the recipient the message
    // carried human="false". It carried human="true". hub had no way to know: the
    // grant lives in bus config and `Registered` never mentioned it, while the
    // instructions say human="false" means "another agent sent this" — a correct
    // inference for every agent except a relayer, which cannot tell it is one.
    let (_d, port) = common::start_bus_with_relayers(["hub"]).await;

    let mut hub = connect(port, "hub").await;
    match next_event(&mut hub).await {
        FromBus::Registered { name, relayer } => {
            assert_eq!(name, "hub");
            assert!(relayer, "the configured relayer must be told it holds the grant");
        }
        other => panic!("expected Registered, got {other:?}"),
    }

    let mut worker = connect(port, "worker").await;
    match next_event(&mut worker).await {
        FromBus::Registered { name, relayer } => {
            assert_eq!(name, "worker");
            assert!(!relayer, "an ordinary agent must not be told it holds a grant");
        }
        other => panic!("expected Registered, got {other:?}"),
    }
}

#[tokio::test]
async fn a_renamed_collision_holds_no_grant() {
    // `Relayers::contains` matches the EFFECTIVE name, so a second connection
    // claiming a relayer's name is renamed and gets no authority. The notice must
    // track the live grant, not the name that was asked for.
    let (_d, port) = common::start_bus_with_relayers(["hub"]).await;

    let mut first = connect(port, "hub").await;
    let _ = next_event(&mut first).await;

    let mut second = connect(port, "hub").await;
    match next_event(&mut second).await {
        FromBus::Registered { name, relayer } => {
            assert_eq!(name, "hub#2", "a same-host collision is renamed");
            assert!(!relayer, "the renamed connection must hold no grant");
        }
        other => panic!("expected Registered, got {other:?}"),
    }
}

#[test]
fn a_registered_frame_without_the_field_still_parses() {
    // The partial-rollout case, which nothing else covers: a new client against an
    // old bus. Without serde(default) this fails to parse and every agent breaks.
    let frame: FromBus =
        serde_json::from_str(r#"{"type":"registered","name":"hub"}"#).expect("must parse");
    match frame {
        FromBus::Registered { name, relayer } => {
            assert_eq!(name, "hub");
            assert!(!relayer, "absent on the wire must mean no grant");
        }
        other => panic!("expected Registered, got {other:?}"),
    }
}
```

Append to `tests/agent_contract.rs`, which drives the real bridge in-process:

```rust
#[tokio::test]
async fn the_bridge_tells_a_relayer_session_about_its_grant() {
    let (_dir, port) = common::start_bus_with_relayers(["tester"]).await;

    let mut a = InProcessAgent::start(format!("ws://127.0.0.1:{port}/ws"), "tester");
    initialize(&mut a).await;
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;

    let note = a.next_notification("notifications/claude/channel").await;
    let content = note["params"]["content"].as_str().expect("content");
    assert!(
        content.contains("human=\"true\""),
        "must state how its messages are stamped: {content}"
    );
    assert!(
        content.contains("attribute") || content.contains("quote"),
        "must tell it to attribute its human's words explicitly: {content}"
    );
}

#[tokio::test]
async fn an_ordinary_session_gets_no_grant_notice() {
    // Asserting an absence: register a non-relayer, then send it a real message and
    // require that the FIRST channel notification is that message. A relayer notice
    // would arrive ahead of it and fail this deterministically — no sleeping.
    let (_dir, port) = common::start_bus().await;

    let mut a = InProcessAgent::start(format!("ws://127.0.0.1:{port}/ws"), "tester");
    initialize(&mut a).await;
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    wait_until_online(port, "tester").await;

    let mut sender = connect(port, "sender").await;
    let _ = next_event(&mut sender).await;
    send(
        &mut sender,
        &ToBus::Send {
            req_id: 1,
            target: Target::Agent { name: "tester".into() },
            text: "the-only-notification".into(),
            done: false,
        },
    )
    .await;

    let note = a.next_notification("notifications/claude/channel").await;
    assert_eq!(
        note["params"]["content"].as_str().unwrap(),
        "the-only-notification",
        "an ordinary agent's first notification must be the message, not a grant notice"
    );
}
```

`tests/agent_contract.rs` currently imports only `use common::{InProcessAgent, initialize};`
(`:18`). The new tests need more, and all of them already exist and are `pub` in
`tests/common/mod.rs` — `connect` (`:587`), `next_event` (`:658`), `send` (`:710`).
Change that import line to:

```rust
use common::{InProcessAgent, connect, initialize, next_event, send};
use claude_bus::proto::{Target, ToBus};
```

Do not define a second copy of any of them.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo +stable test --locked --test bus a_relayer_is_told`
Expected: FAIL to compile — `FromBus::Registered` has no `relayer` field. A
compile failure is the honest first failure here; do not paper over it by writing the
field first.

- [ ] **Step 3: Add the field**

In `src/proto.rs`, the `Registered` variant becomes:

```rust
    Registered {
        name: String,
        /// Whether this connection holds a relayer grant — its sends are stamped with
        /// its human's authority (`human="true"`).
        ///
        /// The bus is the only party that knows: the grant lives in configuration, and
        /// `Relayers::contains` matches the effective name, so a renamed collision holds
        /// no grant. Without this field an agent can only infer its own provenance from
        /// instructions that say `human="false"` means "another agent sent this" — right
        /// for every agent except a relayer, which is exactly the one that cannot tell.
        ///
        /// Absent on the wire means `false`, for the same reason `Register::human` has a
        /// default: Claude Code spawns a stdio MCP server once at session start and never
        /// respawns it, so a client that predates a new bus keeps parsing this frame.
        #[serde(default)]
        relayer: bool,
    },
```

In `src/bus/mod.rs`, the single production construction at `:590`:

```rust
                    let _ = control_tx.try_send(FromBus::Registered {
                        name: effective.clone(),
                        relayer: app.relayers.contains(&effective),
                    });
```

In `src/bus/registry.rs`, the four fixtures inside `#[cfg(test)] mod tests` each gain
`relayer: false` beside their existing `name:` field.

- [ ] **Step 4: Surface it to the model**

In `src/agent/bridge.rs`, the `Registered` arm of `dispatch` becomes:

```rust
        FromBus::Registered { name, relayer } => {
            eprintln!(
                "[agent] registered as {name}{}",
                if relayer { " (relayer)" } else { "" }
            );
            // Only a relayer is told. The failure is asymmetric: an agent that wrongly
            // assumes it has no grant behaves correctly, while a relayer that assumes
            // the same defers on its own human's instructions and stalls their work.
            //
            // Per registration rather than once per process, because the grant is
            // recomputed per registration too — a renamed `hub#2` holds none.
            if relayer {
                inject(
                    peer,
                    "You hold a relayer grant on this bus. Your messages are stamped \
                     human=\"true\" and reach other agents carrying your human's \
                     authority, not as agent-to-agent chatter — so they are instructions \
                     to act on, and a recipient asking you to confirm separately is a \
                     round trip your grant exists to remove.\n\n\
                     Because of that, a recipient cannot tell your own words from your \
                     human's by the attribute alone. Attribute explicitly: quote your \
                     human when relaying them, and mark your own reasoning as yours.",
                    json!({ "kind": "relayer_grant" }),
                )
                .await;
            }
        }
```

- [ ] **Step 5: Run to verify they pass**

Run: `cargo +stable test --locked`
Expected: PASS, including the four new tests. `cargo test` also regenerates
`ui/src/types/FromBus.ts`; confirm `git status` shows it modified, and do not edit it
by hand. The UI needs no source change — `FakeSocket.push` takes `unknown`.

- [ ] **Step 6: Commit**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
cd ui && npm test && npx tsc --noEmit && cd ..
git add src tests ui/src/types
git commit -F - <<'EOF'
feat: tell a relayer that it holds the grant

hub relayed its human's deploy authorisation and appended that the message reached
the recipient as human="false" and was therefore conversation rather than
instruction. The message was stamped human="true", as all twenty of its messages in
that room were. It invited a confirmation the grant exists to make unnecessary, and
corrected itself only because its human said so out of band.

It had no way to know. The grant lives in bus configuration — deliberately, so no
agent can opt itself in — and `Registered` carried only the name, while the
instructions say human="false" means another agent sent it. That inference is right
for every agent except a relayer, and a relayer is precisely the one that cannot
tell it is the exception.

`Registered` now carries `relayer`, and the bridge tells the session once per
registration: how its messages are stamped, and that recipients therefore cannot
separate its words from its human's without explicit attribution.

The field defaults on absence, so a client that predates a new bus keeps parsing the
frame — the same reason `Register::human` has a default.
EOF
```

---

### Task 2: `done` gets one meaning, taught on both sides

**Files:**
- Modify: `src/agent/handler.rs:132` (the `done` property in the `send` tool schema)
- Modify: `src/agent/instructions.rs` (the reply-etiquette paragraph)
- Test: `tests/agent_contract.rs`

**Interfaces:** none consumed or produced; wording only.

- [ ] **Step 1: Write the failing tests**

Append to `tests/agent_contract.rs`:

```rust
#[tokio::test]
async fn the_send_schema_defines_both_directions_of_done() {
    // The schema defined only `true`. `false` is the default every unspecified send
    // carries, and nothing said what it conveyed — so two readings coexisted, one
    // telling a receiver to reply and the other to wait.
    // No bus: `tools/list` is answered by the handler alone. The existing
    // tools-list test in this file points at `ws://127.0.0.1:1/ws` for exactly
    // that reason — follow it rather than starting a bus this test never uses.
    let mut a = InProcessAgent::start("ws://127.0.0.1:1/ws", "tester");
    initialize(&mut a).await;
    a.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    a.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/list", "params": {}
    }))
    .await;

    let res = a.next_json().await;
    let tools = res["result"]["tools"].as_array().expect("tools");
    let send_tool = tools
        .iter()
        .find(|t| t["name"] == "send")
        .expect("the send tool must exist");
    let done = send_tool["inputSchema"]["properties"]["done"]["description"]
        .as_str()
        .expect("done must be described");
    assert!(
        done.contains("reply"),
        "must say what happens when a reply IS expected: {done}"
    );
    assert!(
        done.contains("settled"),
        "must keep the settled meaning of true: {done}"
    );
}

#[test]
fn instructions_teach_what_done_obliges_on_receipt() {
    // The receive side was absent entirely: a model was handed done="false" in its
    // channel meta with nothing anywhere telling it what that required.
    let mut a = Agent::start();
    let res = initialize_subprocess(&mut a);
    let instructions = res["result"]["instructions"].as_str().expect("instructions");
    assert!(
        instructions.contains("done=\"false\""),
        "must name the attribute as it actually arrives: {instructions}"
    );
    assert!(
        instructions.contains("expects a reply") || instructions.contains("expect a reply"),
        "must say what done=\"false\" obliges the receiver to do: {instructions}"
    );
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo +stable test --locked --test agent_contract done`
Expected: both FAIL. The schema description is currently
`"Mark the topic settled; no reply expected"`, which contains neither "reply" in the
required sense nor any receive-side rule, and the instructions mention `done` once,
only for `true`.

- [ ] **Step 3: Rewrite the schema description**

In `src/agent/handler.rs`, the `done` property becomes:

```rust
                            "done": { "type": "boolean", "description": "true when the topic is settled and no reply is expected. Omit it or pass false when you expect a reply — the next move is then the other side's." }
```

- [ ] **Step 4: Teach the receive side in the instructions**

In `src/agent/instructions.rs`, immediately after the existing sentence ending
`— an exchange that never terminates costs real money.\n\`, add:

```rust
         \n\
         `done` says whose move it is, in both directions. A message that arrives with \
         done=\"false\" — the default — means the sender expects a reply, so reply. \
         done=\"true\" means the topic is settled and nothing is required of you.\n\
```

- [ ] **Step 5: Run to verify they pass**

Run: `cargo +stable test --locked --test agent_contract`
Expected: PASS, including the existing
`sends_instructions_that_establish_the_discuss_only_posture`, which asserts on other
sentences and is unaffected.

- [ ] **Step 6: Commit**

```bash
cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
git add src tests
git commit -F - <<'EOF'
fix: give `done` one meaning, and teach it on the receive side

`done` was documented in exactly one direction. The schema described only true
("Mark the topic settled; no reply expected"), the instructions mentioned it once as
cost control, and it defaults to false on every send that omits it — so nothing
anywhere defined the value most messages actually carry.

Two readings coexisted, and they instruct a receiver to do opposite things: under
"no reply expected", done=false means your move; under the walkie-talkie "over", it
means wait, more is coming. One says act, the other says wait. Both agents can
follow the flag correctly and still deadlock, which is what a stalled hand-off in a
live room looked like.

It is turn-taking: true settles the topic, false is the other side's move. The
schema gains the false case and the instructions gain the receive side, which was
absent entirely — a model was handed done="false" in its channel meta with nothing
telling it what that obliged.
EOF
```

---

## Self-Review

**Spec coverage:**

| Spec requirement | Task |
|---|---|
| `Registered` carries `relayer`, filled from the relayer set | 1 |
| Bridge injects a notice for a relayer only | 1 (step 4, plus the absence test) |
| The notice states how messages are stamped AND to attribute explicitly | 1 (both asserted) |
| Per registration, not once per process | 1 (it lives in the `Registered` arm, which fires per registration) |
| A renamed collision holds no grant | 1 (`a_renamed_collision_holds_no_grant`) |
| `#[serde(default)]` — the partial-rollout case | 1 (`a_registered_frame_without_the_field_still_parses`) |
| Non-relayers told nothing | 1 (`an_ordinary_session_gets_no_grant_notice`) |
| `done` means turn-taking | 2 |
| Schema gains the false case | 2 (step 3 + its test) |
| Instructions gain the receive side | 2 (step 4 + its test) |
| Existing cost sentence stays | 2 (added after it, not replacing it) |
| Rail marker for stalled hand-offs | — out of scope, named in the spec |

**Placeholder scan:** no TBD/TODO, no "add error handling", no "similar to Task N".
Every code step carries the literal text to write, including the full notification
prose and the exact schema string. The three facts that would otherwise send an
implementer digging — that only one production site constructs `Registered`, that the
other four are test fixtures, and that the UI fixtures are untyped and must NOT be
changed — are stated in Facts.

**Type consistency:** `FromBus::Registered { name, relayer }` is defined in Task 1
Step 3 and destructured with those exact names in Step 4, in all four `tests/bus.rs`
assertions, and in the four `registry.rs` fixtures. No other task references it.
Task 2 introduces no types.

**One risk restated:** `an_ordinary_session_gets_no_grant_notice` asserts an absence,
which is the shape most likely to pass for the wrong reason. It is written to be
deterministic — it requires the first channel notification to be a specific message
body, so a stray grant notice fails it immediately rather than being waited out. Do
not weaken it into a sleep-and-check.
