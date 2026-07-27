# Hub and permission relay — Design

**Date:** 2026-07-27
**Status:** Approved for planning
**Builds on:** `2026-07-25-claude-message-bus-design.md`

## Problem

Several projects run concurrently, each with its own Claude Code session. `claude-bus`
lets those agents talk to each other, but the human still moves between terminals to
interact with any of them.

The goal is a single session — eventually driven by voice — from which the human can see
what every agent is doing, direct any of them, and move between them without switching
windows. Call it the **hub**.

Explicitly *not* a goal: agents acting on each other's instructions unsupervised. Workers
still ask before doing consequential work. The hub removes the human as *transport*, not
as *decision-maker*.

## Most of this already works

The switchboard needs no new code. A hub is an ordinary agent started with `--name hub`,
and the existing tool vocabulary covers the interaction:

| Need | Existing tool |
| --- | --- |
| What agents exist, who is online | `agents` |
| What conversations exist | `rooms` |
| Catch up on one | `history` |
| Ask or direct a specific agent | `send` |
| Watch a conversation from outside | `claude-bus tail` |

"Which project am I talking to right now" is conversational state in the hub's own head,
not bus state. Voice is the human's client concern; the bus never knows how the human
reached the hub session.

Nothing spawns anything. The hub is a peer with a better view, not a supervisor. Sessions
are started by the human, as they are today.

## The one thing that does not work: blocked approvals

If the human is in the hub and `caas` decides it needs to edit a file, `caas` raises a
permission prompt **in its own session**, which nobody is watching. It stalls silently.
"Workers still ask before acting" only holds if the asking reaches wherever the human is.

Claude Code supports this directly. A channel may declare
`capabilities.experimental['claude/channel/permission'] = {}` alongside `claude/channel`.
Claude Code then forwards tool-approval prompts as
`notifications/claude/channel/permission_request` with four string fields:

| Field | Meaning |
| --- | --- |
| `request_id` | Five lowercase letters, `a`–`z` excluding `l` |
| `tool_name` | e.g. `Bash`, `Write` |
| `description` | Human-readable summary of this call; the constant `Run shell command` when the model supplied none |
| `input_preview` | The arguments as JSON-shaped display text |

The channel answers with `notifications/claude/channel/permission` carrying `request_id`
and `behavior` of `allow` or `deny`. The local dialog stays open throughout; whichever
verdict arrives first wins and the other is dropped.

Treat `description` and `input_preview` as untrusted. Clients from v2.1.211 sanitise them,
but the guidance is explicit that earlier clients relay `description` raw.

## Design

### Opt-in, per project

An agent declares the permission capability **only when `--approver <name>` is set**:

```json
{
  "mcpServers": {
    "msgbus": {
      "command": "claude-bus",
      "args": ["agent", "--bus", "ws://nas.lan:7777/ws", "--approver", "hub"]
    }
  }
}
```

No flag, no capability, no widened surface. A project opts in by its own config, so no
setting elsewhere can redirect a session's approvals.

### The gate is the launch flag, not configuration

Sessions started with `--dangerously-skip-permissions` raise no prompts, so relay is inert
for them at zero cost. Sessions started without it route their prompts to the hub. The
human chooses per launch, and the mechanism needs no switch of its own.

**This rests on an assumption that must be verified** — see *Milestone 0*. The Claude Code
documentation says the flag bypasses "most" prompts, and that explicit ask rules and MCP
tools marked `requiresUserInteraction` still prompt. If some prompts survive the flag,
"inert in skipped sessions" is wrong and the hub will receive occasional prompts from
sessions the human believes are ungated.

### Flow

1. `caas` raises a prompt. Its agent process receives `permission_request`.
2. The agent forwards it to the bus with its own identity attached.
3. The bus mints its **own** request id, records `(bus_id, agent, claude_request_id, approver)`, and routes it to the declared approver.
4. The hub's session receives it as a channel event.
5. The human decides. The hub calls `approve(request, allow|deny)`, where `request` is the
   **bus-minted id** carried on the injected event — the hub never sees or handles Claude
   Code's `request_id`, which stays between the bus and the requesting agent.
6. The bus verifies the answering agent is that request's declared approver, then routes the verdict back.
7. `caas`'s agent emits `notifications/claude/channel/permission` with the original `claude_request_id`.

The bus mints its own ids rather than routing on Claude Code's five-letter ones — those
are designed for a human to retype on a phone, not to be unguessable, and two agents can
hold the same one simultaneously.

### Enforcement

The bus rejects a verdict from any agent other than the request's declared approver. This
is not LAN defence — see *Accepted risk* — it is so a routing bug cannot cross-wire two
agents' approvals, which would be nearly impossible to diagnose from either end.

### The hub's role is microphone, not deputy

The hub surfaces every prompt to the human and never decides on its own. It may summarise
or add context — it is a Claude session and that is useful — but the verdict is the
human's.

This is the load-bearing constraint. A hub that auto-approves is full autonomy routed
through a model's judgement of its own instructions, which is exactly the fence the
original design refused to rely on. The hub's `instructions` string must say so plainly.

### Expiry

A relayed request the human never answers leaves the worker stalled. The local dialog is
still open, so the session is recoverable by walking to that terminal, but the human
should not have to guess.

The bus expires a relayed request after a timeout (default 10 minutes), tells the hub it
expired, and stops accepting a verdict for it. It does **not** synthesise a `deny`: the
local dialog is still live, and denying on the human's behalf is a decision the hub is not
allowed to make.

### Interaction with the exchange cap

Permission traffic is not conversation and must not count toward the runaway cap or the
rate limit. A worker that raises twenty prompts is not a runaway exchange, and pausing a
room because of approvals would strand the very sessions waiting on the human.

## Accepted risk: no authentication

The bus has no auth by design — trusted LAN, consistent with the existing services. The
human has accepted this for relay as well.

Recording what it changes, so the decision is legible later rather than looking like an
oversight: today the worst a rogue LAN client can do is inject text that an agent may
refuse to act on. With relay enabled it can approve `Bash` in any session that opted in.
That is a different category of exposure, and it stops being acceptable the moment
anything on that network is not the human's own.

If that changes, the smallest sufficient fix is a shared token required **only** on the
relay path — verdict submission and capability declaration — leaving ordinary messaging
open. The design keeps those paths separable for that reason.

## Milestone 0: the relay probe

The last time this project designed from documentation alone, POC 1 discovered that
channels silently do not engage in headless `-p` mode — a fact absent from the docs, and
one that would have invalidated an automated end-to-end test strategy. Permission relay is
the same research preview and has never been exercised here.

Before any of this is built, a throwaway probe must answer:

1. **Does relay fire at all** on this account and client version?
2. **What do the four fields actually contain** for a realistic `Bash` and a realistic
   `Write` — particularly whether `description` is useful or the bare `Run shell command`
   constant, since that determines whether a human can decide from the hub without seeing
   `input_preview`.
3. **Does `--dangerously-skip-permissions` suppress every prompt**, or do some survive?
   This is the assumption the whole gating story rests on.
4. **Does a verdict sent over the channel actually satisfy the prompt**, and what happens
   to the still-open local dialog.
5. **What happens to an unanswered request** — does it stay open indefinitely?

If relay does not work, the switchboard still stands on its own: the hub remains useful
for everything except approvals, and gated projects keep being answered at their own
terminal.

## Out of scope

- The hub spawning or terminating sessions.
- Auto-approval of any kind, including "safe" subsets and remembered decisions.
- Per-room autonomy modes. `rooms.mode` remains an unused placeholder; this design does
  not need it, because the gate is the launch flag rather than a room property.
- Authentication, per *Accepted risk*.
- Voice. It is the human's client, not the bus's concern.
