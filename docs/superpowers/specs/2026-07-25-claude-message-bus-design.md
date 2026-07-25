# claude-message-bus — Design

**Date:** 2026-07-25
**Status:** Approved for planning

## Problem

Several projects run concurrently, each in its own directory with its own Claude Code
session. When two projects intersect — a client and a server that must agree on a wire
format, say — the two agents have no way to talk. Coordination goes through the human,
who relays proposals by hand.

We want the agents to talk directly: an agent in project A can reach an agent in project
B, discuss a shared design, and exchange artifacts. The two agents may be on different
machines on the LAN.

## Key constraint: waking an idle session

Claude Code hooks are reactive to events inside a session. A session sitting idle at its
prompt generates no events, so no hook fires, and nothing external can reach it. Hooks
alone cannot deliver a message to an idle agent.

**Channels** can. A channel is an MCP server that declares
`capabilities.experimental['claude/channel'] = {}` in its `initialize` response, which
makes Claude Code register a notification listener. The server may then emit
`notifications/claude/channel` at any time — including while the session is idle. The
event lands in Claude's context as a `<channel>` tag, Claude reads it and acts, and a
`reply`-style tool lets it respond.

This is the mechanism the whole design rests on. Its limits:

- **Research preview.** Custom channels are not on the Anthropic-curated allowlist, so
  sessions must launch with `--dangerously-load-development-channels server:msgbus` and
  acknowledge a warning dialog. The flag syntax and protocol contract may change.
- **The session must be open.** Channels survive idle, not exit.
- **Anthropic auth only** — claude.ai or Console API key. Not Bedrock, Vertex, or
  Foundry. Team/Enterprise orgs must have `channelsEnabled` set by an Owner.
- **Notifications are unacknowledged.** `mcp.notification()` resolves when the message
  hits the transport, not when Claude processes it. If the session did not load the
  server as a channel, or org policy blocks it, events are dropped silently with no
  error to the sender.
- **Events batch.** Several notifications arriving while Claude is busy are delivered
  together on the next turn.

## Architecture

One Rust crate, two subcommands.

```
┌ machine A ─────────────┐      ┌ LAN ──────────┐      ┌ machine B ──────────┐
│ claude  (cwd: caas)    │      │               │      │ claude (dashboard)  │
│   └ claude-bus agent ──┼──WS──┤ claude-bus    ├──WS──┼─ claude-bus agent   │
│        (stdio MCP)     │      │  serve        │      │      (stdio MCP)    │
└────────────────────────┘      │ sqlite + blobs│      └─────────────────────┘
                                └───────┬───────┘
                                        │  claude-bus tail <room>
```

**`claude-bus serve`** — the LAN server. Owns the agent registry, rooms, message log,
and file store. Runs in Docker with one volume. Single process, single writer.

**`claude-bus agent`** — spawned by Claude Code as a stdio MCP subprocess, one per
session. Declares the channel capability, holds a WebSocket to the bus, injects inbound
messages into its session, and exposes the outbound tools. Its process lifetime *is* the
agent's presence: connect on spawn, offline when the socket drops.

**`claude-bus tail <room>`** — a read-only client for watching a conversation.

WebSocket rather than SSE+POST: traffic is symmetric and push-heavy in both directions.
Reconnect with exponential backoff; resume by cursor.

### Storage

SQLite via `sqlx`, blobs on disk at `blobs/<sha256>`.

SQLite over the LAN Postgres deliberately. The bus's entire value is being reachable; a
Postgres restart or upgrade would take the agents' ability to talk with it. Write volume
is a few rows per conversation. `sqlx` keeps a Postgres swap cheap later — same queries,
different driver — and blobs stay on disk either way.

## Data model

| Table | Columns |
| --- | --- |
| `agents` | `name`, `host`, `cwd`, `session_id`, `connected_at`, `last_seen`, `online` |
| `rooms` | `name`, `mode` (default `discuss`), `created_at` |
| `room_members` | `room`, `agent_name` |
| `messages` | `id` (monotonic per room), `room`, `from_agent`, `body`, `done`, `created_at` |
| `files` | `room`, `key`, `sha256`, `size`, `content_type`, `updated_by`, `updated_at` |
| `cursors` | `room`, `agent_name`, `last_delivered_id` |

Membership is keyed by **agent name, not session id**, so closing and reopening a session
rejoins its rooms.

A DM is a room, auto-created on first use, named `dm:<a>|<b>` with members sorted. One
concept in the implementation, two in the API.

`rooms.mode` exists now with the single value `discuss`. It is the extension point for
autonomy levels (see *Autonomy posture*); no other mode is implemented.

## Agent identity

Claude Code does not supply a name. The agent process picks one at startup. Resolution
order, first match wins:

1. `--name <n>` — explicit arg in `.mcp.json`
2. `CLAUDE_BUS_NAME` environment variable
3. `--name-template <t>` — substitutes `{dir}`, `{host}`, `{user}`
4. Default: `{dir}`, the basename of the process's cwd

The result is sanitized: lowercased, non-alphanumerics collapsed to `-`. Names appear
inside `<channel from="...">` attributes and inside DM room keys, so they must stay tame.

Rule 4 depends on the MCP subprocess inheriting Claude Code's launch directory. The
channels documentation uses a relative path in its `.mcp.json` example, which implies it
does. Prefer `CLAUDE_PROJECT_DIR` when set; that variable is confirmed for hooks but not
for MCP servers. **The spike resolves this.** Rules 1–3 are unaffected either way.

Typical user-scope config:

```json
{
  "mcpServers": {
    "msgbus": {
      "command": "claude-bus",
      "args": ["agent", "--bus", "ws://nas.lan:7777"]
    }
  }
}
```

### Collisions

- **Same name, different hosts.** Both register and keep their names. `send(to: ...)`
  fails loudly — `ambiguous: dashboard@lisa, dashboard@nas` — and the qualified
  `name@host` form is always accepted as an address. No silent renaming.
- **Two sessions in one directory.** The second becomes `dashboard#2`, visible in
  `agents()`. Pin with `--name` to make them meaningful.

## Tools exposed to Claude

| Tool | Purpose |
| --- | --- |
| `send(room \| to, text, done?)` | Post a message to a room or DM an agent |
| `history(room, limit)` | Fetch prior messages |
| `rooms()` | List rooms and their members |
| `agents()` | List registered agents and online status |
| `join(room)` | Join or create a room |
| `put_file(room, key, path \| content)` | Store an artifact; exactly one of `path`/`content` |
| `get_file(room, key)` | Retrieve an artifact |
| `list_files(room)` | List artifacts in a room |

`send` returns the full text it sent as its tool result. Claude Code deliberately hides
outbound channel reply text from the terminal, so without this echo the human watches
only half the conversation. *(That tool results render in the terminal is an inference
from normal tool-call display, not from the channels documentation — verify in the
spike.)*

## Message flow

Inbound events are injected as:

```
<channel source="msgbus" room="caas_protocol" from="dashboard" msg_id="42">
proposed frame: 4-byte length prefix, then msgpack
</channel>
```

`meta` keys must be identifiers — letters, digits, underscores — so `room`, `from`, and
`msg_id` are safe. Keys with hyphens are silently dropped by Claude Code.

Each agent holds a per-room cursor. The bus pushes messages with `id > cursor`; the agent
advances the cursor after emitting the notification.

**Reconnect does not replay the backlog.** An agent returning after hours offline gets a
single summary event — `3 unread in caas-protocol` — and calls `history()` if it cares.
Dumping yesterday's conversation into a fresh session wastes context and derails whatever
the human actually sat down to do.

## Autonomy posture

Inbound messages are a **conversation, not instructions**. The `instructions` string in
the server's `initialize` response tells Claude it may read files, reason, and reply, but
must not edit, write, or commit on another agent's say-so without the human present.

This is enforced by permissions, not just by asking the model nicely. The tools
allowlisted in `settings.json` are exactly the discuss-only set:

```json
{
  "permissions": {
    "allow": [
      "mcp__msgbus__send",
      "mcp__msgbus__history",
      "mcp__msgbus__rooms",
      "mcp__msgbus__agents",
      "mcp__msgbus__join",
      "mcp__msgbus__put_file",
      "mcp__msgbus__get_file",
      "mcp__msgbus__list_files"
    ]
  }
}
```

All eight bus tools are allowlisted so an unattended exchange never stalls. None of them
writes to the local repository: `put_file` and `get_file` move bytes to and from the bus,
and applying a retrieved artifact to disk still requires `Write`.

`Edit`, `Write`, and `Bash` are absent, so an agent talked into modifying its repo stops
and asks. The prompt-injection surface between two agents is fenced by the permission
system rather than by model compliance.

The one residual surface is `put_file(path:)`, which reads local disk and uploads it — an
agent could in principle be talked into sharing a file it shouldn't. On a trusted LAN
running your own agents this is acceptable; if it ever isn't, restrict `put_file` to the
`content` form and drop `path`.

Extending to a fully autonomous mode later changes only two things: the injected
instruction text, and which tools are allowlisted. Transport, addressing, storage, and
the file store are all mode-agnostic. The cost of autonomy is not in the bus — it is that
unattended writes require permission relay or `--dangerously-skip-permissions`, a
deployment decision.

## Runaway guards

Two agents replying to each other will volley indefinitely, each reply triggering the
other's channel. Overnight that is real money.

- **Bus-enforced cap.** After N consecutive exchanges in a room with no human input
  (default 20), the bus stops delivering and injects a single "paused — check with your
  human" notice. Resuming is an explicit tool call. Optionally a one-line
  `UserPromptSubmit` hook pings the bus to reset the counter whenever the human types.
- **Model-enforced convention.** `send(done: true)` marks a topic resolved and signals no
  reply is expected. The `instructions` string teaches both mechanisms.
- **Rate limit.** A minimum interval between messages from one agent to one room.

## Error handling

- **Bus unreachable at spawn.** The MCP server still starts and completes `initialize`;
  tools return a clear error and a background task reconnects with backoff. Session
  startup never blocks on the network.
- **Silently dropped notifications.** Claude Code gives the sender no error when channels
  are disabled or unregistered. The agent logs every emission to stderr (which lands in
  `~/.claude/debug/<session-id>.txt`), and the bus records *sent* and *injected*
  separately. Without this, "why didn't it arrive" is unfalsifiable.
- **Oversized blobs.** Cap at 50 MB, reject with a clear message.
- **Unknown room or agent.** Fail with the list of valid options rather than a bare error.

## Testing

- **Bus** — integration tests over real HTTP/WS against a temporary SQLite: room
  creation, membership persistence across reconnect, cursor replay, file round-trip,
  rate limit, exchange cap.
- **Agent contract test** — the important one. Drive `claude-bus agent` over stdio with a
  JSON-RPC harness and assert that `initialize` declares
  `experimental['claude/channel']`, that `tools/list` matches the table above, and that a
  message arriving from the bus produces a well-formed `notifications/claude/channel` on
  stdout with correct `meta` keys.
- **End-to-end** — two real Claude Code sessions, launched with the development-channels
  flag, verifying injection while idle. Not automatable; a scripted manual checklist.

## Milestone 0: the spike

Everything above except channel registration is ordinary CRUD. Channel registration is a
research-preview feature validated only against documentation. Before building a bus on
top of it, prove the premise:

A throwaway Rust stdio server that registers as a channel and injects one hardcoded
string into an idle session, launched with
`claude --dangerously-load-development-channels server:msgbus`.

It answers four questions:

1. Does the account's org policy permit channels at all?
2. Can `rmcp` express an experimental capability and an arbitrary outbound notification
   method — or do we hand-roll the stdio JSON-RPC? (The needed surface is small:
   `initialize`, `tools/list`, `tools/call`, plus outbound notifications. Roughly 300
   lines if the SDK fights us. The channels docs describe the MCP SDK as a requirement of
   their *examples*, not of the protocol; the wire format is plain JSON-RPC over stdio.)
3. Does the MCP subprocess inherit the project directory as its cwd, and is
   `CLAUDE_PROJECT_DIR` set for MCP servers?
4. Does a tool's return value render in the terminal, making the `send` echo work?

If channels turn out to be unavailable, the design changes fundamentally — back to
hooks, with no ability to reach an idle session — so this gate comes first.

## Out of scope

- Authentication and encryption. Trusted LAN, consistent with existing services.
- File versioning, history, and conflict resolution. Overwrite by key.
- Message editing or deletion.
- Reaching sessions that have exited. Channels require a live process.
- Bridging to non-Claude participants.
