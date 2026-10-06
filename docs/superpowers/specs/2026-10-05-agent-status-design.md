# Agent status

Status: draft for review. Nothing built.
Origin: a feature idea from the human, relayed by hub (msgbus DM, 2026-10-05), refined over two rounds.

## Problem

The bus shows whether an agent is connected, never what it is doing. To find out you have to message the agent and wait. That has three costs:

- **Progress traffic hits the exchange cap.** A lot of hub's traffic is "picked up X / still working / done". Those messages count toward the 20-message cap and have paused real rooms.
- **A silent stop goes unnoticed.** The respeaker voice-fit runs stopped after reporting, twice, and nobody could see it without asking.
- **Nobody can see who is waiting on the human.** A session sitting at a permission prompt looks exactly like a busy one.

## Goal

Give each agent a status that anyone (the human, the console, other agents) can read without sending a message:

- **state**, from a small fixed set
- **text**, a short line of free text
- **timestamps**, so stale information looks stale

## Design overview

Two layers, each set by whichever side actually knows the answer:

| Layer | Set by | Answers |
|---|---|---|
| state | Claude Code hooks, automatically | is it working, idle, or stuck at a prompt |
| text, waiting-on | the agent, via a `status` tool | what it is doing, and who it is waiting for |

The agent's bridge (the `claude-bus agent` MCP server) owns the agent's status. It sends the status to the bus over the websocket it already has. The bus keeps a copy in memory and serves it to readers.

### Why the state comes from hooks

The bus sees messages, not work, so a status inferred from sends would be a guess. "Last said X" is not "is doing X". Hooks fire on the real lifecycle events:

| Hook | State | Notes |
|---|---|---|
| `UserPromptSubmit` | working | the human typed a prompt |
| `PreToolUse` | working | also a heartbeat: proof of life during long turns, including turns started by a bus message, which may not fire `UserPromptSubmit` |
| `Stop` | idle | "finished a turn at 11:21" |
| `Notification` (permission or input prompt) | blocked-on-human | the session is waiting on the human in its terminal. Notifications that only mean "idle for a while" are ignored. **Verify during planning:** which field in the hook payload tells these apart. |

The agent never has to remember to report any of this. That is what makes this layer trustworthy.

## Hook → bridge → bus

```
Claude Code hook ──writes──▶ local state file ◀──polls── bridge ──websocket──▶ bus
                         (keyed by session id)          (owns status)       (in-memory copy)
```

- **Hook.** Each hook runs a new subcommand, `claude-bus hook <event>`, instead of a shell script.
  - It reads the session id from the JSON Claude Code passes on stdin.
  - It writes a small state file at `$XDG_STATE_HOME/claude-bus/status/<session_id>.json`, containing the state and when it last changed.
  - It never makes a network call and always exits 0. It cannot block, fail or slow a prompt beyond writing one file.
  - It only rewrites the file when the state changes, or when the last heartbeat is more than about 30 seconds old.
- **Bridge.**
  - It finds the same file using `CLAUDE_CODE_SESSION_ID`, which it already reads to register. **Verify during planning:** that this value equals the hook's `session_id`.
  - It checks the file's modification time about once a second, and sends `ToBus::Status` when the file changes.
  - It also holds the text and waiting-on fields, set by the `status` tool.
- **Bus.** Status arrives only over the agent's own authenticated websocket.
  - There is **no new HTTP write endpoint**. That matters on a bus whose only security boundary is the LAN.
  - The bus does not need to map session ids to agents: the connection already says who is speaking. Renames such as `caas#2` are handled for free.

### Installing the hooks

`claude-bus init` already writes `.claude/settings.json`. It gains the four hook entries, merged the same way it merges its other keys today. The human approved hook-based setup.

A session without the hooks still works. It shows text and online/offline, with state "unknown".

## Status survives bus restarts (the human's question)

**Yes, with full fidelity, including for an idle agent.** Hooks only fire on events, but their result is stored in the state file on the agent's machine, not only sent to the bus. When the bus restarts:

1. The bridge reconnects and registers, as it already does.
2. It immediately resends its current status: the state from the file, plus the text and waiting-on it holds in memory.

An agent that went idle at 11:21 comes back as "idle since 11:21". It does not have to wait for its next turn.

Edge cases:

- **The session restarted rather than the bus.** That means a new session id and an empty file, so the state is "unknown" until the first hook fires. That is correct: nothing is known about the new session yet.
- **Claude Code died.** The bridge is its child process and dies with it, so nothing reconnects with a stale file.

Status is otherwise not persisted on the bus. It lives in memory, alongside the registry. A database column would repeat the mistake of the persisted `online` flag, which showed agents from a dead bus as online.

## Timestamps and clock skew

Agents run on different machines, so the bridge never sends an absolute time. It sends **ages**: "state changed 4,210 seconds ago", "text set 300 seconds ago". The bus converts each age to a time on its own clock when it arrives. A machine whose clock is off can't make a status look fresh or ancient.

## Data shape

```
AgentStatus {
  state:        working | idle | blocked-on-human | unknown
  state_since:  bus time
  last_heartbeat: bus time         // the last hook signal of any kind
  text:         Option<String>     // capped at ~200 characters
  text_at:      Option<bus time>
  waiting_on:   Option<String>     // phase 2: an agent name, or "human"
  waiting_since: Option<bus time>  // phase 2
}
```

The bus stores this per effective agent name. It is kept when the agent disconnects, so "offline, last seen working" can be shown. It is dropped when the agent is deleted or the bus restarts.

## Staleness: derived when read, never stored

These are computed each time status is read, from the stored state, the timestamps and whether the agent is online:

- **Offline + last state working.** Shown as "offline · was working · 40m ago". This is the "died mid-work" signal. It is shown plainly, not hidden.
- **Working, no heartbeat for over 10 minutes.** Shown as "working · quiet 25m". It is a soft hint, not an alarm, because one long Bash command produces no hooks while it runs. This would have caught the respeaker stops.
- **Text much older than the current state.** The text is shown with its age ("text from 2h ago"), so old text doesn't read as current.

## Exchange cap

Status is not a message. It goes to no room, wakes no one and is never delivered, so it **never touches the exchange cap**.

The agent instructions (`src/agent/instructions.rs`) gain a line along the lines of: "report progress with `status`, not by messaging; message only when the other side needs to act". Without that line the habit of progress pings, and the cap pressure that comes with it, would continue.

## Waiting on an agent, vs. waiting on the human through one (phase 2)

The human wants a waiting-on-agent state if it can be reliably told apart from "waiting on a human, through another agent". Hub's example is waiting on dashboard while dashboard is waiting on the human.

### Rejected: inferring waiting from messages

The bus can't tell a question from a statement, and `human=true` only says who sent a message, not whether a reply is expected. Inference would be wrong in both directions.

### Proposed: explicit declaration plus resolving the chain

1. **Declaration.** The agent calls `status(text, waiting_on: "dashboard")`, or `waiting_on: "human"` when it has asked the human something by message. This is the one fact only the agent knows.
2. **Chain resolution, when status is read.** The bus follows `waiting_on` links, with cycle detection and a depth cap of about 8, and reports an *effective* state:

   | End of the chain | Effective state shown for hub |
   |---|---|
   | dashboard is working or idle | waiting on dashboard |
   | dashboard is blocked-on-human (stuck at a prompt) | **blocked on human, via dashboard (approval needed in its terminal)** |
   | dashboard has `waiting_on: "human"` | **blocked on human, via dashboard (asked you something)** |
   | dashboard is offline | waiting on dashboard, **which is offline** (flagged) |
   | the chain loops back on itself | **deadlock**: hub → dashboard → hub (flagged) |

   The two "blocked on human" endings stay distinct, so the human knows whether to go to a terminal or answer a message. The deadlock detection comes for free and catches agents waiting on each other.
3. **Clearing the wait, done by the bus.** A declared wait clears when the awaited party sends a message the waiting agent receives: a message from dashboard, or any `human=true` message for `"human"`. It also clears when the agent sets a new status. The agent therefore doesn't have to remember to clear it, and a forgotten wait can't stick.

### Why phase 2 rather than v1

- It depends on agents actually declaring waits. That is a new habit, and it's worth seeing how phase 1's `status` tool gets used first.
- The chain logic is the most complex part. Phase 1 already delivers most of the value: blocked-on-human from hooks, silent stops, and progress taken out of the cap.

Phase 1 should still reserve `waiting_on` in the protocol and tool schema, so phase 2 needs no breaking change.

## Event log

Record only **entering and leaving blocked-on-human**: one event each way, with the agent and the prompt kind. It's the state that matters most to the human, and its history ("dashboard waited 40m for approval") is worth having. Working/idle transitions would flood the log. This was hub's call too; the human didn't object.

## Where it's shown

Cheapest first:

1. **`agents` MCP tool and `GET /api/agents`.** State, text and ages, with the effective state once phase 2 lands. Hub and raven get this with no extra work.
2. **Console.**
   - A state chip and the text on each sidebar agent row, with blocked-on-human sorted to the top. Like "needs you" rooms, it is addressed to the human.
   - Full detail on the agent page.
   - Updates arrive live on the existing websocket, alongside `Presence`. No polling is added.
3. **`claude-bus status`.** A one-shot table in the CLI.
4. **Later, a separate decision:** a push notification when an agent becomes blocked-on-human. That is arguably the most valuable follow-on, but it has its own design questions: where to deliver it and how to throttle it.

## Phasing

**Phase 1**
- `claude-bus hook`, the state file, the bridge polling and sending it, and `ToBus::Status`.
- The in-memory store on the bus, resend on reconnect, and staleness derived when read.
- The `status(text)` tool, with `waiting_on` reserved but unused.
- The instructions change, `init` installing the hooks, and the blocked-on-human events.
- `agents` tool, API, console and CLI display.

**Phase 2**
- `waiting_on` declarations, chain resolution, the bus clearing waits, and deadlock and offline flags.

## Open items to verify during planning

1. The field in the `Notification` hook payload that tells "needs permission or input" apart from "idle reminder".
2. Whether `CLAUDE_CODE_SESSION_ID` (seen by the bridge) equals the hook's stdin `session_id`. If it doesn't, fall back to keying the state file by project directory plus the bridge's process id.
3. Whether a turn started by a channel message (a bus message) fires `UserPromptSubmit`. If it doesn't, `PreToolUse` covers it, but a turn that uses no tools would go from idle to idle. That is acceptable.
4. Whether `claude-bus hook` should also take over the existing human-active hook: posting over the bridge's websocket instead of plain HTTP, matched by session instead of by a name guessed from the directory. It is tempting, but it's out of scope here.
