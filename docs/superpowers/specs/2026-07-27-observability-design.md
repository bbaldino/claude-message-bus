# Observability — Design

**Date:** 2026-07-27
**Status:** Approved for planning
**Builds on:** `2026-07-25-claude-message-bus-design.md`

## Problem

`claude-bus tail` shows one room, live, if someone happens to be watching. Nothing answers
questions after the fact:

- What did those two agents actually agree yesterday?
- Which agent uploaded that artifact, and when?
- Did that message reach the recipient, or is it still queued?
- Why did this room stop delivering?

For a system whose premise is agents talking to each other unattended, being able to read
the record afterwards is the missing half.

## Two problems, not one

Most of what an audit or monitoring view needs is **already on disk**: `agents`, `rooms`,
`room_members`, `messages`, `files`, `cursors`. Who said what, when, in which room, which
artifacts and by whom, who is online and last seen. That is a reading problem.

What a *debugging* view needs is not recorded at all. `delivered_to` and `queued_for` are
computed per send, returned in the reply, and discarded. A room paused by the exchange cap,
a rate-limited send, a keepalive timeout, an agent reconnecting under a different effective
name — all go to stderr and evaporate. There is `online` and `last_seen`, but no history of
transitions.

So this project is an event log plus a set of read-only views over it and the existing
tables.

## Why the event log records everything

The scope decision is grounded in defects this project actually produced. Each would have
been visible:

| Defect | How the log shows it |
| --- | --- |
| `ToBus::Ack` had no producer; every cursor stuck at 0 | **By absence** — no ack events, ever |
| `send` reporting delivered for messages only queued | Per-recipient delivery outcome |
| Reconnecting agent silently becoming `caas#2` | Registration recording the effective name |
| Room paused at 20 with no visible cause | Pause events |
| Ghost agent online forever after a lost socket | Disconnect events carrying a reason |

The `Ack` case is why the log records mechanical churn and not only alarming events: the
signal was **absence**, and absence is only meaningful against an expectation. A log that
skipped "boring" events would not have shown it.

## The event log

```sql
CREATE TABLE IF NOT EXISTS events (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  created_at  INTEGER NOT NULL,
  kind        TEXT NOT NULL,
  agent       TEXT,
  room        TEXT,
  detail_json TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS events_room_id  ON events(room, id);
CREATE INDEX IF NOT EXISTS events_agent_id ON events(agent, id);
CREATE INDEX IF NOT EXISTS events_kind_id  ON events(kind, id);
```

Hybrid on purpose. The fixed columns are the axes every query filters on — "everything in
this room", "everything this agent did", "every pause" — so those stay plain indexed
lookups. The variable part lives in `detail_json`, so a new event kind never needs a
migration. `agent` and `room` are nullable because not every event has both.

### Kinds recorded

| Kind | `detail_json` carries |
| --- | --- |
| `message_sent` | `msg_id`, `delivered_to[]`, `queued_for[]`, `done` |
| `message_injected` | `msg_id`, whether the channel notification was written |
| `ack` | `last_delivered_id` |
| `agent_registered` | `requested_name`, `effective_name`, `host`, `session_id` |
| `agent_disconnected` | `reason` — socket closed, keepalive timeout, replaced |
| `room_joined` | — |
| `room_paused` | `count`, the cap in force |
| `rate_limited` | `retry_in_ms` |
| `resumed` | — |
| `file_stored` | `key`, `size`, `sha256` |
| `file_fetched` | `key` |

`agent_registered` records both the requested and effective name specifically so the
`caas` → `caas#2` collision is visible rather than inferred.

### Write discipline

Two rules, both non-negotiable:

**A logging failure must never fail the operation being logged.** The result is ignored
deliberately, with a comment saying so — matching the existing pattern in `handle`.

**The insert holds no lock and changes no control flow.** It sits beside the existing
`append_message` await. This is a leaf call, not a structural change: the two hot-path
defects this project hit (converting every send to `try_send`, adding `biased` to a select
loop) were changes to how messages flow and how tasks are scheduled. An insert is neither.

## Architecture

New routes on the existing axum server in `claude-bus serve` — same binary, same port,
same container, same Dockerfile. A new `src/web/` module owns HTTP handlers and HTML
rendering and reads through the existing `Store`. The protocol layer does not change.

Rejected: a separate service reading the same SQLite. Two things to deploy and
concurrent-access questions, for no benefit when the data is already in the process.

**Server-rendered HTML, no single-page app.** The Dockerfile is `rust:1-slim` →
`debian:stable-slim` with no Node anywhere. An SPA means a JavaScript toolchain in the
build stage, a bundler, and a second dependency tree — for a tool that displays tables of
text. Server rendering leaves the image and build untouched.

**No SSE initially.** Live views auto-refresh on a short interval. Real streaming means SSE
plumbing or a JS build, and for "who is online, what is happening" a few seconds of
staleness is not perceptible. Revisit only if it feels laggy.

## Pages

| Route | Shows |
| --- | --- |
| `/` | Agents with online state, rooms by last activity, recent events |
| `/rooms` | All rooms, member lists, last activity |
| `/rooms/:name` | The transcript, both halves interleaved, **with system events inline** — delivered vs queued, pauses, rate limits |
| `/agents` | All agents, online state, host, last seen |
| `/agents/:name` | Its rooms, its activity, its connect/disconnect history including collisions |
| `/rooms/:name/files` | Artifacts with uploader, size, hash; downloadable |
| `/events` | The raw log, filterable by kind, agent, room, time |

The room transcript is the view that does not exist today in any form. `tail` shows a live
room to whoever is watching; this shows what happened, afterwards, with the bus's own
behaviour interleaved against it.

## Read-only

The UI performs no writes. It cannot be the cause of a bug it is being used to
investigate, and with no authentication on the bus, anything the UI can do is available to
anything that can reach the port.

A control surface — resume, post as a human, disconnect an agent, delete a room — is
plausible later and explicitly out of scope now. The read layer should stay clean enough
that adding `POST` routes is additive rather than a rewrite, which needs no special
preparation beyond not doing anything odd.

**When that day comes, the no-auth question sharpens considerably.** Read-only makes it
moot; a delete button does not.

## Testing

- **Event writes**: assert each kind is emitted on the operation that should emit it, with
  the fields the table above promises. The `Ack` defect argues for testing that boring
  events fire, not only interesting ones.
- **A failing log write does not fail the operation** — the discipline is worthless
  untested.
- **Handlers**: integration tests over real HTTP against a temp SQLite, asserting rendered
  content rather than status codes alone. A page that returns 200 with an empty table has
  failed at its only job.
- **Interleaving**: a room with messages and events must render them in one correct
  chronological order, since that ordering is the entire value of the transcript view.

## Out of scope

- Any write, action, or control surface.
- Retention or roll-up. Events accumulate unbounded; at this volume that is fine for a
  long time, but it is deferred rather than solved.
- Full-text search. Filtering by room, agent, kind, and time covers the common questions;
  FTS is a schema addition worth its own decision.
- Authentication. Consistent with the existing design, and moot while read-only.
- Replacing `claude-bus tail`. It stays the live single-room view.
