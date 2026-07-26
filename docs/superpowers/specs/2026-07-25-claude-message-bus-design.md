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

**This mechanism is verified working on this account** (POC 1, 2026-07-25 — see
`poc/probe/`). A session sitting idle received a pushed message, read its `meta`
attributes, and replied through a tool. Observed details:

- The startup banner confirms registration: `Channels (experimental) messages from
  server:probe inject directly in this session`.
- The event is injected **as a `user`-role message**, not a system reminder:

  ```
  <channel source="probe" probe_id="1" sent_at="1785012528480">
  PROBE_HELLO_MARKER — this arrived over the channel while the session was idle...
  </channel>
  ```

  This matters. Text from another agent arrives carrying the same authority as the
  human's own input. Instruction-based restraint would be asking the model to discount
  something indistinguishable from its user; the permission allowlist below is therefore
  the load-bearing control, not a secondary one.
- `meta` keys become tag attributes and are legible to the model — it echoed
  `probe_id=1` back through a tool, a value it could only have read off the tag.

Its limits:

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

**Verified in POC 1.** The MCP subprocess receives `cwd` = the project directory, so rule
4 works. Better, Claude Code exports these to the subprocess:

| Variable | Observed value |
| --- | --- |
| `CLAUDE_PROJECT_DIR` | `/home/bbaldino/work/claude-message-bus` |
| `CLAUDE_CODE_SESSION_ID` | the session's uuid |
| `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_PID`, `CLAUDECODE` | present |

So prefer `CLAUDE_PROJECT_DIR` over `cwd` for the `{dir}` substitution — it is explicit
and immune to any later working-directory change. And `CLAUDE_CODE_SESSION_ID` gives the
`agents.session_id` column a real value rather than a synthesized one.

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

`send` returns the full text it sent as its tool result, so the outbound half of a
conversation is recoverable rather than invisible.

**`send` must wait for a bus ack before returning.** POC 3 exposed this: with a
fire-and-forget send, the tool result reads `sent → gamma: …` even when gamma is offline
and the message was merely queued. The bus's "offline; queued" notice arrives
asynchronously, after the tool has already returned. Telling the model a message was
delivered when it was only queued is the kind of quiet lie that makes an agent wait
forever for a reply that was never coming. The real `send` issues a request and waits for
the bus to confirm `delivered` or `queued`, and says which.

**POC 1 refined what this buys us.** The echoed text does reach the model — it appeared
verbatim in the tool result and the model quoted it back. But the terminal collapsed the
call to a one-line `Called probe`, so the echo is *not* rendered on screen by default.
Local visibility of outbound messages therefore comes from two things, neither of which
is raw tool-result display:

1. The `instructions` string directs the agent to state what it sent in its own visible
   prose. Model output always renders. In POC 1 this worked unprompted by design.
2. `claude-bus tail <room>` remains the authoritative interleaved view of both halves.

Keep the echo anyway: it costs nothing and makes the transcript self-contained on replay.

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

  The default of 20 comes from POC 3, where a real negotiation converged in eight
  messages. It is a runaway backstop at ~2.5× observed length, not a working limit.
- **Model-enforced convention.** `send(done: true)` marks a topic resolved and signals no
  reply is expected. The `instructions` string teaches both mechanisms.

  POC 3 showed the models already self-terminate when instructed to ("no need to reply
  just to confirm"), so this is a clarity improvement rather than the primary control.
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

## Milestone 0: POC status

**POC 1 — channel connectivity. PASSED** (2026-07-25, `poc/probe/`). A Node MCP server
declaring `experimental['claude/channel']`, pushed into an idle interactive session,
which read the tag and replied through a tool. Resolved unknowns #1, #3, and #4; see the
verified findings inline above.

Two incidental results worth carrying forward:

- **Channels do not engage in headless `-p` mode.** The flag parses without error, but
  Claude Code never probes for the capability — the debug log enumerates the server as
  `{hasTools, hasPrompts, hasResources, hasResourceSubscribe}` and nothing more, and
  pushed notifications are silently discarded. Interactive sessions only. This is fine
  for the intended use, but it means no automated end-to-end test is possible; the
  end-to-end checklist stays manual.
- **The permission stall is real and easy to miss.** In POC 1 the reply sat unnoticed
  behind an approval prompt for 98 seconds. This is precisely why all eight bus tools are
  allowlisted rather than a subset.

**POC 2 — Rust port. PASSED** (`poc/rust-probe/`). No hand-rolled JSON-RPC needed:
`rmcp` 2.2.0 expresses both required pieces natively.

| Need | rmcp 2.2.0 |
| --- | --- |
| `claude/channel` under `experimental` | `ServerCapabilities.experimental: BTreeMap<String, JsonObject>`, set via `ServerCapabilities::builder().enable_experimental_with(..)` |
| Arbitrary outbound notification method | `ServerNotification::CustomNotification::new(method, params)`, sent with `Peer::send_notification` |

Verified on the wire, without involving Claude Code:

- `initialize` emits `{"experimental":{"claude/channel":{}},"tools":{}}` — byte-identical
  to the Node probe that passed POC 1.
- A POST produces a well-formed `notifications/claude/channel` carrying the right
  `content` and `meta` (`test-notify.mjs` asserts this).

Two implementation notes for the real binary: rmcp's model types are `#[non_exhaustive]`,
so they are built via builders or field assignment rather than struct literals; and
`Implementation::from_build_env()` reports *rmcp's* version, so set `name` and `version`
explicitly.

A live-session confirmation script is at `poc/rust-probe/run-test.sh`. Given the wire
bytes are identical to the already-passing Node probe, it confirms rather than
discovers.

**POC 3 — two-session round trip. BUILT, automated tests pass; live run pending**
(`poc/round-trip/`). One binary, two subcommands — `serve` and `agent` — as the real
design specifies, in-memory only. `test-roundtrip.mjs` drives a bus and three agent
processes over stdio and asserts the whole loop: registration and roster, a message
surfacing on another agent as a `notifications/claude/channel` with correct `content` and
`meta`, replies flowing back, and messages for an absent agent being held and delivered
on connect. All twelve checks pass.

It produced one design correction, already folded into the tools section above: `send`
must wait for a bus ack, or it reports delivery for messages that were only queued.

**The live two-session run PASSED** (`poc/round-trip/TRANSCRIPT.md`). Two sessions in
different project directories negotiated an RPC wire format. The human typed one prompt,
into one session. Four findings, all of which settle open questions:

- **They converge.** Eight messages, four from each side, then a clean stop. The final
  message ended *"Applying A1–A3 and I'd call v1 final; no need to reply just to
  confirm."* — the agent terminated the exchange itself, honoring the instruction to stop
  rather than acknowledge endlessly. No cap was needed to end it.
- **The exchange cap of 20 is now evidence-based** rather than a guess: 2.5× the observed
  length of a real negotiation. Keep it as a runaway backstop, not a working limit.
- **The discuss-only fence held, unprompted and without a permission stall.** Mid-exchange
  one agent wrote: *"Note this is a message draft, not a file in my repo; where it lands
  is my human's call."* It declined to write on its own reasoning. The permission
  allowlist never had to catch anything — which is the desired order: instructions first,
  permissions as the backstop.
- **The exchange had genuine adversarial value.** The receiving agent caught a real
  self-contradiction in the proposal (an error-code band defined one way and used
  another) and caught the proposer attributing its own design decisions to the JSON-RPC
  2.0 spec. This is review, not two agents agreeing with each other — which is the whole
  reason to build the thing.

## Out of scope

- Authentication and encryption. Trusted LAN, consistent with existing services.
- File versioning, history, and conflict resolution. Overwrite by key.
- Message editing or deletion.
- Reaching sessions that have exited. Channels require a live process.
- Bridging to non-Claude participants.
