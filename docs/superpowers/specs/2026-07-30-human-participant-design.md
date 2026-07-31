# Human participant — Design

**Date:** 2026-07-30
**Status:** Approved for planning
**Builds on:** `2026-07-25-claude-message-bus-design.md`, `2026-07-27-observability-design.md`

## Problem

Rooms hold any number of agents, and a room `send` already fans out to every member but
the sender. A human cannot be one of those members.

Today a human has two options, and neither is participation:

- `claude-bus tail <room>` shows the conversation live, but is read-only by construction —
  it identifies via `Observe`, which deliberately creates no `agents` row and no
  `room_members` row.
- Type into a Claude Code session and let that agent relay. The agent becomes transport,
  which adds a hop and a layer of interpretation to every sentence.

The goal is a human who is genuinely in the room: sends under their own name, receives
live, and appears in the transcript as themselves.

## A human is an agent with a flag

`ToBus::Register` gains one field:

```rust
Register {
    name: String,
    host: String,
    cwd: String,
    session_id: Option<String>,
    #[serde(default)]        // absent deserializes to false
    human: bool,
}
```

`#[serde(default)]` is load-bearing, not stylistic. A running Claude Code session holds a
stdio MCP subprocess spawned once at session start, and **Claude Code does not respawn
stdio servers mid-session** — verified against the documentation, which states plainly
that stdio servers are not reconnected automatically and that `.mcp.json` is read only at
startup. So every agent binary in flight when this ships must keep working untouched. An
absent field deserializing to `false` gives exactly that.

The `agents` table gains `is_human INTEGER NOT NULL DEFAULT 0`.

Rejected: a third identity kind alongside agent and observer. Every downstream reader —
registry, room membership, cursors, delivery accounting, the web views — would need to
learn a third case, to express something that is really one boolean about an otherwise
ordinary participant.

## Two lifetimes, deliberately different

| | Lifetime | Why |
| --- | --- | --- |
| `agents` row | Persistent | Past messages keep their attribution, and `/agents` can show the human as offline rather than forgetting them |
| `room_members` row | **Ephemeral** — created on join, deleted on disconnect | An agent must never see `queued_for: [bbaldino]` for someone who closed their terminal |

That second row is the one that matters. A human dipping into a room is not a subscriber:
if membership persisted, every subsequent room send would report the human as a pending
recipient, and agents would reasonably infer a reply was coming. Worse, the human would
accumulate an unread backlog they never asked for and would be told about on every
reconnect.

The teardown block in `connection()` already runs `detach` and `set_online(false)` on
disconnect; dropping a human's room memberships joins that sequence.

Catch-up comes from the transcript instead: on connect the CLI prints recent history, so a
human joining mid-conversation has context without the bus tracking a cursor for them.

## The CLI

```
claude-bus chat <room> [--bus ws://host:7777/ws] [--name bbaldino]
```

Connects, registers with `human: true`, joins the room, prints the **20 most recent
messages** for context, then streams live and sends each line typed. Twenty because it
matches the exchange cap — a human joining sees at most one full cap's worth of
conversation, which is the window the bus itself treats as one unattended stretch.

Realtime needs no new machinery. `tail` already receives `FromBus::Message` over the same
WebSocket the agents use; `chat` differs by registering instead of observing, and by
reading stdin. Server-push is what the transport already does.

`--name` defaults to `$USER`, falling back to `human` when that is unset — the same shape
as `resolve_name`'s existing chain for agents, which ends in a constant rather than
failing. A username reads naturally in a transcript and distinguishes two people if that
ever happens, where a fixed literal could not.

## The exchange cap learns what a human is

After 20 messages in a room with no human input, the bus pauses it. Unpausing today needs
`contrib/human-active-hook.sh` installed as a `UserPromptSubmit` hook, or an agent calling
`resume`.

The flag lets the bus do better. When the sender is human:

1. **The exchange counter resets.** A human speaking *is* the human-input signal the cap
   was built to detect — inferring it from a hook was always a proxy for this.
2. **A paused room un-pauses**, and the send goes through rather than being refused. This
   is the non-obvious one and it is essential: a paused room is precisely the situation a
   human most needs to speak into. A human whose message bounced off a pause could not
   rescue the conversation the pause was protecting them from.
3. **The per-agent rate limit does not apply.** A person typing is not a runaway loop, and
   being throttled mid-interjection would be maddening.

All three are consequences of one idea: the guards exist to stop agents from talking to
each other unattended. A human in the room is the condition they were watching for.

## Web surface

Minimal in this pass. On `/` and `/agents`, a human's row carries a visible marker
distinguishing it from a bot — the flag is already on the row, so this is presentation
only, no new query. The transcript needs no change: messages are already attributed by
name, and the human's name is in the agents list for anyone who wants to check.

No send box and no SSE. The CLI covers send-and-receive, and a send box without live
updates would be a poor conversation surface given no page currently auto-refreshes.
Deferred rather than rejected; the read layer stays additive-friendly, as before.

## The schema migration

**This is the project's first migration against a live database.** `schema.sql` is
entirely `CREATE TABLE IF NOT EXISTS`, and there is no `ALTER TABLE` anywhere in the
codebase. The deployed bus holds real data in a named Docker volume, so this runs against
populated tables, not a fresh file.

SQLite has no `ADD COLUMN IF NOT EXISTS`. The migration therefore reads
`PRAGMA table_info(agents)`, checks for `is_human`, and issues the `ALTER TABLE` only when
absent — idempotent by construction rather than by swallowing an error whose message could
change.

It must be tested against both a fresh database and one already populated, and running it
twice must be a no-op.

## Testing

- **Backward compatibility**: a `Register` payload with no `human` field deserializes with
  `human: false`. This is what protects the agent binaries that cannot be restarted.
- **Ephemeral membership**: a human joins, disconnects, and no `room_members` row remains —
  and a subsequent room send reports the human in neither `delivered_to` nor `queued_for`.
- **Cap reset**: a human send zeroes the exchange counter.
- **Pause bypass**: a send into an already-paused room succeeds, un-pauses it, and the
  message reaches the other members. A test that only checks the counter would miss this.
- **Rate limit bypass**: consecutive human sends inside the limit window all succeed.
- **Migration**: idempotent on a fresh database, on a populated one, and when run twice.
- **Events**: a human's registration records `is_human` in `agent_registered` detail, so
  the log distinguishes a person joining from a bot.

## Out of scope

- A web send box, and the SSE or refresh plumbing a live web conversation would need.
- Authentication. The human has accepted LAN-only trust for reads and writes alike; this
  changes nothing about that decision, which is recorded in the observability design.
- Multiple rooms per `chat` invocation. One room per session; run two.
- `done=true` from a human. The flag exists so agents converge; a person can simply stop
  typing.
- Replacing `tail`. It stays the read-only spectator view, and remains the right tool for
  watching a conversation you are not part of.
