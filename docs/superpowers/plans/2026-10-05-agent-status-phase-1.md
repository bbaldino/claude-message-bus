# Agent Status (Phase 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every agent has a readable status. The state (working / idle / blocked-on-human) comes from Claude Code hooks, and the agent can add a short line of text. Anyone can read it without sending a message. It is never counted toward the exchange cap, and it is resent after a bus restart.

**Architecture:**
- A hook subcommand (`claude-bus hook <event>`) writes a small per-session state file on the agent's machine. It makes no network calls.
- The agent's bridge (`claude-bus agent`):
  - polls that file and holds the text set by a new `status` tool
  - publishes both through a `tokio::sync::watch` channel
  - sends `ToBus::Status` after every `Register`, and again on every change
- The bus keeps an in-memory map of statuses and works out staleness when the status is read.
- Readers get the status from `/api/agents`, `/api/rail`, the agent detail endpoint, the `agents` MCP tool, a new `claude-bus status` CLI, and the console. The console updates live from a new `FromBus::Status` push.

**Tech Stack:** Rust (axum, tokio, tokio-tungstenite, rmcp, serde, ts-rs); React/TypeScript console in `ui/` (Vite, Vitest).

**Spec:** `docs/superpowers/specs/2026-10-05-agent-status-design.md`

## Global Constraints

- Format Rust with `cargo +nightly fmt`. CI runs `cargo +stable clippy --all-targets --all-features -- -D warnings`.
- TypeScript uses prettier with `{ "singleQuote": true, "semi": false, "printWidth": 100 }` (already in `ui/.prettierrc`). Run `npm run format:check` in `ui/`.
- Add Rust dependencies only with `cargo add`. This plan needs none.
- For multi-letter acronyms, capitalize only the first letter: `HttpClient`, not `HTTPClient`.
- `ts-rs` regenerates `ui/src/types/` during `cargo test`. Commit the regenerated files: CI fails on `git diff --exit-code -- ui/src/types`.
- **CI's Rust job runs with `ui/dist` unbuilt.** Before every push, run `find ui/dist -mindepth 1 ! -name .gitkeep -delete && cargo test`, then `make ui` to restore. No test may depend on whether the bundle is built.
- The text is capped at **200 characters**, truncated on a character boundary.
- The "quiet" threshold is **10 minutes** since the last heartbeat while working.
- The hook rewrites the file for a heartbeat at most every **30 seconds**. The bridge polls the file every **1 second**.
- The hook must **never print to stdout**: `UserPromptSubmit` stdout is injected into the model's context. It must always exit 0, and it must not make network calls.
- New protocol fields need `#[serde(default)]`, because agent binaries in flight are never respawned. New `ToBus` variants are fire-and-forget, with no `req_id`.
- Status is never persisted to SQLite and never passes through `Guards` (the exchange cap).
- The event log records only entering and leaving blocked-on-human, as kind `blocked_on_human` with `{"entered": bool, "reason": ..., "via": ...}`.
- Reserve `waiting_on` (`Option<String>`, `#[serde(default)]`) on the wire and in the tool schema, but ignore it in phase 1.

## Review Focus

1. **Hook stdout and failure modes.** `claude-bus hook` given empty, invalid or very large stdin, with no `session_id`, or with an unwritable state dir must print nothing on stdout, exit 0, and finish in well under a second. The test goes in Task 4.
2. **A new agent connected to an old bus.** An old bus answers `ToBus::Status` with `FromBus::Error { req_id: None }`. The bridge must log it and carry on: `send` and the other tools keep working. The test goes in Task 5.
3. **Hostile or oversized text.** 5,000 characters, embedded newlines, `<script>`. The bus truncates to 200 characters. The `agents` tool renders it on one line, and the console renders it as text, never HTML. The test goes in Task 2, with rendering in Task 3.
4. **Renamed agents.** The second `caas` session becomes `caas#2`. Its status must attach to `caas#2`, not to the original `caas`, and two sessions in the same project directory must not share a state file. The test goes in Task 5.
5. **A deleted agent's status.** After `DELETE /api/agents/{name}`, no reader may show the old status, including when an agent with the same name registers later. The test goes in Task 2.

---

### Task 0: Planning-time spike (manual, no code merged)

The spec's open items, now partly answered by the Claude Code hooks docs:

| Question | From the docs | Still to confirm by hand |
|---|---|---|
| Which Notification payload field means "needs approval"? | Moot. The `Notification` hook's **matcher** filters on the type: `permission_prompt` (fires after a tool approval has waited about 6 seconds), `idle_prompt` (finished about 60 seconds ago, no input since), `elicitation_dialog`, `elicitation_url_dialog`, `agent_needs_input`, and others. We install hooks only for `permission_prompt`, `elicitation_dialog` and `elicitation_url_dialog`, passing the type as an argument, so `idle_prompt` never runs us. No undocumented payload field is needed. | That the matcher syntax `permission_prompt` works as documented. |
| Is `CLAUDE_CODE_SESSION_ID`, as the MCP server sees it, the same as the hook's stdin `session_id`? | **Undocumented.** The bridge already reads that variable at `src/agent/mod.rs`. | Yes, by hand. |
| Does a turn started by a channel (bus) message fire `UserPromptSubmit`? | Undocumented. | Yes, by hand. Phase 1 is fine either way, because `PreToolUse` covers it. |
| How do `PreToolUse` and `PostToolUse` order around a permission prompt? | Undocumented. A separate `PermissionRequest` event exists. | Yes, by hand. |
| Do hooks block the session? | Yes, until they exit. The default timeout is 10 minutes (30 seconds for `UserPromptSubmit`). We set `"timeout": 5`. | — |

**Caveat to tell the human:** `claude-bus launch` passes `--dangerously-skip-permissions` and disallows `AskUserQuestion`. Hub-launched workers therefore never hit a permission prompt or a dialog, so they will never show blocked-on-human from hooks. Only interactive sessions will. Phase 2's `waiting_on: "human"` is what covers a worker that has asked the human something by message.

- [ ] **Step 1: Install a logging hook in a scratch project**

Create `/tmp/hookspike/.claude/settings.json`:

```json
{
  "hooks": {
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "sh -c 'cat >> /tmp/hookspike/log.jsonl; echo >> /tmp/hookspike/log.jsonl; echo UPS $(date +%s) >> /tmp/hookspike/order.log'", "timeout": 5 }] }],
    "PreToolUse": [{ "hooks": [{ "type": "command", "command": "sh -c 'cat >> /tmp/hookspike/log.jsonl; echo >> /tmp/hookspike/log.jsonl; echo PRE $(date +%s) >> /tmp/hookspike/order.log'", "timeout": 5 }] }],
    "PostToolUse": [{ "hooks": [{ "type": "command", "command": "sh -c 'cat >> /tmp/hookspike/log.jsonl; echo >> /tmp/hookspike/log.jsonl; echo POST $(date +%s) >> /tmp/hookspike/order.log'", "timeout": 5 }] }],
    "Stop": [{ "hooks": [{ "type": "command", "command": "sh -c 'cat >> /tmp/hookspike/log.jsonl; echo >> /tmp/hookspike/log.jsonl; echo STOP $(date +%s) >> /tmp/hookspike/order.log'", "timeout": 5 }] }],
    "Notification": [{ "matcher": "permission_prompt", "hooks": [{ "type": "command", "command": "sh -c 'cat >> /tmp/hookspike/log.jsonl; echo >> /tmp/hookspike/log.jsonl; echo NOTIFY-PERM $(date +%s) >> /tmp/hookspike/order.log'", "timeout": 5 }] }]
  }
}
```

- [ ] **Step 2: Run an interactive session in that directory, with the msgbus MCP server configured**

```bash
cd /tmp/hookspike && claude --dangerously-load-development-channels server:msgbus
```

In the session:
- Ask it to run a Bash command that needs approval. Wait more than 6 seconds before approving.
- From another agent, send it a bus message.
- Leave it idle for more than 60 seconds.

- [ ] **Step 3: Record the answers**

```bash
curl -s http://127.0.0.1:7777/api/agents | jq '.[] | select(.cwd=="/tmp/hookspike") | .sessionId'
jq -r .session_id /tmp/hookspike/log.jsonl | sort -u
cat /tmp/hookspike/order.log
```

Expected:
- One `session_id` in the log, equal to the agent's `sessionId`.
- `NOTIFY-PERM` between a `PRE` and its `POST`.
- No `NOTIFY-PERM` line for the idle period.
- Whether a `UPS` line appeared for the bus-message turn.

- [ ] **Step 4: Decide what changes**

- If the session ids **differ** or the bridge's is **absent**, change `status_key` in Task 4 to key on the project directory, then continue. Only that function changes; it is the one place both sides derive the key.
- If `PreToolUse` fires **after** the `permission_prompt` notification for the same tool, then `PreToolUse` would clear the blocked state too early. In that case use `PostToolUse` as the "working" signal and drop `PreToolUse` from Task 6's hook list.

Write the findings into the spec's "Open items" section, replacing each question with its answer.

- [ ] **Step 5: Commit the spec update**

```bash
git add docs/superpowers/specs/2026-10-05-agent-status-design.md
git commit -m "docs: record hook spike results for agent status"
```

---

### Task 1: Protocol: `AgentState`, `ToBus::Status`, `StatusView`, `FromBus::Status`

**Files:**
- Modify: `src/proto.rs` (add types; add `status` to `AgentInfo`; add the `ToBus::Status` and `FromBus::Status` variants; tests)
- Modify: `src/bus/mod.rs`: `req_id_of` gets a `ToBus::Status { .. } => None` arm
- Modify: `src/bus/commands.rs`: a temporary `ToBus::Status { .. } => {}` arm, replaced in Task 2

**Interfaces:**
- Produces:
  - `proto::AgentState { Working, Idle, BlockedOnHuman, Unknown }`, serialized `snake_case`
  - `ToBus::Status { state, state_age_ms: Option<u64>, heartbeat_age_ms: Option<u64>, reason: Option<String>, text: Option<String>, text_age_ms: Option<u64>, waiting_on: Option<String> }`
  - `proto::StatusView { state, since: i64, last_heartbeat: Option<i64>, reason: Option<String>, text: Option<String>, text_at: Option<i64>, quiet: bool }`
  - `FromBus::Status { name: String, status: StatusView }`
  - `AgentInfo.status: Option<StatusView>`

- [ ] **Step 1: Write the failing tests** (append to `mod tests` in `src/proto.rs`)

```rust
#[test]
fn status_round_trips_and_omitted_fields_default() {
    let cmd = ToBus::Status {
        state: AgentState::BlockedOnHuman,
        state_age_ms: Some(4_210),
        heartbeat_age_ms: Some(10),
        reason: Some("permission_prompt".into()),
        text: Some("voice-fit run 3/5".into()),
        text_age_ms: Some(300_000),
        waiting_on: None,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"type\":\"status\""), "{json}");
    assert!(json.contains("\"blocked_on_human\""), "{json}");
    assert_eq!(serde_json::from_str::<ToBus>(&json).unwrap(), cmd);

    // A minimal frame (state only) must parse: every other field defaults.
    let minimal: ToBus = serde_json::from_str(r#"{"type":"status","state":"idle"}"#).unwrap();
    assert_eq!(
        minimal,
        ToBus::Status {
            state: AgentState::Idle,
            state_age_ms: None,
            heartbeat_age_ms: None,
            reason: None,
            text: None,
            text_age_ms: None,
            waiting_on: None,
        }
    );
}

#[test]
fn agent_info_without_status_still_parses() {
    // A new agent binary talking to an old bus must keep parsing `agents` replies.
    let v: AgentInfo =
        serde_json::from_str(r#"{"name":"caas","host":"h","online":true,"version":"0.8.0"}"#)
            .unwrap();
    assert_eq!(v.status, None);
}
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test --lib proto::tests::status_round_trips`
Expected: compile error, `AgentState` not found.

- [ ] **Step 3: Implement**

In `src/proto.rs`, above `ToBus`:

```rust
/// What an agent is doing, as its Claude Code hooks last reported it.
///
/// `Unknown` is a real state, not a missing value: a session without the hooks
/// installed, or one that has not fired a hook since it started.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Working,
    Idle,
    BlockedOnHuman,
    Unknown,
}
```

Add this variant to `ToBus`, after `WatchEvents`:

```rust
    /// The agent's current status. Fire-and-forget like `Ack`: no `req_id`, no reply.
    ///
    /// Ages, not timestamps: the agent and the bus are often on different machines,
    /// and an age measured on one clock survives the trip, where an absolute time
    /// would carry the sender's clock error with it. The bus turns each age into
    /// its own time on arrival.
    Status {
        state: AgentState,
        #[serde(default)]
        state_age_ms: Option<u64>,
        #[serde(default)]
        heartbeat_age_ms: Option<u64>,
        /// Why the state is what it is, e.g. the notification type that made it
        /// `blocked_on_human` (`permission_prompt`).
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        text_age_ms: Option<u64>,
        /// Reserved for phase 2 (waiting-on chains). Ignored by the bus today.
        #[serde(default)]
        waiting_on: Option<String>,
    },
```

After `AgentInfo`:

```rust
/// An agent's status as readers see it. Every time is bus time, epoch milliseconds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
pub struct StatusView {
    pub state: AgentState,
    #[ts(type = "number")]
    pub since: i64,
    #[ts(type = "number | null")]
    pub last_heartbeat: Option<i64>,
    pub reason: Option<String>,
    pub text: Option<String>,
    #[ts(type = "number | null")]
    pub text_at: Option<i64>,
    /// Working, but no hook has fired for longer than the quiet threshold. A soft
    /// hint, not a verdict: one long Bash command fires no hooks while it runs.
    pub quiet: bool,
}
```

Add to `AgentInfo`:

```rust
    /// `None` when the agent has never reported a status, or when a bus that
    /// predates statuses answered.
    #[serde(default)]
    pub status: Option<StatusView>,
```

Add this variant to `FromBus`, after `Presence`:

```rust
    /// An agent's status changed. Sent to the same observers as `Presence`.
    Status { name: String, status: StatusView },
```

Fix the compile errors:
- `src/bus/commands.rs` `ListAgents` arm: add `status: None,` to the `AgentInfo` literal. Task 3 fills it in.
- `src/bus/commands.rs` `handle` match: add `ToBus::Status { .. } => {}`. Task 2 replaces it.
- `src/bus/mod.rs` `req_id_of`: add `| ToBus::Status { .. }` to the `None` arm.
- `handle_observer` already rejects unknown commands through its `other =>` arm.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib proto && cargo build --all-targets`
Expected: PASS and a clean build. `ui/src/types/AgentState.ts`, `StatusView.ts`, `AgentInfo.ts` and `FromBus.ts` are regenerated.

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt
git add src/proto.rs src/bus/mod.rs src/bus/commands.rs ui/src/types
git commit -m "feat: protocol for agent status"
```

---

### Task 2: The bus's status store, applying updates, events, and clearing

**Files:**
- Create: `src/bus/status.rs`
- Modify: `src/bus/mod.rs`:
  - `pub(crate) mod status;`
  - `App` gains `pub(crate) statuses: status::Statuses`
  - build it in `serve_on_full`
  - disconnect teardown
- Modify: `src/bus/commands.rs`: the `ToBus::Status` arm
- Modify: `src/web/api.rs`: `agent_delete` success path clears the status
- Test: unit tests in `src/bus/status.rs`; integration tests in a new `tests/status.rs`

**Interfaces:**
- Consumes: the Task 1 types.
- Produces:

```rust
pub(crate) const TEXT_CAP: usize = 200;
pub(crate) const QUIET_AFTER_MS: i64 = 10 * 60 * 1000;
#[derive(Clone, Default)] pub(crate) struct Statuses(..);
impl Statuses {
    pub(crate) async fn apply(&self, name: &str, update: Update, now: i64) -> Transition;
    pub(crate) async fn view(&self, name: &str, now: i64) -> Option<StatusView>;
    pub(crate) async fn all(&self, now: i64) -> HashMap<String, StatusView>;
    pub(crate) async fn remove(&self, name: &str) -> Option<AgentState>;
    pub(crate) async fn state_of(&self, name: &str) -> Option<AgentState>;
}
pub(crate) struct Update { state, state_age_ms, heartbeat_age_ms, reason, text, text_age_ms }
pub(crate) struct Transition { pub before: Option<AgentState>, pub after: AgentState, pub reason: Option<String> }
pub(crate) fn truncate_text(s: &str) -> String;
```

- [ ] **Step 1: Write the failing unit tests** (in `src/bus/status.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::AgentState;

    fn upd(state: AgentState) -> Update {
        Update {
            state,
            state_age_ms: Some(0),
            heartbeat_age_ms: Some(0),
            reason: None,
            text: None,
            text_age_ms: None,
        }
    }

    #[tokio::test]
    async fn ages_become_bus_times() {
        let s = Statuses::default();
        let mut u = upd(AgentState::Idle);
        u.state_age_ms = Some(60_000);
        u.text = Some("wrote anchor.verified.json".into());
        u.text_age_ms = Some(5_000);
        s.apply("caas", u, 1_000_000).await;
        let v = s.view("caas", 1_000_000).await.unwrap();
        assert_eq!(v.since, 940_000);
        assert_eq!(v.text_at, Some(995_000));
        assert_eq!(v.text.as_deref(), Some("wrote anchor.verified.json"));
    }

    #[tokio::test]
    async fn quiet_is_derived_at_read_time_and_only_while_working() {
        let s = Statuses::default();
        s.apply("w", upd(AgentState::Working), 0).await;
        assert!(!s.view("w", QUIET_AFTER_MS).await.unwrap().quiet);
        assert!(s.view("w", QUIET_AFTER_MS + 1).await.unwrap().quiet);
        s.apply("i", upd(AgentState::Idle), 0).await;
        assert!(!s.view("i", QUIET_AFTER_MS * 10).await.unwrap().quiet);
    }

    #[tokio::test]
    async fn text_is_capped_on_a_char_boundary_and_flattened() {
        let s = Statuses::default();
        let mut u = upd(AgentState::Working);
        u.text = Some(format!("é{}\nsecond line", "x".repeat(5_000)));
        s.apply("a", u, 0).await;
        let t = s.view("a", 0).await.unwrap().text.unwrap();
        assert_eq!(t.chars().count(), TEXT_CAP);
        assert!(!t.contains('\n'), "status text renders on one line: {t:?}");
    }

    #[tokio::test]
    async fn empty_text_clears_it() {
        let s = Statuses::default();
        let mut u = upd(AgentState::Working);
        u.text = Some("busy".into());
        s.apply("a", u, 0).await;
        let mut u = upd(AgentState::Working);
        u.text = Some(String::new());
        s.apply("a", u, 0).await;
        assert_eq!(s.view("a", 0).await.unwrap().text, None);
    }

    #[tokio::test]
    async fn apply_reports_the_transition() {
        let s = Statuses::default();
        let t = s.apply("a", upd(AgentState::Working), 0).await;
        assert_eq!((t.before, t.after), (None, AgentState::Working));
        let mut b = upd(AgentState::BlockedOnHuman);
        b.reason = Some("permission_prompt".into());
        let t = s.apply("a", b, 1).await;
        assert_eq!(
            (t.before, t.after),
            (Some(AgentState::Working), AgentState::BlockedOnHuman)
        );
        assert_eq!(t.reason.as_deref(), Some("permission_prompt"));
    }

    #[tokio::test]
    async fn remove_forgets_the_agent() {
        let s = Statuses::default();
        s.apply("a", upd(AgentState::Idle), 0).await;
        assert_eq!(s.remove("a").await, Some(AgentState::Idle));
        assert!(s.view("a", 0).await.is_none());
    }
}
```

- [ ] **Step 2: Confirm they fail**

Run: `cargo test --lib bus::status`
Expected: compile error, the module does not exist.

- [ ] **Step 3: Implement `src/bus/status.rs`**

```rust
//! Agent status, kept in memory beside the registry.
//!
//! Never persisted, on purpose: a persisted `online` column once showed agents
//! from a dead bus as online. After a bus restart, agents resend their status
//! on registration (see `agent::bridge`), so nothing is lost by keeping this in
//! memory. Staleness (`quiet`) is computed on every read, never stored.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::proto::{AgentState, StatusView};

pub(crate) const TEXT_CAP: usize = 200;
pub(crate) const QUIET_AFTER_MS: i64 = 10 * 60 * 1000;

pub(crate) struct Update {
    pub state: AgentState,
    pub state_age_ms: Option<u64>,
    pub heartbeat_age_ms: Option<u64>,
    pub reason: Option<String>,
    pub text: Option<String>,
    pub text_age_ms: Option<u64>,
}

pub(crate) struct Transition {
    pub before: Option<AgentState>,
    pub after: AgentState,
    pub reason: Option<String>,
}

#[derive(Clone)]
struct Entry {
    state: AgentState,
    since: i64,
    last_heartbeat: Option<i64>,
    reason: Option<String>,
    text: Option<String>,
    text_at: Option<i64>,
}

#[derive(Clone, Default)]
pub(crate) struct Statuses(Arc<Mutex<HashMap<String, Entry>>>);

/// One line, at most `TEXT_CAP` characters. Line breaks become spaces, so every
/// renderer (tool text, CLI table, console row) can treat the text as a single line.
pub(crate) fn truncate_text(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(TEXT_CAP)
        .collect()
}

fn at(now: i64, age_ms: Option<u64>) -> Option<i64> {
    age_ms.map(|a| now - i64::try_from(a).unwrap_or(i64::MAX).min(now))
}

impl Statuses {
    pub(crate) async fn apply(&self, name: &str, u: Update, now: i64) -> Transition {
        let mut map = self.0.lock().await;
        let before = map.get(name).map(|e| e.state);
        let text = u.text.map(|t| truncate_text(&t)).filter(|t| !t.is_empty());
        let text_at = text.as_ref().and(at(now, u.text_age_ms).or(Some(now)));
        let entry = Entry {
            state: u.state,
            since: at(now, u.state_age_ms).unwrap_or(now),
            last_heartbeat: at(now, u.heartbeat_age_ms),
            reason: u.reason.clone(),
            text,
            text_at,
        };
        map.insert(name.to_string(), entry);
        Transition {
            before,
            after: u.state,
            reason: u.reason,
        }
    }

    pub(crate) async fn view(&self, name: &str, now: i64) -> Option<StatusView> {
        self.0.lock().await.get(name).map(|e| view_of(e, now))
    }

    pub(crate) async fn all(&self, now: i64) -> HashMap<String, StatusView> {
        self.0
            .lock()
            .await
            .iter()
            .map(|(k, e)| (k.clone(), view_of(e, now)))
            .collect()
    }

    pub(crate) async fn state_of(&self, name: &str) -> Option<AgentState> {
        self.0.lock().await.get(name).map(|e| e.state)
    }

    pub(crate) async fn remove(&self, name: &str) -> Option<AgentState> {
        self.0.lock().await.remove(name).map(|e| e.state)
    }
}

fn view_of(e: &Entry, now: i64) -> StatusView {
    let last_signal = e.last_heartbeat.unwrap_or(e.since).max(e.since);
    StatusView {
        state: e.state,
        since: e.since,
        last_heartbeat: e.last_heartbeat,
        reason: e.reason.clone(),
        text: e.text.clone(),
        text_at: e.text_at,
        quiet: e.state == AgentState::Working && now - last_signal > QUIET_AFTER_MS,
    }
}
```

Note the full-replace semantics of `apply`. The bridge always sends its **whole** current status, never a diff, so a frame without `text` means there is no text. That is how `empty_text_clears_it` and the restart resend stay simple.

- [ ] **Step 4: Run the unit tests**

Run: `cargo test --lib bus::status`
Expected: PASS.

- [ ] **Step 5: Wire it into `App`, the command arm, disconnect, and delete**

`src/bus/mod.rs`:
- Add `pub(crate) mod status;`.
- Add `pub(crate) statuses: status::Statuses,` to `App`, and `statuses: status::Statuses::default(),` where `App` is built in `serve_on_full`.
- In the disconnect teardown (inside `if let Some(name) = me {`, after `notify_presence`), add the block below.

```rust
        // A session that dies while blocked on the human is no longer blocked: close the
        // event the way a resolved prompt would, so the log never shows a wait that
        // stayed open forever. The status itself stays, so readers see "offline, was
        // blocked".
        if app.statuses.state_of(&name).await == Some(crate::proto::AgentState::BlockedOnHuman) {
            let _ = app
                .store
                .append_event(
                    "blocked_on_human",
                    Some(&name),
                    None,
                    json!({ "entered": false, "via": "disconnect" }),
                )
                .await;
        }
```

`src/bus/commands.rs`: replace the placeholder arm.

```rust
        ToBus::Status {
            state,
            state_age_ms,
            heartbeat_age_ms,
            reason,
            text,
            text_age_ms,
            waiting_on: _,
        } => {
            let now = crate::store::now_ms();
            let t = app
                .statuses
                .apply(
                    me,
                    crate::bus::status::Update {
                        state,
                        state_age_ms,
                        heartbeat_age_ms,
                        reason,
                        text,
                        text_age_ms,
                    },
                    now,
                )
                .await;
            let blocked = crate::proto::AgentState::BlockedOnHuman;
            let was = t.before == Some(blocked);
            let is = t.after == blocked;
            if was != is {
                let _ = app
                    .store
                    .append_event(
                        "blocked_on_human",
                        Some(me),
                        None,
                        json!({ "entered": is, "reason": t.reason, "via": "hook" }),
                    )
                    .await;
            }
            if let Some(status) = app.statuses.view(me, now).await {
                app.registry
                    .notify_presence(FromBus::Status {
                        name: me.to_string(),
                        status,
                    })
                    .await;
            }
        }
```

`src/web/api.rs` `agent_delete`, in the `Some(Ok(counts))` success arm, before appending the event:

```rust
            // The status must not outlive the agent, or a later agent registering
            // under the same name would inherit it.
            app.statuses.remove(&name).await;
```

- [ ] **Step 6: Write the integration tests** (`tests/status.rs`)

```rust
mod common;

use claude_bus::proto::{AgentState, FromBus, ToBus};

fn status(state: AgentState) -> ToBus {
    ToBus::Status {
        state,
        state_age_ms: Some(0),
        heartbeat_age_ms: Some(0),
        reason: None,
        text: None,
        text_age_ms: None,
        waiting_on: None,
    }
}

async fn events_of(port: u16, kind: &str) -> Vec<serde_json::Value> {
    let all = common::get_json(port, &format!("/api/events?kind={kind}")).await;
    all.as_array().unwrap().clone()
}

#[tokio::test]
async fn a_status_reaches_presence_watchers() {
    let (_dir, port) = common::start_bus().await;
    let mut obs = common::connect_observer(port, "console").await;
    common::send(&mut obs, &ToBus::WatchPresence { req_id: 1 }).await;
    let mut a = common::connect(port, "caas").await;
    common::next_event(&mut a).await; // Registered
    common::send(&mut a, &status(AgentState::Working)).await;
    let mut saw = false;
    for _ in 0..10 {
        if let FromBus::Status { name, status } = common::next_event(&mut obs).await {
            assert_eq!(name, "caas");
            assert_eq!(status.state, AgentState::Working);
            saw = true;
            break;
        }
    }
    assert!(saw, "the observer never received the status push");
}

#[tokio::test]
async fn entering_and_leaving_blocked_writes_exactly_two_events() {
    let (_dir, port) = common::start_bus().await;
    let mut a = common::connect(port, "caas").await;
    common::next_event(&mut a).await;
    common::send(&mut a, &status(AgentState::Working)).await;
    let mut b = status(AgentState::BlockedOnHuman);
    if let ToBus::Status { reason, .. } = &mut b {
        *reason = Some("permission_prompt".into());
    }
    common::send(&mut a, &b).await;
    common::send(&mut a, &b).await; // a repeat must not log a second entry
    common::send(&mut a, &status(AgentState::Working)).await;
    common::send(&mut a, &status(AgentState::Idle)).await; // not logged at all
    assert!(
        common::wait_until(|| async { events_of(port, "blocked_on_human").await.len() == 2 })
            .await,
        "expected one entry and one exit"
    );
    let ev = events_of(port, "blocked_on_human").await;
    let entered: Vec<bool> = ev
        .iter()
        .map(|e| e["detail"]["entered"].as_bool().unwrap())
        .collect();
    assert!(entered.contains(&true) && entered.contains(&false), "{ev:?}");
}

#[tokio::test]
async fn disconnecting_while_blocked_closes_the_wait_and_keeps_the_status() {
    let (_dir, port) = common::start_bus().await;
    let mut a = common::connect(port, "caas").await;
    common::next_event(&mut a).await;
    common::send(&mut a, &status(AgentState::BlockedOnHuman)).await;
    assert!(
        common::wait_until(|| async { events_of(port, "blocked_on_human").await.len() == 1 })
            .await
    );
    drop(a);
    assert!(
        common::wait_until(|| async {
            events_of(port, "blocked_on_human")
                .await
                .iter()
                .any(|e| e["detail"]["via"] == "disconnect")
        })
        .await,
        "a disconnect while blocked must close the wait"
    );
}

#[tokio::test]
async fn deleting_an_agent_forgets_its_status() {
    let (_dir, port) = common::start_bus().await;
    let mut a = common::connect(port, "caas").await;
    common::next_event(&mut a).await;
    common::send(&mut a, &status(AgentState::Idle)).await;
    drop(a);
    assert!(common::wait_until(|| async { !common::agent_is_online(port, "caas").await }).await);
    assert_eq!(common::delete_same_origin(port, "/api/agents/caas").await, 204);
    let mut again = common::connect(port, "caas").await;
    common::next_event(&mut again).await;
    let agents = common::get_json(port, "/api/agents").await;
    let caas = agents.as_array().unwrap().iter().find(|a| a["name"] == "caas").unwrap();
    assert!(caas["status"].is_null(), "a re-registered name inherited a status: {caas}");
}

#[tokio::test]
async fn status_never_counts_toward_the_exchange_cap() {
    // A cap of 2. Fifty statuses, then one real send, which must still be allowed.
    let (_dir, port, _path) =
        common::start_bus_with_guards_dir(claude_bus::bus::delivery::Guards::new(2, 0)).await;
    let mut a = common::connect(port, "a").await;
    common::next_event(&mut a).await;
    let mut b = common::connect(port, "b").await;
    common::next_event(&mut b).await;
    for _ in 0..50 {
        common::send(&mut a, &status(AgentState::Working)).await;
    }
    common::send(
        &mut a,
        &ToBus::Send {
            req_id: 9,
            target: claude_bus::proto::Target::Agent { name: "b".into() },
            text: "real".into(),
            done: false,
        },
    )
    .await;
    loop {
        match common::next_event(&mut a).await {
            FromBus::Reply { req_id: 9, .. } => break,
            FromBus::Error { req_id: Some(9), message } => panic!("send refused: {message}"),
            _ => continue,
        }
    }
}
```

The `deleting_an_agent_forgets_its_status` test depends on Task 3's `status` field in `/api/agents`. Mark it `#[ignore = "enabled in Task 3"]` now and remove the attribute in Task 3.

- [ ] **Step 7: Run the integration tests**

Run: `cargo test --test status`
Expected: PASS, with the one test ignored.

- [ ] **Step 8: Commit**

```bash
cargo +nightly fmt
git add src/bus src/web/api.rs tests/status.rs
git commit -m "feat: bus keeps agent status in memory and logs blocked-on-human"
```

---

### Task 3: Readers: the `agents` tool, the HTTP API, observer `ListAgents`, and `claude-bus status`

**Files:**
- Modify: `src/bus/commands.rs`: extract `pub(crate) async fn reply_list_agents(app, control_tx, req_id)` and fill `AgentInfo.status`
- Modify: `src/bus/mod.rs`: `handle_observer` gains a `ToBus::ListAgents { req_id }` arm, and the rejection text adds `list_agents`
- Modify: `src/web/api.rs`: `Agent`, `RailAgent` and `AgentDetail` gain `status: Option<StatusView>`
- Modify: `src/agent/handler.rs`: the `agents` tool renders the status
- Create: `src/status_cli.rs` (`claude-bus status`); modify `src/lib.rs` and `src/main.rs`
- Test: `tests/status.rs` (more cases); unit tests in `src/status_cli.rs` and `src/agent/handler.rs`

**Interfaces:**
- Consumes: `Statuses::view`, `Statuses::all`, `StatusView`.
- Produces:
  - `pub fn render_status(s: &StatusView, online: bool, now: i64) -> String`, in a new `src/status_text.rs` that the tool and the CLI share
  - `claude_bus::status_cli::run(bus: String) -> anyhow::Result<()>`

- [ ] **Step 1: Write the failing rendering tests** (`src/status_text.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{AgentState, StatusView};

    fn sv(state: AgentState, since: i64, text: Option<&str>, quiet: bool) -> StatusView {
        StatusView {
            state,
            since,
            last_heartbeat: None,
            reason: None,
            text: text.map(String::from),
            text_at: text.map(|_| since),
            quiet,
        }
    }

    #[test]
    fn idle_with_text_reads_naturally() {
        let s = sv(AgentState::Idle, 0, Some("wrote anchor.verified.json"), false);
        assert_eq!(render_status(&s, true, 5 * 60_000), "idle 5m: wrote anchor.verified.json");
    }

    #[test]
    fn offline_shows_the_last_state_as_past() {
        let s = sv(AgentState::Working, 0, None, false);
        assert_eq!(render_status(&s, false, 40 * 60_000), "offline, was working 40m ago");
    }

    #[test]
    fn quiet_working_says_so() {
        let s = sv(AgentState::Working, 0, None, true);
        assert_eq!(render_status(&s, true, 25 * 60_000), "working? (quiet) 25m");
    }

    #[test]
    fn blocked_names_the_reason() {
        let mut s = sv(AgentState::BlockedOnHuman, 0, None, false);
        s.reason = Some("permission_prompt".into());
        assert_eq!(
            render_status(&s, true, 2 * 60_000),
            "blocked on human (permission_prompt) 2m"
        );
    }

    #[test]
    fn ages_scale_to_hours_and_days() {
        assert_eq!(age(90 * 60_000), "1h30m");
        assert_eq!(age(3 * 24 * 3_600_000), "3d");
        assert_eq!(age(30_000), "30s");
    }
}
```

- [ ] **Step 2: Confirm they fail**

Run: `cargo test --lib status_text`
Expected: compile error.

- [ ] **Step 3: Implement `src/status_text.rs`** and add `pub mod status_text;` and `pub mod status_cli;` to `src/lib.rs`

```rust
//! One-line rendering of an agent status, shared by the `agents` MCP tool and the
//! `claude-bus status` CLI so the two never describe the same status differently.

use crate::proto::{AgentState, StatusView};

pub fn age(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3_600 => format!("{}m", s / 60),
        3_600..86_400 if s % 3_600 / 60 == 0 => format!("{}h", s / 3_600),
        3_600..86_400 => format!("{}h{}m", s / 3_600, s % 3_600 / 60),
        _ => format!("{}d", s / 86_400),
    }
}

fn state_word(s: AgentState) -> &'static str {
    match s {
        AgentState::Working => "working",
        AgentState::Idle => "idle",
        AgentState::BlockedOnHuman => "blocked on human",
        AgentState::Unknown => "unknown",
    }
}

pub fn render_status(s: &StatusView, online: bool, now: i64) -> String {
    let since = age(now - s.since);
    let mut out = if !online {
        format!("offline, was {} {since} ago", state_word(s.state))
    } else if s.quiet {
        format!("working? (quiet) {since}")
    } else if let Some(r) = s.reason.as_deref().filter(|_| s.state == AgentState::BlockedOnHuman) {
        format!("{} ({r}) {since}", state_word(s.state))
    } else {
        format!("{} {since}", state_word(s.state))
    };
    if let Some(t) = &s.text {
        out.push_str(": ");
        out.push_str(t);
        // Text set long before the current state began is shown with its age, so it
        // never reads as current.
        if let Some(at) = s.text_at
            && s.since - at > 10 * 60_000
        {
            out.push_str(&format!(" (text from {} ago)", age(now - at)));
        }
    }
    out
}
```

- [ ] **Step 4: Fill the readers in**

`src/bus/commands.rs`: move the `ListAgents` arm body into `reply_list_agents`, which `handle`'s arm then calls.

```rust
pub(crate) async fn reply_list_agents(app: &App, control_tx: &registry::Sender, req_id: u64) {
    let online = app.registry.online().await;
    let statuses = app.statuses.all(crate::store::now_ms()).await;
    let agents = app
        .store
        .agents()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|a| AgentInfo {
            online: online.contains(&a.name),
            status: statuses.get(&a.name).cloned(),
            name: a.name,
            host: a.host,
            version: a.version,
        })
        .collect();
    let _ = control_tx.try_send(FromBus::Reply {
        req_id,
        result: ReplyResult::Agents { agents },
    });
}
```

`src/bus/mod.rs` `handle_observer`: add `ToBus::ListAgents { req_id } => commands::reply_list_agents(app, control_tx, req_id).await,`. It is read-only, like `ListRooms`. Update the rejection message to `"observers may only watch, list_rooms, list_agents, or history — …"`.

`src/web/api.rs`:
- Add `pub status: Option<crate::proto::StatusView>,` to `Agent`, `RailAgent` and `AgentDetail`. Doc comment: `/// See \`proto::StatusView\`. \`None\` until the agent first reports one.`
- In `agents()` and `rail()`, fetch `let statuses = app.statuses.all(crate::store::now_ms()).await;` once before the loop and set `status: statuses.get(&name).cloned()`. Compute this before `name` is moved, the same way `is_relayer` is.
- In `agent_detail`, set `status: app.statuses.view(&name, crate::store::now_ms()).await`.

`src/agent/handler.rs` `agents` tool: change the `format!` to append the status when present.

```rust
                            let base = format!(
                                "{} — {} — {} — {}",
                                a.name,
                                a.host,
                                if a.online { "online" } else { "offline" },
                                a.version.as_deref().unwrap_or("unknown")
                            );
                            match &a.status {
                                Some(s) => format!(
                                    "{base} — {}",
                                    crate::status_text::render_status(
                                        s,
                                        a.online,
                                        crate::store::now_ms()
                                    )
                                ),
                                None => base,
                            }
```

Also add to the tool's description: `"…and each agent's status (working, idle, blocked on human), with how long it has been in that state and any text it set."`

`src/status_cli.rs`:

```rust
//! `claude-bus status`: a one-shot table of every agent and its status.
//!
//! Connects as an observer, the way `tail` does, and asks for `list_agents`.
//! No new HTTP client, and no registration that would add a row to `agents`.

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use crate::proto::{AgentInfo, FromBus, ReplyResult, ToBus};

pub fn table(agents: &[AgentInfo], now: i64) -> String {
    let mut rows: Vec<&AgentInfo> = agents.iter().collect();
    // Blocked on the human first: those rows ask something of the reader.
    rows.sort_by_key(|a| {
        let blocked = a.online
            && a.status.as_ref().map(|s| s.state) == Some(crate::proto::AgentState::BlockedOnHuman);
        (!blocked, !a.online, a.name.clone())
    });
    let width = rows.iter().map(|a| a.name.chars().count()).max().unwrap_or(0);
    rows.iter()
        .map(|a| {
            let status = match &a.status {
                Some(s) => crate::status_text::render_status(s, a.online, now),
                None if a.online => "online, no status".to_string(),
                None => "offline".to_string(),
            };
            format!("{:width$}  {status}", a.name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn run(bus: String) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(&bus).await?;
    let (mut sink, mut stream) = ws.split();
    for cmd in [
        ToBus::Observe { name: format!("status-{}", std::process::id()) },
        ToBus::ListAgents { req_id: 1 },
    ] {
        sink.send(Message::text(serde_json::to_string(&cmd)?)).await?;
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let msg = tokio::time::timeout_at(deadline, stream.next())
            .await
            .map_err(|_| anyhow::anyhow!("the bus at {bus} did not answer within 10s"))?;
        let Some(msg) = msg else { anyhow::bail!("the bus closed the connection") };
        let Ok(text) = msg?.into_text() else { continue };
        match serde_json::from_str::<FromBus>(&text) {
            Ok(FromBus::Reply { req_id: 1, result: ReplyResult::Agents { agents } }) => {
                println!("{}", table(&agents, crate::store::now_ms()));
                return Ok(());
            }
            Ok(FromBus::Error { message, .. }) => anyhow::bail!("bus refused: {message}"),
            _ => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{AgentState, StatusView};

    #[test]
    fn blocked_agents_sort_first() {
        let mk = |name: &str, state: AgentState| AgentInfo {
            name: name.into(),
            host: "h".into(),
            online: true,
            version: None,
            status: Some(StatusView {
                state,
                since: 0,
                last_heartbeat: None,
                reason: None,
                text: None,
                text_at: None,
                quiet: false,
            }),
        };
        let out = table(&[mk("a", AgentState::Idle), mk("z", AgentState::BlockedOnHuman)], 0);
        assert!(out.lines().next().unwrap().starts_with('z'), "{out}");
    }
}
```

`src/main.rs`:
- Add `Some("status") => { let bus = flag(&args, "--bus").unwrap_or_else(|| "ws://127.0.0.1:7777/ws".to_string()); claude_bus::status_cli::run(bus).await?; Ok(()) }`.
- Add a usage line: `eprintln!("  claude-bus status [--bus ws://host:7777/ws]   # every agent's status");`.

- [ ] **Step 5: Extend the integration tests** (append to `tests/status.rs`, and remove the `#[ignore]` from Task 2)

```rust
#[tokio::test]
async fn every_reader_reports_the_status() {
    let (_dir, port) = common::start_bus().await;
    let mut a = common::connect(port, "caas").await;
    common::next_event(&mut a).await;
    let mut s = status(AgentState::Idle);
    if let ToBus::Status { text, .. } = &mut s {
        *text = Some("<script>alert(1)</script>".into());
    }
    common::send(&mut a, &s).await;
    assert!(
        common::wait_until(|| async {
            common::get_json(port, "/api/agents").await[0]["status"]["state"] == "idle"
        })
        .await
    );
    let rail = common::get_json(port, "/api/rail").await;
    assert_eq!(rail["agents"][0]["status"]["state"], "idle", "{rail}");
    let detail = common::get_json(port, "/api/agents/caas").await;
    assert_eq!(
        detail["status"]["text"], "<script>alert(1)</script>",
        "stored verbatim; escaping is the renderer's job: {detail}"
    );
}

#[tokio::test]
async fn an_observer_can_list_agents_with_status() {
    let (_dir, port) = common::start_bus().await;
    let mut a = common::connect(port, "caas").await;
    common::next_event(&mut a).await;
    common::send(&mut a, &status(AgentState::Working)).await;
    let mut obs = common::connect_observer(port, "status-cli").await;
    common::send(&mut obs, &ToBus::ListAgents { req_id: 4 }).await;
    loop {
        if let FromBus::Reply {
            req_id: 4,
            result: claude_bus::proto::ReplyResult::Agents { agents },
        } = common::next_event(&mut obs).await
        {
            let caas = agents.iter().find(|x| x.name == "caas").unwrap();
            if caas.status.is_some() {
                break;
            }
            common::send(&mut obs, &ToBus::ListAgents { req_id: 4 }).await;
        }
    }
}
```

- [ ] **Step 6: Run everything**

Run: `cargo test --lib status_text status_cli && cargo test --test status && cargo test --test web && cargo test --test agent_contract`
Expected: PASS. Update any test that builds `AgentInfo` or the API structs by hand, adding `status: None`. The compiler lists each one.

- [ ] **Step 7: Commit**

```bash
cargo +nightly fmt
git add src tests ui/src/types
git commit -m "feat: agent status in the agents tool, API, and claude-bus status"
```

---

### Task 4: `claude-bus hook` and the state file

**Files:**
- Create: `src/agent/status.rs` (pure transition rules, file path and key, atomic read and write)
- Create: `src/hook.rs` (the subcommand: read stdin, apply, write, never fail)
- Modify: `src/agent/mod.rs` (`pub mod status;`), `src/lib.rs` (`pub mod hook;`), `src/main.rs` (the `hook` arm)
- Test: unit tests in `src/agent/status.rs`; a new `tests/hook.rs` that runs the real binary

**Interfaces:**
- Produces:

```rust
// src/agent/status.rs
pub const HEARTBEAT_EVERY_MS: i64 = 30_000;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookState { pub state: AgentState, pub changed_at_ms: i64, pub heartbeat_at_ms: i64, pub reason: Option<String> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent { PromptSubmit, ToolUse, Stop, Blocked(String) }
impl HookEvent { pub fn parse(args: &[String]) -> Option<HookEvent> }
pub fn next(prev: Option<&HookState>, ev: &HookEvent, now_ms: i64) -> Option<HookState>;
pub fn status_key(session_id: Option<&str>) -> Option<String>;
pub fn status_dir(xdg_state_home: Option<String>, home: Option<String>) -> Option<PathBuf>;
pub fn read(path: &Path) -> Option<HookState>;
pub fn write(path: &Path, s: &HookState) -> std::io::Result<()>;
// src/hook.rs
pub fn run(args: &[String]); // never panics, never prints to stdout
```

- [ ] **Step 1: Write the failing unit tests** (`src/agent/status.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::AgentState::*;

    fn st(state: crate::proto::AgentState, changed: i64, hb: i64) -> HookState {
        HookState {
            state,
            changed_at_ms: changed,
            heartbeat_at_ms: hb,
            reason: None,
        }
    }

    #[test]
    fn events_map_to_states() {
        assert_eq!(next(None, &HookEvent::PromptSubmit, 5).unwrap().state, Working);
        assert_eq!(next(None, &HookEvent::ToolUse, 5).unwrap().state, Working);
        assert_eq!(next(None, &HookEvent::Stop, 5).unwrap().state, Idle);
        let b = next(None, &HookEvent::Blocked("permission_prompt".into()), 5).unwrap();
        assert_eq!((b.state, b.reason.as_deref()), (BlockedOnHuman, Some("permission_prompt")));
    }

    #[test]
    fn a_change_resets_since_and_heartbeat() {
        let n = next(Some(&st(Working, 0, 0)), &HookEvent::Stop, 100).unwrap();
        assert_eq!((n.changed_at_ms, n.heartbeat_at_ms), (100, 100));
    }

    #[test]
    fn same_state_only_rewrites_for_a_due_heartbeat() {
        let prev = st(Working, 0, 0);
        assert_eq!(next(Some(&prev), &HookEvent::ToolUse, HEARTBEAT_EVERY_MS - 1), None);
        let n = next(Some(&prev), &HookEvent::ToolUse, HEARTBEAT_EVERY_MS).unwrap();
        assert_eq!((n.changed_at_ms, n.heartbeat_at_ms), (0, HEARTBEAT_EVERY_MS));
    }

    #[test]
    fn a_tool_use_after_a_prompt_unblocks() {
        let blocked = HookState {
            reason: Some("permission_prompt".into()),
            ..st(BlockedOnHuman, 0, 0)
        };
        let n = next(Some(&blocked), &HookEvent::ToolUse, 10).unwrap();
        assert_eq!((n.state, n.reason), (Working, None));
    }

    #[test]
    fn args_parse() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(HookEvent::parse(&a(&["prompt-submit"])), Some(HookEvent::PromptSubmit));
        assert_eq!(HookEvent::parse(&a(&["tool-use"])), Some(HookEvent::ToolUse));
        assert_eq!(HookEvent::parse(&a(&["stop"])), Some(HookEvent::Stop));
        assert_eq!(
            HookEvent::parse(&a(&["blocked", "elicitation_dialog"])),
            Some(HookEvent::Blocked("elicitation_dialog".into()))
        );
        assert_eq!(HookEvent::parse(&a(&["blocked"])), None);
        assert_eq!(HookEvent::parse(&a(&["teleport"])), None);
    }

    #[test]
    fn keys_are_filesystem_safe_and_never_empty() {
        assert_eq!(status_key(Some("0f9c1d2e-3a4b")).as_deref(), Some("0f9c1d2e-3a4b"));
        assert_eq!(status_key(Some("../../etc/passwd")).as_deref(), Some("______etc_passwd"));
        assert_eq!(status_key(Some("")), None);
        assert_eq!(status_key(None), None);
    }

    #[test]
    fn the_dir_prefers_xdg_state_home() {
        assert_eq!(
            status_dir(Some("/x".into()), Some("/home/b".into())),
            Some(PathBuf::from("/x/claude-bus/status"))
        );
        assert_eq!(
            status_dir(None, Some("/home/b".into())),
            Some(PathBuf::from("/home/b/.local/state/claude-bus/status"))
        );
        assert_eq!(status_dir(None, None), None);
    }

    #[test]
    fn write_then_read_round_trips_and_garbage_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/s.json");
        let s = st(Idle, 1, 2);
        write(&p, &s).unwrap();
        assert_eq!(read(&p), Some(s));
        std::fs::write(&p, "not json").unwrap();
        assert_eq!(read(&p), None);
    }
}
```

`tempfile` is already a dev-dependency (it's used in `tests/web.rs`).

- [ ] **Step 2: Confirm they fail**

Run: `cargo test --lib agent::status`
Expected: compile error.

- [ ] **Step 3: Implement `src/agent/status.rs`**

```rust
//! The agent-side half of status: the state file the hook writes and the bridge reads.
//!
//! The hook (`claude-bus hook`) is a separate, short-lived process that Claude Code
//! spawns on lifecycle events. It cannot reach the bridge's memory, so the two meet
//! at a small JSON file keyed by Claude session id. The file is also what makes
//! status survive a bus restart: the last event's result is still on disk when the
//! bridge reconnects, even if no hook has fired since.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::proto::AgentState;

pub const HEARTBEAT_EVERY_MS: i64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookState {
    pub state: AgentState,
    pub changed_at_ms: i64,
    pub heartbeat_at_ms: i64,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    PromptSubmit,
    ToolUse,
    Stop,
    /// Carries the Notification matcher type that fired it, e.g. `permission_prompt`.
    Blocked(String),
}

impl HookEvent {
    pub fn parse(args: &[String]) -> Option<HookEvent> {
        match args.first().map(String::as_str)? {
            "prompt-submit" => Some(HookEvent::PromptSubmit),
            "tool-use" => Some(HookEvent::ToolUse),
            "stop" => Some(HookEvent::Stop),
            "blocked" => args.get(1).map(|t| HookEvent::Blocked(t.clone())),
            _ => None,
        }
    }
}

/// The file to write, or `None` when nothing changed and no heartbeat is due.
/// Skipping redundant writes is what keeps a `tool-use` hook cheap on every tool call.
pub fn next(prev: Option<&HookState>, ev: &HookEvent, now_ms: i64) -> Option<HookState> {
    let (state, reason) = match ev {
        HookEvent::PromptSubmit | HookEvent::ToolUse => (AgentState::Working, None),
        HookEvent::Stop => (AgentState::Idle, None),
        HookEvent::Blocked(t) => (AgentState::BlockedOnHuman, Some(t.clone())),
    };
    match prev {
        Some(p) if p.state == state && p.reason == reason => {
            (now_ms - p.heartbeat_at_ms >= HEARTBEAT_EVERY_MS).then(|| HookState {
                heartbeat_at_ms: now_ms,
                ..p.clone()
            })
        }
        _ => Some(HookState {
            state,
            changed_at_ms: now_ms,
            heartbeat_at_ms: now_ms,
            reason,
        }),
    }
}

/// The one place both sides derive the file name. The hook passes the `session_id`
/// from its stdin; the bridge passes `CLAUDE_CODE_SESSION_ID`. If Task 0's spike
/// showed those differ, this is the only function to change.
pub fn status_key(session_id: Option<&str>) -> Option<String> {
    let id = session_id?.trim();
    if id.is_empty() {
        return None;
    }
    Some(
        id.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
            .collect(),
    )
}

pub fn status_dir(xdg_state_home: Option<String>, home: Option<String>) -> Option<PathBuf> {
    match (xdg_state_home.filter(|s| !s.is_empty()), home) {
        (Some(x), _) => Some(PathBuf::from(x).join("claude-bus/status")),
        (None, Some(h)) => Some(PathBuf::from(h).join(".local/state/claude-bus/status")),
        (None, None) => None,
    }
}

pub fn read(path: &Path) -> Option<HookState> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Atomic: write a sibling temp file, then rename over the target, so the bridge's
/// poller can never read a half-written file.
pub fn write(path: &Path, s: &HookState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(s)?)?;
    std::fs::rename(&tmp, path)
}
```

`src/agent/mod.rs`: add `pub mod status;`.

- [ ] **Step 4: Run the unit tests**

Run: `cargo test --lib agent::status`
Expected: PASS.

- [ ] **Step 5: Write the failing end-to-end hook tests** (`tests/hook.rs`)

```rust
//! Runs the real binary the way Claude Code would: arguments, JSON on stdin, and an
//! inherited environment. Asserts what the agent's session would observe.

use std::io::Write;
use std::process::{Command, Stdio};

fn hook(state_home: &std::path::Path, args: &[&str], stdin: &[u8]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_claude-bus"))
        .arg("hook")
        .args(args)
        .env("XDG_STATE_HOME", state_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).ok();
    child.wait_with_output().unwrap()
}

#[test]
fn a_stop_hook_writes_idle_and_prints_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let out = hook(dir.path(), &["stop"], br#"{"session_id":"s1","hook_event_name":"Stop"}"#);
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "stdout reaches the model: {:?}", out.stdout);
    let file = dir.path().join("claude-bus/status/s1.json");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert_eq!(v["state"], "idle");
}

#[test]
fn bad_input_never_fails_or_prints() {
    let dir = tempfile::tempdir().unwrap();
    let big = vec![b'x'; 4 * 1024 * 1024];
    for (args, stdin) in [
        (vec!["stop"], b"".as_slice()),
        (vec!["stop"], b"not json".as_slice()),
        (vec!["stop"], br#"{"no_session":1}"#.as_slice()),
        (vec!["stop"], big.as_slice()),
        (vec!["teleport"], br#"{"session_id":"s"}"#.as_slice()),
        (vec![], br#"{"session_id":"s"}"#.as_slice()),
    ] {
        let start = std::time::Instant::now();
        let out = hook(dir.path(), &args, stdin);
        assert!(out.status.success(), "{args:?} failed");
        assert!(out.stdout.is_empty(), "{args:?} printed to stdout");
        assert!(start.elapsed() < std::time::Duration::from_secs(2), "{args:?} was slow");
    }
}

#[test]
fn an_unwritable_state_dir_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let file_in_the_way = dir.path().join("claude-bus");
    std::fs::write(&file_in_the_way, "").unwrap(); // a file where the dir should be
    let out = hook(dir.path(), &["stop"], br#"{"session_id":"s1"}"#);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}
```

- [ ] **Step 6: Implement `src/hook.rs`** and the `main.rs` arm

```rust
//! `claude-bus hook <event> [type]`: called by Claude Code hooks, never by people.
//!
//! The contract with Claude Code is strict. Hooks block the session until they exit,
//! and `UserPromptSubmit` stdout is injected into the model's context. So this never
//! writes to stdout, never makes a network call, always exits 0, and reads at most
//! 1 MiB of stdin. A diagnostic goes to stderr, which Claude Code shows only in
//! verbose mode.

use std::io::Read;

use crate::agent::status;

const MAX_STDIN: u64 = 1024 * 1024;

pub fn run(args: &[String]) {
    if let Err(e) = try_run(args) {
        eprintln!("claude-bus hook: {e}");
    }
}

fn try_run(args: &[String]) -> anyhow::Result<()> {
    let Some(ev) = status::HookEvent::parse(args) else {
        anyhow::bail!("unknown hook event {args:?}");
    };
    let mut buf = Vec::new();
    std::io::stdin().take(MAX_STDIN).read_to_end(&mut buf)?;
    let input: serde_json::Value = serde_json::from_slice(&buf)?;
    let Some(key) = status::status_key(input["session_id"].as_str()) else {
        anyhow::bail!("no session_id on stdin");
    };
    let Some(dir) = status::status_dir(
        std::env::var("XDG_STATE_HOME").ok(),
        std::env::var("HOME").ok(),
    ) else {
        anyhow::bail!("neither XDG_STATE_HOME nor HOME is set");
    };
    let path = dir.join(format!("{key}.json"));
    let now = crate::store::now_ms();
    if let Some(next) = status::next(status::read(&path).as_ref(), &ev, now) {
        status::write(&path, &next)?;
    }
    Ok(())
}
```

A 4 MiB stdin with `take(1 MiB)` leaves the writer blocked on a full pipe once we exit. That is fine: the test writes from the parent before waiting, and `write_all` errors with a broken pipe, which the test ignores (`.ok()`).

`src/lib.rs`: add `pub mod hook;`.

`src/main.rs`, before `_ => usage()`:

```rust
        Some("hook") => {
            // Synchronous and infallible by design; see `claude_bus::hook`.
            claude_bus::hook::run(&args[2..]);
            Ok(())
        }
```

Don't list `hook` in `usage()`. It isn't for people, and `init` installs it. Say so in a comment by the arm.

- [ ] **Step 7: Run the tests**

Run: `cargo test --test hook && cargo test --lib agent::status`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
cargo +nightly fmt
git add src/agent/status.rs src/agent/mod.rs src/hook.rs src/lib.rs src/main.rs tests/hook.rs
git commit -m "feat: claude-bus hook writes the per-session status file"
```

---

### Task 5: The bridge publishes status, the `status` tool, and resending on reconnect

**Files:**
- Modify: `src/agent/status.rs`: `LocalStatus` and `to_wire`
- Modify: `src/agent/bridge.rs`: `BridgeConfig.status_file`; a `watch` receiver in `connect_once`; the file poller
- Modify: `src/agent/mod.rs`: build the watch channel; derive `status_file`; new `run_on_with_options`
- Modify: `src/agent/handler.rs`: `status` in `BUS_TOOL_NAMES` (now 10) and `list_tools`; the `call_tool` arm; the `Handler.status` field
- Modify: `src/agent/instructions.rs`: one paragraph
- Modify: `src/init.rs`: only the doc comment ("nine" becomes "ten"). The allowlist follows `BUS_TOOL_NAMES` automatically
- Modify: `tests/common/mod.rs`: `InProcessAgent::start_with_status_file` and an `IsolatedBus` helper
- Test: `tests/agent_contract.rs` (new cases); unit tests for `to_wire`

**Interfaces:**
- Consumes: `HookState`, `status::read`, `status_key`, `status_dir`, and `ToBus::Status`.
- Produces:

```rust
// src/agent/status.rs
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalStatus { pub hook: Option<HookState>, pub text: Option<(String, i64)> }
impl LocalStatus { pub fn to_wire(&self, now_ms: i64) -> Option<ToBus> }
pub type StatusTx = std::sync::Arc<tokio::sync::watch::Sender<LocalStatus>>;
// src/agent/mod.rs
pub struct AgentOptions { pub liveness: bridge::Liveness, pub status_file: Option<Option<PathBuf>> }
pub async fn run_on_with_options<T, E, A>(transport: T, bus_url: String, name: String, opts: AgentOptions) -> anyhow::Result<()>;
```

`status_file: None` means derive the path from the environment. `Some(None)` turns the poller off. `Some(Some(path))` uses that file.

- [ ] **Step 1: Write the failing `to_wire` tests** (append to `src/agent/status.rs` tests)

```rust
    #[test]
    fn to_wire_sends_ages_and_whole_status() {
        let ls = LocalStatus {
            hook: Some(st(Idle, 1_000, 9_000)),
            text: Some(("done".into(), 7_000)),
        };
        match ls.to_wire(10_000) {
            Some(crate::proto::ToBus::Status {
                state,
                state_age_ms,
                heartbeat_age_ms,
                text,
                text_age_ms,
                ..
            }) => {
                assert_eq!(state, Idle);
                assert_eq!(state_age_ms, Some(9_000));
                assert_eq!(heartbeat_age_ms, Some(1_000));
                assert_eq!(text.as_deref(), Some("done"));
                assert_eq!(text_age_ms, Some(3_000));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn text_without_hooks_reports_unknown_and_nothing_reports_nothing() {
        assert_eq!(LocalStatus::default().to_wire(0), None);
        let ls = LocalStatus {
            hook: None,
            text: Some(("x".into(), 0)),
        };
        assert!(matches!(
            ls.to_wire(0),
            Some(crate::proto::ToBus::Status { state: Unknown, .. })
        ));
    }
```

- [ ] **Step 2: Implement `LocalStatus`**

```rust
/// Everything the bridge knows about its own status. The bridge always sends all of
/// it, never a diff (see `bus::status::Statuses::apply`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalStatus {
    pub hook: Option<HookState>,
    /// The text and when it was set, by this machine's clock.
    pub text: Option<(String, i64)>,
}

pub type StatusTx = std::sync::Arc<tokio::sync::watch::Sender<LocalStatus>>;

fn age(now: i64, then: i64) -> Option<u64> {
    u64::try_from(now - then).ok()
}

impl LocalStatus {
    pub fn to_wire(&self, now_ms: i64) -> Option<crate::proto::ToBus> {
        if self.hook.is_none() && self.text.is_none() {
            return None;
        }
        let h = self.hook.as_ref();
        Some(crate::proto::ToBus::Status {
            state: h.map_or(AgentState::Unknown, |h| h.state),
            state_age_ms: h.and_then(|h| age(now_ms, h.changed_at_ms)),
            heartbeat_age_ms: h.and_then(|h| age(now_ms, h.heartbeat_at_ms)),
            reason: h.and_then(|h| h.reason.clone()),
            text: self.text.as_ref().map(|(t, _)| t.clone()),
            text_age_ms: self.text.as_ref().and_then(|(_, at)| age(now_ms, *at)),
            waiting_on: None,
        })
    }
}
```

Run: `cargo test --lib agent::status`. Expected: PASS.

- [ ] **Step 3: Wire up the bridge**

`BridgeConfig` gains `pub status_file: Option<PathBuf>,`.

`run` gains `status: StatusTx` and passes it to `connect_once`. In `connect_once`, subscribe **before** sending `Register`. Subscribing marks the current value as seen, so the explicit send below is the first status this connection carries:

```rust
    let mut status_rx = status.subscribe();
    // … the existing Register send …
    // Re-establish status right after registering. On a fresh session this sends
    // nothing (there is no status yet); after a bus restart it restores the last
    // known state with its original age, so an idle agent comes back "idle since
    // 11:21" without waiting for its next hook.
    if let Some(cmd) = status_rx.borrow().to_wire(crate::store::now_ms()) {
        sink.send(Message::text(serde_json::to_string(&cmd)?)).await?;
    }
```

Add a `select!` arm. A `watch` channel holds only the newest value, so a burst of changes collapses into one send, and a stale update can never arrive after a fresh one. A queued `mpsc` would allow both:

```rust
            changed = status_rx.changed() => {
                if changed.is_err() { continue } // sender dropped: the poller is gone
                let cmd = status_rx.borrow_and_update().to_wire(crate::store::now_ms());
                if let Some(cmd) = cmd {
                    sink.send(Message::text(serde_json::to_string(&cmd)?)).await?;
                }
            }
```

`dispatch` already logs a `FromBus::Error` that has no pending request. Check that it doesn't panic or close anything on `req_id: None`. That is the old-bus case.

Add the poller to `bridge.rs`:

```rust
/// Watches the hook's state file. Polls rather than using inotify: a one-second
/// stat is cheap, works on every filesystem (including network homes), and needs
/// no new dependency. Only a changed `HookState` is published, so the heartbeat
/// rewrites (every 30s at most) are the only routine traffic.
pub async fn poll_status_file(path: PathBuf, status: StatusTx) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tick.tick().await;
        let latest = crate::agent::status::read(&path);
        status.send_if_modified(|s| {
            if s.hook != latest && latest.is_some() {
                s.hook = latest.clone();
                true
            } else {
                false
            }
        });
    }
}
```

`src/agent/mod.rs`: make `run_on_with_liveness` delegate to `run_on_with_options(transport, bus_url, name, AgentOptions { liveness, status_file: None })`. Move the body there and add:

```rust
    let status: StatusTx = std::sync::Arc::new(tokio::sync::watch::channel(LocalStatus::default()).0);
    // …
    let session_id = env.var("CLAUDE_CODE_SESSION_ID");
    let status_file = match opts.status_file {
        Some(explicit) => explicit,
        None => status::status_key(session_id.as_deref()).and_then(|key| {
            status::status_dir(env.var("XDG_STATE_HOME"), env.var("HOME"))
                .map(|d| d.join(format!("{key}.json")))
        }),
    };
    if let Some(path) = status_file.clone() {
        tokio::spawn(bridge::poll_status_file(path, status.clone()));
    }
```

Pass `status.clone()` into `Handler { status: status.clone(), .. }` and `bridge::run(..)`. Check that `RealEnv` has a `var` method for arbitrary keys. It does: `env.var("CLAUDE_PROJECT_DIR")` is already used.

- [ ] **Step 4: Add the `status` tool**

`BUS_TOOL_NAMES` becomes `[&str; 10]` with `"status"` appended. In `list_tools`:

```rust
                Tool::new(
                    Cow::Borrowed("status"),
                    Cow::Borrowed(
                        "Set a short line saying what you are doing or just finished, e.g. \
                         \"voice-fit run 3/5\" or \"wrote anchor.verified.json\". Anyone on the \
                         bus can read it without messaging you, and it never counts toward the \
                         exchange cap. Whether you are working, idle, or waiting on your human \
                         is reported automatically; this adds the what. Pass an empty string \
                         to clear it.",
                    ),
                    schema(json!({
                        "type": "object",
                        "properties": {
                            "text": { "type": "string", "description": "At most 200 characters; longer is truncated" },
                            "waiting_on": { "type": "string", "description": "Reserved; not yet used" }
                        },
                        "required": ["text"]
                    })),
                ),
```

Add `pub status: crate::agent::status::StatusTx,` to the `Handler` struct. In `call_tool`:

```rust
            "status" => {
                let Some(text) = s("text") else {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        "`text` is required (an empty string clears it)",
                    )]));
                };
                let now = crate::store::now_ms();
                let text = crate::bus::status::truncate_text(&text);
                self.status.send_modify(|st| {
                    st.text = (!text.is_empty()).then(|| (text.clone(), now));
                });
                if text.is_empty() {
                    text_of("status text cleared".to_string())
                } else {
                    text_of(format!("status set: {text}"))
                }
            }
```

`truncate_text` lives in `bus::status`, which is `pub(crate)`. Make that function `pub` so the bridge and the bus truncate the same way.

The tool doesn't wait for the bus. When disconnected, the status goes out on the next registration, which the reply text needn't mention.

`src/agent/instructions.rs`: before the final "Other tools:" paragraph, add:

```
Report progress with the `status` tool, not by messaging: "picked up X", "still working",
"done" are what `status` is for. Anyone can read it without interrupting you, and it does
not count toward the exchange cap the way messages do. Message someone only when they need
to read or act on what you say.
```

Add `status` to the "Other tools" list as well.

- [ ] **Step 5: Add the test helpers** (`tests/common/mod.rs`)

```rust
impl InProcessAgent {
    /// `start`, but with the status file pinned to `path` instead of derived from
    /// `CLAUDE_CODE_SESSION_ID` (which tests do not set).
    pub fn start_with_status_file(
        bus_url: impl Into<String>,
        name: impl Into<String>,
        path: std::path::PathBuf,
    ) -> Self {
        let (to_agent, from_agent, agent_stdin, agent_stdout) = Self::pipes();
        let (bus_url, name) = (bus_url.into(), name.into());
        let task = tokio::spawn(async move {
            if let Err(e) = claude_bus::agent::run_on_with_options(
                (agent_stdin, agent_stdout),
                bus_url,
                name,
                claude_bus::agent::AgentOptions {
                    liveness: claude_bus::agent::bridge::Liveness::default(),
                    status_file: Some(Some(path)),
                },
            )
            .await
            {
                eprintln!("[InProcessAgent] run_on exited with an error: {e}");
            }
        });
        Self {
            to_agent,
            from_agent,
            runner: Some(Runner::Shared(task)),
        }
    }
}

/// A bus on its own runtime, so dropping it really kills it, connections included.
/// Aborting the `serve_on` task alone would leave axum's per-connection tasks (and
/// so the agent's socket) alive on the shared runtime, and the agent would never
/// notice the "restart".
pub struct IsolatedBus {
    rt: Option<tokio::runtime::Runtime>,
    pub port: u16,
}

impl IsolatedBus {
    /// `port: 0` picks a free port. Pass a previous bus's port to restart "in place".
    pub fn start(dir: std::path::PathBuf, port: u16) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let listener = rt.block_on(async {
            // A just-dropped listener can take a moment to free its port.
            for _ in 0..50 {
                if let Ok(l) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                    return l;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            panic!("port {port} never became free");
        });
        let port = listener.local_addr().unwrap().port();
        rt.spawn(async move { claude_bus::bus::serve_on(listener, dir).await.unwrap() });
        Self { rt: Some(rt), port }
    }
}

impl Drop for IsolatedBus {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}
```

`IsolatedBus::start` calls `block_on`, which panics inside an async context. Call it from `tokio::task::spawn_blocking` in the async tests below.

- [ ] **Step 6: Write the failing contract tests** (`tests/agent_contract.rs`)

```rust
async fn status_of(port: u16, name: &str) -> serde_json::Value {
    let agents = common::get_json(port, "/api/agents").await;
    agents
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == name)
        .map(|a| a["status"].clone())
        .unwrap_or(serde_json::Value::Null)
}

#[tokio::test]
async fn the_status_tool_reaches_the_bus() {
    let (_dir, port) = common::start_bus().await;
    let mut a = common::InProcessAgent::start(format!("ws://127.0.0.1:{port}/ws"), "caas");
    common::initialize(&mut a).await;
    assert!(common::wait_until(|| common::agent_is_online(port, "caas")).await);
    a.send(json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
        "params":{"name":"status","arguments":{"text":"voice-fit run 3/5"}}}))
        .await;
    a.next_json().await;
    assert!(
        common::wait_until(|| async {
            status_of(port, "caas").await["text"] == "voice-fit run 3/5"
        })
        .await
    );
    // No hooks configured, so the state is honestly unknown.
    assert_eq!(status_of(port, "caas").await["state"], "unknown");
}

#[tokio::test]
async fn a_hook_file_change_reaches_the_bus() {
    let (_dir, port) = common::start_bus().await;
    let state = tempfile::tempdir().unwrap();
    let file = state.path().join("s.json");
    let mut a = common::InProcessAgent::start_with_status_file(
        format!("ws://127.0.0.1:{port}/ws"),
        "caas",
        file.clone(),
    );
    common::initialize(&mut a).await;
    let now = claude_bus::store::now_ms();
    claude_bus::agent::status::write(
        &file,
        &claude_bus::agent::status::HookState {
            state: claude_bus::proto::AgentState::BlockedOnHuman,
            changed_at_ms: now,
            heartbeat_at_ms: now,
            reason: Some("permission_prompt".into()),
        },
    )
    .unwrap();
    assert!(
        common::wait_until(|| async {
            status_of(port, "caas").await["state"] == "blocked_on_human"
        })
        .await
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn status_is_reestablished_after_a_bus_restart() {
    let data = tempfile::tempdir().unwrap();
    let dir = data.path().to_path_buf();
    let bus = tokio::task::spawn_blocking({
        let dir = dir.clone();
        move || common::IsolatedBus::start(dir, 0)
    })
    .await
    .unwrap();
    let port = bus.port;
    common::wait_until_bus_ready(port).await;

    let state = tempfile::tempdir().unwrap();
    let file = state.path().join("s.json");
    let hour_ago = claude_bus::store::now_ms() - 3_600_000;
    claude_bus::agent::status::write(
        &file,
        &claude_bus::agent::status::HookState {
            state: claude_bus::proto::AgentState::Idle,
            changed_at_ms: hour_ago,
            heartbeat_at_ms: hour_ago,
            reason: None,
        },
    )
    .unwrap();
    let mut a = common::InProcessAgent::start_with_status_file(
        format!("ws://127.0.0.1:{port}/ws"),
        "caas",
        file,
    );
    common::initialize(&mut a).await;
    assert!(common::wait_until(|| async { status_of(port, "caas").await["state"] == "idle" }).await);

    // Restart: a new process with the same data dir and the same port, and no memory.
    tokio::task::spawn_blocking(move || drop(bus)).await.unwrap();
    let _bus2 = tokio::task::spawn_blocking(move || common::IsolatedBus::start(dir, port))
        .await
        .unwrap();
    common::wait_until_bus_ready(port).await;

    // No hook fires. The status must come back from the file, with its original age.
    assert!(
        common::wait_until_timeout(std::time::Duration::from_secs(15), || async {
            status_of(port, "caas").await["state"] == "idle"
        })
        .await,
        "the bridge did not re-establish its status after reconnecting"
    );
    let since = status_of(port, "caas").await["since"].as_i64().unwrap();
    assert!(
        claude_bus::store::now_ms() - since >= 3_500_000,
        "the age was lost; since = {since}"
    );
}

#[tokio::test]
async fn a_renamed_session_reports_status_under_its_effective_name() {
    let (_dir, port) = common::start_bus().await;
    let _first = common::connect(port, "caas").await; // holds "caas"
    let mut a = common::InProcessAgent::start(format!("ws://127.0.0.1:{port}/ws"), "caas");
    common::initialize(&mut a).await;
    assert!(common::wait_until(|| common::agent_is_online(port, "caas#2")).await);
    a.send(json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
        "params":{"name":"status","arguments":{"text":"mine"}}}))
        .await;
    a.next_json().await;
    assert!(common::wait_until(|| async { status_of(port, "caas#2").await["text"] == "mine" }).await);
    assert!(status_of(port, "caas").await.is_null(), "the original caas must be untouched");
}

#[tokio::test]
async fn an_old_bus_refusing_status_does_not_break_the_agent() {
    // A minimal fake "old bus": answers Registered, refuses `status` as an unparseable
    // command (exactly what the real old bus does), and answers one `send` normally.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        while let Some(Ok(Message::Text(t))) = ws.next().await {
            let v: serde_json::Value = serde_json::from_str(&t).unwrap();
            let reply = match v["type"].as_str() {
                Some("register") => json!({"type":"registered","name":"caas"}),
                Some("status") => json!({"type":"error","req_id":null,
                    "message":"unparseable command: unknown variant `status`"}),
                Some("send") => json!({"type":"reply","req_id":v["req_id"],
                    "result":{"kind":"sent","room":"dm:b|caas","msg_id":1,
                              "delivered_to":["b"],"queued_for":[]}}),
                _ => continue,
            };
            ws.send(Message::text(reply.to_string())).await.unwrap();
        }
    });
    let mut a = common::InProcessAgent::start(format!("ws://127.0.0.1:{port}/ws"), "caas");
    common::initialize(&mut a).await;
    a.send(json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
        "params":{"name":"status","arguments":{"text":"x"}}}))
        .await;
    a.next_json().await;
    a.send(json!({"jsonrpc":"2.0","id":8,"method":"tools/call",
        "params":{"name":"send","arguments":{"to":"b","text":"hi"}}}))
        .await;
    let r = a.next_json().await;
    assert!(
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("delivered to b"),
        "{r}"
    );
}
```

If `common::initialize` or `next_json` for a tool call works differently (for example, if there are interleaved notifications), follow how the existing `send` contract tests in `tests/agent_contract.rs` read their replies.

Also update the existing contract test that pins `BUS_TOOL_NAMES` against `list_tools`. It should pass unchanged once `status` is in both. Update the "nine tools" comments in `init.rs` and `handler.rs` to say ten.

- [ ] **Step 7: Run**

Run: `cargo test --test agent_contract && cargo test --lib`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
cargo +nightly fmt
git add src tests
git commit -m "feat: bridge publishes status from hooks and the status tool, and resends on reconnect"
```

---

### Task 6: `claude-bus init` installs the hooks

**Files:**
- Modify: `src/init.rs`: `HOOKS` table, `merge_hooks`, `plan_merge`/`AllowlistStatus` counting, the dry-run and merge previews, and the module doc
- Test: unit tests in `src/init.rs`; extend `tests/init_dry_run.rs` with one case

**Interfaces:**
- Produces:

```rust
pub const HOOKS: [(&str, Option<&str>, &str); 6]; // (event, matcher, command)
pub fn merge_hooks(existing: Value) -> (Value, Vec<String>) // merged, and labels of the entries added
```

The hook entries, in Claude Code's settings format:

| Event | Matcher | Command |
|---|---|---|
| `UserPromptSubmit` | — | `claude-bus hook prompt-submit` |
| `PreToolUse` | — | `claude-bus hook tool-use` (or `PostToolUse`, if Task 0 said so) |
| `Stop` | — | `claude-bus hook stop` |
| `Notification` | `permission_prompt` | `claude-bus hook blocked permission_prompt` |
| `Notification` | `elicitation_dialog` | `claude-bus hook blocked elicitation_dialog` |
| `Notification` | `elicitation_url_dialog` | `claude-bus hook blocked elicitation_url_dialog` |

Each entry has `"timeout": 5`. The commands rely on `claude-bus` being on `PATH`, the same assumption the `claude mcp add … -- claude-bus agent` entry already makes.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod hook_merge_tests {
    use super::*;
    use serde_json::json;

    fn commands(v: &Value, event: &str) -> Vec<String> {
        v["hooks"][event]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|m| m["hooks"].as_array().into_iter().flatten())
            .filter_map(|h| h["command"].as_str().map(String::from))
            .collect()
    }

    #[test]
    fn adds_all_hooks_to_an_empty_file() {
        let (v, added) = merge_hooks(json!({}));
        assert_eq!(added.len(), HOOKS.len());
        assert_eq!(commands(&v, "Stop"), vec!["claude-bus hook stop"]);
        assert_eq!(commands(&v, "Notification").len(), 3);
        let perm = v["hooks"]["Notification"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["matcher"] == "permission_prompt")
            .unwrap();
        assert_eq!(perm["hooks"][0]["timeout"], 5);
    }

    #[test]
    fn is_idempotent_and_keeps_unrelated_hooks_and_keys() {
        let mine = json!({
            "theme": "dark",
            "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "notify-send done" } ] } ] }
        });
        let (once, _) = merge_hooks(mine);
        let (twice, added) = merge_hooks(once.clone());
        assert!(added.is_empty(), "second merge added {added:?}");
        assert_eq!(once, twice);
        assert_eq!(twice["theme"], "dark");
        let stop = commands(&twice, "Stop");
        assert!(stop.contains(&"notify-send done".to_string()));
        assert!(stop.contains(&"claude-bus hook stop".to_string()));
    }

    #[test]
    fn leaves_the_existing_human_active_hook_alone() {
        let v = json!({ "hooks": { "UserPromptSubmit": [ { "hooks": [
            { "type": "command", "command": "/path/to/human-active-hook.sh", "timeout": 5 } ] } ] } });
        let (merged, _) = merge_hooks(v);
        let ups = commands(&merged, "UserPromptSubmit");
        assert_eq!(ups.len(), 2, "{ups:?}");
    }

    #[test]
    fn a_non_object_hooks_key_is_replaced_not_crashed_on() {
        let (v, added) = merge_hooks(json!({ "hooks": "garbage" }));
        assert_eq!(added.len(), HOOKS.len());
        assert!(v["hooks"].is_object());
    }
}
```

- [ ] **Step 2: Implement**

```rust
/// The status hooks `init` installs. Matchers on `Notification` are what keep the
/// idle reminder (`idle_prompt`) from ever reaching us: only the prompt types that
/// genuinely wait on the human are listed, and the type travels as an argument so
/// no undocumented payload field is needed.
pub const HOOKS: [(&str, Option<&str>, &str); 6] = [
    ("UserPromptSubmit", None, "claude-bus hook prompt-submit"),
    ("PreToolUse", None, "claude-bus hook tool-use"),
    ("Stop", None, "claude-bus hook stop"),
    ("Notification", Some("permission_prompt"), "claude-bus hook blocked permission_prompt"),
    ("Notification", Some("elicitation_dialog"), "claude-bus hook blocked elicitation_dialog"),
    (
        "Notification",
        Some("elicitation_url_dialog"),
        "claude-bus hook blocked elicitation_url_dialog",
    ),
];

/// Add any missing status hook, keyed by its exact command string. Every other key,
/// every other hook (including a user's own command on the same event), and the
/// order of what was already there all survive untouched.
pub fn merge_hooks(existing: Value) -> (Value, Vec<String>) {
    let mut root = match existing {
        Value::Object(m) => Value::Object(m),
        _ => json!({}),
    };
    let map = root.as_object_mut().expect("object");
    let hooks = map.entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let hooks = hooks.as_object_mut().expect("object");
    let mut added = Vec::new();
    for (event, matcher, command) in HOOKS {
        let list = hooks.entry(event).or_insert_with(|| json!([]));
        if !list.is_array() {
            *list = json!([]);
        }
        let list = list.as_array_mut().expect("array");
        let present = list.iter().any(|m| {
            m["hooks"]
                .as_array()
                .is_some_and(|hs| hs.iter().any(|h| h["command"] == command))
        });
        if present {
            continue;
        }
        let mut entry = json!({ "hooks": [ { "type": "command", "command": command, "timeout": 5 } ] });
        if let Some(m) = matcher {
            entry["matcher"] = json!(m);
        }
        list.push(entry);
        added.push(match matcher {
            Some(m) => format!("hook {event}[{m}]"),
            None => format!("hook {event}"),
        });
    }
    (root, added)
}
```

Integrate it in `plan_merge`: after `let merged = merge_allowlist(existing, &tool_refs);`, run `let (merged, hooks_added) = merge_hooks(merged);` and extend `added` with `hooks_added`. In `build_scope_plan`, `total` becomes `tools.len() + HOOKS.len()`. Then `NothingToDo` means the tools *and* the hooks are present, and an existing install that has only the allowlist gets `AddAllowlistOnly`, which writes the hooks.

`allowlist_fragment` and `print_merge_preview` print `added`. Check that their wording ("N permission entries") still reads correctly, and change it to "N entries (permissions and status hooks)".

Update the module doc's settings paragraph to mention that the status hooks are merged here too, and why: "status hooks, so the bus can show whether this session is working, idle, or waiting on you".

- [ ] **Step 3: Run**

Run: `cargo test --lib hook_merge_tests && cargo test --test init_dry_run --test init_conflict --test init_reproduction`
Expected: PASS. Where an existing `init_*` test asserts the exact `added` count or the printed text, update its expectation to include the six hooks. The `HOOKS` table is the source; don't hand-copy the number into a test.

Then add one case to `tests/init_dry_run.rs`: a dry run against a project with no settings file prints `hook Notification[permission_prompt]`. Follow that file's existing harness for running a dry run and capturing its output.

- [ ] **Step 4: Commit**

```bash
cargo +nightly fmt
git add src/init.rs tests/init_*.rs
git commit -m "feat: claude-bus init installs the status hooks"
```

---

### Task 7: The console shows status

**Files:**
- Modify: `ui/src/data/live.ts`: emit `status` frames
- Modify: `ui/src/data/store.ts`: patch `rail.agents[i].status` on a `status` frame
- Create: `ui/src/ui/statusText.ts` (the TypeScript twin of `src/status_text.rs`) with `statusText.test.ts`
- Modify: `ui/src/rail/AgentRow.tsx` and `ui/src/rail/Rail.tsx` (a status line on each row; blocked sorts first) plus `Rail.module.css`
- Modify: `ui/src/agent/AgentHeader.tsx` (the status under the title)
- Modify: test fixtures that build `RailAgent`, `Agent` or `AgentDetail` (add `status: null`)
- Test: `ui/src/rail/Rail.test.tsx`, `ui/src/agent/Agent.test.tsx`, `ui/src/data/store.test.ts`

**Interfaces:**
- Consumes: the generated `StatusView`, `AgentState`, and the `FromBus` union (with its `status` member), plus `RailAgent.status`.
- Produces: `export function statusText(s: StatusView, online: boolean, now: number): string`

- [ ] **Step 1: Write the failing tests**

`ui/src/ui/statusText.test.ts`:

```ts
import { expect, test } from 'vitest'
import type { StatusView } from '../types/StatusView'
import { statusText } from './statusText'

const sv = (over: Partial<StatusView>): StatusView => ({
  state: 'idle',
  since: 0,
  last_heartbeat: null,
  reason: null,
  text: null,
  text_at: null,
  quiet: false,
  ...over,
})

test('matches the Rust renderer for the common cases', () => {
  expect(statusText(sv({ text: 'wrote anchor.verified.json' }), true, 5 * 60_000)).toBe(
    'idle 5m: wrote anchor.verified.json',
  )
  expect(statusText(sv({ state: 'working' }), false, 40 * 60_000)).toBe(
    'offline, was working 40m ago',
  )
  expect(statusText(sv({ state: 'working', quiet: true }), true, 25 * 60_000)).toBe(
    'working? (quiet) 25m',
  )
  expect(
    statusText(sv({ state: 'blocked_on_human', reason: 'permission_prompt' }), true, 120_000),
  ).toBe('blocked on human (permission_prompt) 2m')
})
```

`StatusView` keeps snake_case field names on the wire, because it is a `proto` type like `HistoryItem`. Check the generated `ui/src/types/StatusView.ts`, and if it came out camelCase, use those names.

In `Rail.test.tsx`, append:

```tsx
test('a blocked agent sorts first and shows its status line', () => {
  const blocked = {
    ...rail.agents[0],
    name: 'needs-me',
    online: true,
    lastSeen: 0,
    status: {
      state: 'blocked_on_human' as const,
      since: Date.now() - 120_000,
      last_heartbeat: null,
      reason: 'permission_prompt',
      text: null,
      text_at: null,
      quiet: false,
    },
  }
  renderWithStore(<Rail />, { rail: { ...rail, agents: [...rail.agents, blocked] } })
  const names = screen.getAllByTestId('agent-name').map((n) => n.textContent)
  expect(names[0]).toBe('needs-me')
  expect(screen.getByTestId('agent-status-needs-me').textContent).toMatch(
    /^blocked on human \(permission_prompt\)/,
  )
})

test('status text is rendered as text, never as HTML', () => {
  const a = {
    ...rail.agents[1],
    status: {
      state: 'idle' as const,
      since: Date.now(),
      last_heartbeat: null,
      reason: null,
      text: '<img src=x onerror=alert(1)>',
      text_at: Date.now(),
      quiet: false,
    },
  }
  const { container } = renderWithStore(<Rail />, { rail: { ...rail, agents: [a] } })
  expect(container.querySelector('img')).toBeNull()
  expect(screen.getByTestId(`agent-status-${a.name}`).textContent).toContain('<img')
})
```

In `store.test.ts`, add a test in the style of the existing presence-patch test. A `status` frame must replace the matching rail agent's `status` and leave the others untouched. Copy that test's setup: its fake `live` emitter and a rail containing two agents.

- [ ] **Step 2: Confirm they fail**

Run: `cd ui && npx vitest run src/ui/statusText.test.ts src/rail/Rail.test.tsx`
Expected: FAIL (the module is missing; the test ids are not found).

- [ ] **Step 3: Implement**

`ui/src/ui/statusText.ts`:

```ts
import type { StatusView } from '../types/StatusView'

// The twin of `src/status_text.rs`. The two must describe a status identically;
// statusText.test.ts pins the shared cases.
export function age(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  if (s < 60) return `${s}s`
  if (s < 3600) return `${Math.floor(s / 60)}m`
  if (s < 86400) {
    const m = Math.floor((s % 3600) / 60)
    return m === 0 ? `${Math.floor(s / 3600)}h` : `${Math.floor(s / 3600)}h${m}m`
  }
  return `${Math.floor(s / 86400)}d`
}

const WORD: Record<StatusView['state'], string> = {
  working: 'working',
  idle: 'idle',
  blocked_on_human: 'blocked on human',
  unknown: 'unknown',
}

export function statusText(s: StatusView, online: boolean, now: number): string {
  const since = age(now - s.since)
  let out = !online
    ? `offline, was ${WORD[s.state]} ${since} ago`
    : s.quiet
      ? `working? (quiet) ${since}`
      : s.state === 'blocked_on_human' && s.reason
        ? `${WORD[s.state]} (${s.reason}) ${since}`
        : `${WORD[s.state]} ${since}`
  if (s.text) {
    out += `: ${s.text}`
    if (s.text_at !== null && s.since - s.text_at > 10 * 60_000) {
      out += ` (text from ${age(now - s.text_at)} ago)`
    }
  }
  return out
}
```

`live.ts` `onmessage`: add `else if (msg.type === 'status') emit('status', msg)`.

`store.ts`: next to the presence handler, add:

```ts
  deps.live.on('status', (p) => {
    const frame = p as FromBus
    if (frame.type !== 'status') return
    if (!state.rail) return
    setState({
      rail: {
        ...state.rail,
        agents: state.rail.agents.map((a) =>
          a.name === frame.name ? { ...a, status: frame.status } : a,
        ),
      },
    })
  })
```

`Rail.tsx` `sortAgents`: blocked agents come first. They are addressed to the human, like "needs you" rooms.

```ts
const blockedFirst = (a: RailAgent) =>
  a.online && a.status?.state === 'blocked_on_human' ? 0 : 1
function sortAgents(agents: RailAgent[]): RailAgent[] {
  return [...agents].sort(
    (a, b) =>
      blockedFirst(a) - blockedFirst(b) ||
      Number(b.online) - Number(a.online) ||
      b.lastSeen - a.lastSeen,
  )
}
```

`AgentRow.tsx`: below the existing `rowLine` div, add the line below. It reuses the ticking `now` already passed in. Plain text in JSX is escaped by React, which is what the HTML test pins.

```tsx
      {agent.status && (
        <div
          className={`${styles.agentStatus} ${
            agent.online && agent.status.state === 'blocked_on_human' ? styles.agentBlocked : ''
          }`}
          data-testid={`agent-status-${agent.name}`}
        >
          {statusText(agent.status, agent.online, now)}
        </div>
      )}
```

`Rail.module.css`:

```css
.agentStatus {
  padding-left: 14px;
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--text-quaternary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.agentBlocked {
  color: var(--attention);
}
```

`--attention` already exists; it is used by the "needs you" chip. Truncating with an ellipsis is fine here because the full text is on the agent page.

`AgentHeader.tsx`: under the existing subtitle, add:

```tsx
      {agent.status && (
        <p className={styles.subtitle} data-testid="agent-detail-status">
          {statusText(agent.status, agent.online, now)}
        </p>
      )}
```

Add `status: null` to every fixture that builds `RailAgent`, `Agent` or `AgentDetail`. `npx tsc --noEmit -p .` lists them; it is the same mechanical pass the relayer feature needed. Add one `Agent.test.tsx` case: a detail with a `status` renders `agent-detail-status`.

- [ ] **Step 4: Run all the frontend checks**

Run: `cd ui && npm run typecheck && npm run format:check && npm test && npm run build`
Expected: all pass.

- [ ] **Step 5: Look at it**

Run a scratch bus (`claude-bus serve --port 17787 --data <scratch>`). Connect an in-process or raw websocket agent, send a `status` frame, open `http://localhost:17787/`, and check:
- the status line in the rail
- the line on the agent page
- the live update when a new status frame arrives
- the blocked row sorting first, in the attention colour

Delete the scratch data afterwards.

- [ ] **Step 6: Commit**

```bash
git add ui/src
git commit -m "feat: console shows agent status live"
```

---

### Task 8: Documentation and the final gate

**Files:**
- Modify: `docs/DEPLOY.md`: a new "Agent status" section; the "Reading the record" bullet list mentions status
- Modify: `README.md`: the command list gains `claude-bus status`; one sentence on status in the intro
- Modify: `contrib/human-active-hook.sh`: no change. Add a sentence to DEPLOY.md that it coexists with the status hooks.

- [ ] **Step 1: Write the DEPLOY section** (after "Which agents are running which version")

```markdown
## What each agent is doing

Each agent has a status: **working**, **idle**, **blocked on human** (sitting at a permission
or input prompt in its own terminal), or **unknown**, plus an optional line of text the agent
sets with its `status` tool ("voice-fit run 3/5"). Read it without messaging anyone:

- `claude-bus status`, a one-shot table, blocked agents first
- the `agents` tool, so any agent (hub, raven) can survey the fleet
- the console's sidebar and agent pages, live

The state comes from Claude Code hooks, which `claude-bus init` installs into
`.claude/settings.json`. They call `claude-bus hook …`, which writes a small file under
`$XDG_STATE_HOME/claude-bus/status/` (default `~/.local/state/…`), and the agent's MCP server
forwards it to the bus over its existing connection. The hooks never touch the network, never
print, and always exit 0. A session without them shows text only, with state "unknown".

Status never counts toward the exchange cap. It is held in memory on the bus. After a bus
restart, each agent resends its last status when it reconnects, so an agent idle since 11:21
still reads "idle since 11:21".

Signals worth knowing:

- **offline, was working** means the session died mid-work.
- **working? (quiet)** means no hook has fired for 10 minutes. Often this is a long-running
  command, but sometimes it is a stall.
- **blocked on human** is logged as `blocked_on_human` events (entry and exit only), so the
  event log shows how long each prompt waited.

Sessions started with `claude-bus launch` skip permission prompts entirely, so they never show
"blocked on human". That state is for interactive sessions.
```

- [ ] **Step 2: Run the full gate, in CI's unbuilt-bundle state**

```bash
find ui/dist -mindepth 1 ! -name .gitkeep -delete
cargo +nightly fmt --check
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
git diff --exit-code -- ui/src/types
(cd ui && npm ci && npm run typecheck && npm run format:check && npm test && npm run build)
cargo test   # again, now with the bundle built
```

Expected: everything passes in both states.

- [ ] **Step 3: Commit**

```bash
git add docs/DEPLOY.md README.md
git commit -m "docs: agent status"
```
