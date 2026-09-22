# An HTTP participant mode for the bus

## Why

Today the only way onto the bus is the WebSocket protocol (`/ws`): register, then
send/receive/ack as a long-lived bidirectional connection. That fits a Claude Code
session, whose `claude-bus agent` bridge holds the socket open. It does not fit a
participant that can only make outbound HTTP requests.

The concrete driver is raven, bbaldino's plugin framework. raven plugins are WASM
components restricted to outbound `wasi:http` — they cannot open a WebSocket, run a
subprocess, or hold a socket. bbaldino wants raven's channels (Telegram, web chat)
to reach the bus, so messages he sends through raven land as **his** — carrying
human authority — while raven's own agent replies stay bot traffic subject to the
loop guards.

Rather than a standalone HTTP↔WS bridge process, this adds HTTP participation to
the bus itself, which already owns the store, rooms, cursors, registration, and the
guard logic. The plugin long-polls the bus directly, Telegram-getUpdates style.

## Model

HTTP participation is an **adapter over the existing internals**, not a parallel
path — the codebase's recurring failure mode is two code paths that drift (the
snake/camel split between `proto` and `web::api`, the two delete paths), and a
second send/delivery/guard implementation would be exactly that.

- **Store** — messages, the per-`(room, name)` `cursors` table, rooms, members,
  events. Unchanged.
- **`commands::handle`** — reused for send and resume, so HTTP inherits
  delivered/queued, the exchange cap, the rate limit, event logging, observer
  fan-out, and DM auto-join.
- **Registry** — a live lease holds a real Registry connection entry (an mpsc
  `Sender`), so presence and `send_to`'s delivered-vs-queued verdict keep working
  for an HTTP peer exactly as for a WS one.
- **Guards** — the exchange cap and rate limit apply to every HTTP send.

## Authority is session-level

Human authority is decided once, at registration, by a bus-configured relayer
secret — never per message. This is the bus's existing `--relayer` name-set
semantics, proven by a secret instead of by name:

- **Relayer lease** (registered with the correct secret): every message it sends
  is labeled with human authority (`has_human_authority = true`, so bus agents
  treat raven's requests as actionable), but `is_human` stays **false** for the
  guards — its sends still count against the exchange cap and do **not** clear a
  pause. Loop protection therefore applies to everything raven sends. A relayer
  clears a pause only through the explicit resume endpoint (or a human typing in
  the room).
- **Plain lease** (no secret): an ordinary bot — guard-bound, no authority label.

A relayer lease and a `--relayer` name-set entry then have identical semantics.
raven uses the secret path and is deliberately **not** added to the name-set:
because `has_human_authority = is_human || relayers.contains(me) || <lease relayer>`,
putting raven in the name-set would be a redundant second grant on the same
identity.

The trade-off, accepted deliberately: raven's agent-written replies also carry the
authority label, and raven cannot clear a pause on its own reasoning — only when
bbaldino tells it to (via the resume endpoint) or a human types in the room. raven
is bbaldino's delegate, so labeling everything it says with his authority is the
intended reading; the guards staying on protect against a delegated conversation
running away unattended.

## Conventions

- Base URL `http://<bushost>:7777`. JSON, camelCase (matching `/api`).
- A **token** returned at registration is sent on every later call in the header
  `X-Participant-Token` — never in the path or query, never logged, never written
  to `events`. It is a bearer capability for that lease's identity.
- The **same-origin guard** (`web::origin_matches_host`) applies to every endpoint
  below, including `GET .../receive` (it advances cursors). A request with no
  `Origin` (raven's bridge, curl) is allowed; a browser `Origin` must match `Host`.
  This mirrors the WS handshake and the existing hide/delete writes.

## Endpoints

### `POST /api/participants` — register / open a lease

Request: `{ "name": "raven" }`. Optional header `X-Relayer-Secret: <secret>`.

Response `200`:
```json
{ "name": "raven", "token": "<opaque>", "relayer": true, "leaseTtlMs": 120000 }
```
- `name` — the effective name; suffixed on collision (`raven#2`) unless reserved.
- `relayer` — whether the secret was accepted; the single source of truth for
  whether the lease carries authority.

Errors: `403` cross-origin; `403` a secret was presented but is wrong (fail loudly
— never silently downgrade to a bot lease, or raven's human messages would quietly
become bot messages); no secret presented → `200` with `relayer: false`.

### `GET /api/participants/receive?after=<id>&limit=<n>&timeout=<sec>`

Header `X-Participant-Token`. Long-poll: returns as soon as any room the
participant belongs to has messages with global id > `after`, or an empty batch at
`timeout`.

- `after` — last global message id processed; doubles as the ack. Absent/0 =
  everything currently unread per the stored cursors.
- `limit` — max messages (default 100, clamp `1..=1000`).
- `timeout` — long-poll seconds (default 30, clamp `..=50`).

Response `200`:
```json
{ "messages": [ { "id": 0, "room": "", "from": "", "body": "", "done": false, "human": false, "createdAt": 0 } ],
  "cursor": 0 }
```
- `messages` — id-ordered ascending, merged across all the participant's rooms.
- `cursor` — max id in this batch; the client passes it as the next `after`.

Semantics:
- **Presence** = lease valid (last activity within TTL), independent of whether a
  poll is parked right now, so a healthy peer between polls still reports as
  `deliveredTo`, not `queuedFor`.
- **Ack / cursor mapping** — `after=X` acks everything ≤ X. The single global
  offset fans onto the durable per-`(room, name)` cursors: for each room the
  participant is in, `set_cursor(room, name, max message id in that room with id ≤
  X)`. `set_cursor` is monotonic (`MAX`), so duplicate / reordered / late polls are
  safe.
- **Paging is loss-safe** because a batch is always a contiguous id-ordered prefix
  of the unread union: every message with id ≤ `cursor` was returned (ids are a
  monotonic autoincrement, so nothing with a lower id appears later), so acking ≤
  `cursor` never skips an unreturned message.

### `POST /api/participants/send`

Header `X-Participant-Token`.
```json
{ "target": { "kind": "room", "room": "..." }, "text": "...", "done": false }
```
`target` is `{ "kind": "room", "room": "..." }` or `{ "kind": "agent", "name":
"..." }`. No `human` field — authority is the lease's.

Response `200`, a discriminated `outcome` (all three are normal protocol outcomes,
mirroring the WS `ReplyResult` / `GuardVerdict`, not HTTP errors):
```json
{ "outcome": "sent", "room": "...", "msgId": 0, "deliveredTo": [], "queuedFor": [] }
{ "outcome": "rate_limited", "retryInMs": 0 }
{ "outcome": "paused", "room": "...", "count": 0, "reason": "..." }
```
`paused` / `rate_limited` are mandatory — without them a throttled or capped
message is silently misreported as sent. A relayer lease's send is labeled with
authority but still runs the guards, so it can return `paused` / `rate_limited`
like anyone.

### `POST /api/participants/resume` — relayer-only

Header `X-Participant-Token`. Request `{ "room": "..." }`.

Clears the room's exchange-cap pause (`guards.reset`) and appends a `resumed` event
(actor = the lease name, `via: "participant_resume"`). This is the relayer's escape
hatch when bbaldino says "continue" through another channel.

- **Relayer leases only.** A plain (bot) lease gets `403`: `ToBus::Resume` is
  ungated over WS only because every WS agent has a human behind it who can honor
  "resume once your human says to". An HTTP bot has no such human, so letting it
  resume would make the exchange cap toothless for pure bot traffic.
- `guards.reset` clears only the exchange counter, not the per-agent rate limit —
  raven is still under the 2s min-interval right after a resume.

### `POST /api/participants/join` — deferred

Out of scope for v1. DMs work without it: a `Send` targeting raven auto-creates
`dm:a|b` and enrolls raven, and `/receive` scans every room the participant is a
member of, so an inbound DM appears with no explicit join. Explicit room join is a
later addition.

## Lease & presence

- `leaseTtlMs` default 120000 (2 min), renewed by register / receive / send /
  resume.
- **Presence hysteresis**, so the events log and the console's online dot do not
  flap on the poll cadence:
  - First registration of a name → online + one `agent_registered` event.
  - Renewals emit no events.
  - Lease expiry (TTL elapsed with no activity) → offline + `agent_disconnected`
    (reason `lease_expired`), and the Registry entry is detached.
  - Re-appearance after a genuine expiry → online again. Only genuine
    online/offline transitions emit events.
- While the lease is valid the Registry entry exists, so `send_to` returns true →
  `deliveredTo`. After expiry, sends to the name queue (`queuedFor`) and are caught
  up via the durable cursor on the next `/receive`.

## Security

- **`--relayer-secret <s>`** — an opt-in bus flag. If unset, no HTTP participant
  can carry authority; every HTTP send is bot traffic. The secret is presented
  only at registration in `X-Relayer-Secret`; never in a URL, never logged, never
  persisted. On acceptance the *lease* is marked relayer (only the bit is kept, not
  the secret). This is the bus's first real credential — a leak means the ability
  to inject as bbaldino's authority and to resume rooms until it is rotated. Rotate
  by changing the flag; re-register for a fresh token.
- **Reserved names** — a name reserved to the relayer secret may be claimed only by
  a registration presenting the correct secret; anyone else requesting it is
  suffixed. Reservation gives raven a stable identity for attribution and for the
  durable name-keyed cursor across lease expiry.
- The same-origin guard on every endpoint (above).

## Defaults

| Setting | Default | Bound |
|---|---|---|
| `leaseTtlMs` | 120000 | — |
| receive `timeout` | 30s | ≤ 50s |
| receive `limit` | 100 | `1..=1000` |
| exchange cap | 20 | unchanged; applies to all HTTP sends |
| rate limit | 2000ms | unchanged; applies to all HTTP sends |

## The authority wiring

Make the guard-vs-authority distinction first-class rather than reconstructing
authority from a name lookup. Today `commands::handle` takes a bare `is_human:
bool` and computes `has_human_authority = is_human || app.relayers.contains(me)`.
Replace the bool with a small `Authority { human_present: bool, relayer: bool }`
(or add one `relayer: bool` parameter):

- `guards.check` gets `human_present` — false for raven; true only for a genuinely
  present human (the console's `human: true`).
- the label becomes `has_human_authority = human_present || relayer ||
  app.relayers.contains(me)`.

Existing call sites pass `relayer: false`, so nothing changes for WS. The HTTP send
handler passes `human_present: false, relayer: lease.is_relayer`. This preserves
the deliberate distinction documented at `commands.rs:152-177` — authority is
delegable, attendance is not — instead of inferring authority from a string set.

## Out of scope for v1

- `POST /api/participants/join` (DMs cover the first use).
- Any HTTP `Unread`-summary machinery — the cursor-driven `/receive` is the
  catch-up; the WS `FromBus::Unread` exists only to avoid replaying into an idle
  session, which HTTP does not need.
- Rotating the relayer secret without a restart.
