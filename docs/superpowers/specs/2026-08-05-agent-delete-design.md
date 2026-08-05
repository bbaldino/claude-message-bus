# Deleting an agent from the web UI

## Problem

The `agents` table keeps a row per *effective* name forever. `upsert_agent` writes
it at registration, `set_online(false)` flips it on disconnect, and nothing ever
deletes it. So a one-off name collision leaves a permanent tombstone:

```
network-debug    hardac  0.3.2  18:51:28  online
network-debug#2  hardac  0.3.2  18:38:33  offline
```

`network-debug#2` was minted by `Registry::attach` because a second connection
registered on the same host while the first still held the bare name. That
connection is long gone; the row is not, and there is no way to remove it short
of editing the database by hand.

The stale row is not purely cosmetic. `leave_all_rooms` documents why: an
agent's room membership is deliberately durable, because that is what makes
messages queue for it while it is away. A membership held by a name that will
never reconnect keeps reporting that name in `queued_for` on every later send,
telling other agents a reply is pending from someone who has gone.

## Scope

Add an explicit delete to the web UI, restricted to agents that are currently
offline.

**Deletes:** the `agents` row, the agent's `room_members` entries, its `cursors`
entries.

**Keeps:** `messages` and `events`. Room transcripts stay readable through the
`history` tool, and the audit log retains the record of how the name came to
exist.

**Out of scope:** bulk or automatic pruning, deleting online agents, a CLI
equivalent, and any change to how collisions are named in the first place.

## The read-only invariant this breaks, deliberately

`src/web/mod.rs` opens by stating that the web views perform no writes, for two
reasons: the UI cannot be the cause of a bug it is being used to investigate,
and the bus has no authentication, so anything the UI can do is available to
anything that can reach the port. The bus binds `0.0.0.0`, so the second reason
is live rather than theoretical.

This feature adds the first write endpoint. The offline-only restriction is what
keeps the trade acceptable: an unauthenticated caller can remove metadata for
connections that are already dead, but cannot disrupt a live session, cannot
drop a connected agent's memberships, and cannot alter any transcript. The
module doc must be updated to say the UI performs exactly one write and why,
rather than left asserting something no longer true.

## Routes

| Route | Method | Purpose |
|---|---|---|
| `/agents/{name}/delete` | `GET` | Confirmation page |
| `/agents/{name}/delete` | `POST` | Perform, then `303` to `/agents` |

The entry point is a `delete` link on the agent detail page (`GET
/agents/{name}`), not on the list. The list is where a tombstone gets noticed,
but routing a destructive action through the detail page is a deliberate speed
bump, and the detail page already computes the agent's room memberships.

### Confirmation page

Renders, before any mutation:

- name, host, last seen, current online state
- every room membership that will be dropped, by name
- the number of cursors that will be dropped
- an explicit line that messages and events are kept
- a POST button, and a cancel link back to `/agents/{name}`

### Guards

Applied on **both** routes:

- unknown name renders a "no agent named X" page, not a 500
- an online agent is refused; the confirm page shows the reason and renders no
  button, so there is never a button that is known to fail

The POST **re-checks liveness rather than trusting the GET**. Without it there is
a real gap: the confirm page loads for an offline agent, the agent reconnects
while the page is being read, and the POST drops the memberships of a live
agent — precisely what the offline-only rule exists to prevent.

Liveness is read from the in-memory registry, not from `agents.online`. The
column is persisted and reconciled at startup by `mark_all_offline`, but the
registry is what actually determines routability; a stale column could otherwise
admit a live agent.

`Registry::online()` today returns the whole sorted `Vec<String>`. Add
`Registry::is_online(&self, name: &str) -> bool` beside it.

## Store

Two new methods, returning two different shapes — the confirmation page needs
room *names* to display, while the audit event needs *counts* of what was
actually removed.

`Store::forget_agent(name) -> Result<ForgetCounts>` deletes from `agents`,
`room_members`, and `cursors` inside a single transaction. `ForgetCounts` carries
the rows removed from each table, for the audit event.

The store currently issues every query directly against `&self.pool` and opens
no transactions anywhere, so this introduces `pool.begin()`. It is warranted: a
partial failure that removes the `agents` row while leaving memberships behind
is the worst available outcome, because the row is what makes the agent visible
in the UI and therefore deletable at all. Losing it strands the memberships with
no route back to them.

`Store::agent_footprint(name) -> Result<AgentFootprint>` mutates nothing and
returns `{ rooms: Vec<String>, cursors: i64 }` — room names rather than a count,
because the confirmation page lists the memberships at risk individually. This
keeps that query out of the web layer, which would otherwise duplicate the
membership filtering the agent detail page already performs.

## Audit

The POST appends an `agent_deleted` event carrying the name, host, last-seen
timestamp, and the removed counts.

This is the reason `events` is preserved rather than deleted: once the `agents`
row is gone, that event is the only remaining record that the agent existed.

`summarize()` in `src/web/mod.rs` gains an arm for the new kind, so it renders as
a readable phrase rather than falling through to compact JSON.

## Testing

Following the store's existing async test pattern:

- deletes the `agents` row, its memberships, and its cursors, while leaving that
  agent's messages and events intact
- refuses an online agent, leaving every row untouched
- an unknown name produces a clean error rather than a panic
- the TOCTOU case: offline when the confirm page renders, online when the POST
  arrives, refused
- a `#2`-suffixed name survives the URL round trip. `encode_path_segment`
  already exists for this; `#` is exactly the character that would silently
  truncate a path if it were not used, and the tombstone case that motivates
  this feature always contains one.

## Consequences accepted

- The web UI is no longer write-free, and its module doc changes accordingly.
- An offline agent that is deleted and later reconnects comes back as a fresh
  row with no memberships. It rejoins rooms as a new member, and its cursors
  restart, so it may be re-delivered messages it had already seen.
- Nothing prevents deleting an offline agent that is merely idle rather than
  gone for good. The confirmation page listing the memberships at risk is the
  only safeguard, which is proportionate for an action expected to be rare.
