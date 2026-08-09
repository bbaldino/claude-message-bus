# Agent detail and delete (phase 2d)

## Context

Phase 2c filled the console's main pane with a room transcript and an events dock.
Selecting an agent in the rail still lands on a placeholder. This phase builds the
agent detail screen and the delete flow that lives on top of it.

The delete is why this console effort started: a stale tombstone row (`network-debug#2`,
offline, never doing anything) had no way to be removed from the web UI.

### The decomposition, as it now stands

| | Piece | Status |
|---|---|---|
| 1 | Frontend infrastructure | done |
| 2a | Data layer | done |
| 2b | Shell — tokens, fonts, routing, rail, top bar | done |
| 2c | Room screen and events dock | done |
| **2d** | **Agent detail, delete modal, the agent empty state. This spec.** | |
| 2e | Files tab and the remaining empty states | |
| 2f | Composer | |
| 2g | Light mode | |

**2d was split at the files seam.** The handoff's detail group (4a–4f) is four screens
plus four endpoints, larger than either prior phase. The delete modal is specified as a
modal *over* agent detail, sharing its presence subscription, so those two cannot be
separated. The files tab is a tab in the room screen with its own endpoint and no
relationship to either — the only clean cut.

## The backend, which this phase needs and 2c did not

2c required no Rust changes. This one requires three endpoints.

**`GET /api/agents/{name}`** — the detail payload: identity (host, cwd, session,
version, last seen, online, human), a 20-bucket volume series (`message_buckets`
already supports this scope), the agent's rooms each with a message count and last
activity, and **the most recent 50 events plus the true total**, because the section
header reads "312 total" while the list shows a slice. Fifty is a chosen number, not a
derived one: enough that a normal agent's whole history fits and the cap never shows,
few enough that a chatty one does not ship thousands of rows to render a sidebar list.
Two new store queries: rooms-with-counts for an agent, and events-for-an-agent with a
count.

**`GET /api/agents/{name}/deletion`** — what a delete would remove, plus the agent's
current `online`. Largely a DTO over `agent_footprint`, which already exists and is
tested: it returns room names and a cursor count, and the registration count is simply
whether the row exists.

**`DELETE /api/agents/{name}`** — wraps the existing `forget_agent`.

### Why the counts are their own endpoint

The modal must state what will actually be removed, counted from the database **at
dialog open**. Folding those counts into the detail payload would make them as old as
the screen: leave agent detail open while the agent joins a room, then delete, and the
modal confidently reports 0 memberships while removing 1. A modal whose purpose is to
prevent deleting the wrong thing cannot carry a quietly stale count.

Rejected alternatives: one fat endpoint (the staleness above); returning counts on the
`DELETE` response (accurate, but it inverts the design — the operator is meant to see
consequences before typing the name, not after the row is gone).

### The server is the authority on refusal

`DELETE` re-checks `online` at the moment of deletion and returns a refusal the client
renders. The client never decides this. The race is real and known: the existing HTML
flow has a regression test for an agent coming back online between the confirm page and
the POST, written after it fired in CI.

**CSRF:** the existing POST carries an `Origin`-vs-`Host` check because the bus binds
`0.0.0.0` with no authentication. The JSON `DELETE` carries the same explicit check. A
cross-origin form POST cannot issue `DELETE`, which helps, but that is a property of
today's browsers rather than a decision we made, and this endpoint destroys rows.

### The existing HTML delete page stays

The handoff says `/agents/:name/delete` "should not exist", and it is right about the
shape. But it is the only delete available until the console is finished, and removing
it mid-effort is a regression in the UI actually in use. It goes when the old UI does.

## Agent detail

**The name wraps; it does not ellipsise.** `word-break: break-all` at mono 600 16px.
Names like `release-artifact-verifier#2@buildbox` run 36 characters, and an agent cannot
be identified from a truncated name. Beside it a `human` badge and an online/offline
state pill; below, "agent · seen 4s ago", or for a tombstone "agent · last seen 6h ago ·
never active in a room".

**First consumer of the `detail` volume strip variant** — 20 buckets at 44px, built in
2b and unused since. With no activity it renders a flat run of 8% bars captioned "no
messages in the last 100 min" rather than disappearing: a missing chart is
indistinguishable from a broken one.

**Identity is a definition list**, not a table row — host, cwd (`break-all`), full
session uuid, version, last seen. Version and last seen carry a secondary clause in dim
("0.3.3 · matches bus", "25 Jul 22:51:14 · 4s ago") so absolute and relative forms both
appear without a second column. The `differs` badge lands here, and **is buildable for
the first time**: agent detail can fetch `/api/meta` and compare, which the rail could
not.

**Rooms and event history** each carry a section header with a count; the event count is
the true total though the list is capped. An agent with no rooms gets a stated
explanation in a dashed box — "Never joined a room. Registered, then went quiet — the
usual signature of a session that was killed before it did any work" — not blank space.
The agent empty state (4f's third case) is one line: "Registered 4 seconds ago. Nothing
has happened yet."

**Two things that are design work rather than transcription:**

- The handoff warns the scrolling pane needs `flex: 1; min-height: 0` or it outgrows its
  track and pushes the footer — and therefore the delete button — off-screen entirely.
  It says this bit the prototype and will bite the implementation. Treat it as a
  requirement, not advice.
- Event kinds render "in their family colour", which requires a kind→hue mapping that
  does not exist yet: blue delivery, violet lifecycle, amber attention, red destructive,
  teal files, green presence.

**The parked volume-strip colour question stays parked.** The handoff gives active bars
as blue *or* green with no stated trigger anywhere, and an agent's strip was the natural
place to settle it. Both strips stay blue. Inventing a trigger the design does not state
is what this effort has declined to do three times, and this is not the phase to start.

## The delete modal

**A modal over the agent being viewed, not a route.** You cannot land on it cold by URL,
and you cannot lose the context you were deciding from.

**4c — confirmation.** The name echoed verbatim in a recessed block with `break-all`,
then "WILL BE REMOVED" over real counts — registration, memberships, cursors — with a
final greyed line stating that messages and files are kept because they belong to the
room. Then a text input requiring the name typed **exactly**, with live "6 characters to
go" progress. `delete` is disabled until it matches, and `Enter` is inert until then.

The typed confirmation is not merely friction: it forces the operator to reproduce the
`#2@buildbox` suffix themselves, which is exactly the part people get wrong when
deleting the wrong one of two similarly-named agents — the situation that started this
effort.

**4d — refused, and it must be real.** It exists only for the race where the agent came
back online between the screen loading and the dialog opening. It states the mechanism
rather than the rule — deleting a live agent would strip its memberships underneath it
and it would re-register on its next heartbeat — then gives numbered steps to stop the
session.

Its live-watch strip claims "this dialog updates itself", and that must be true: the
dialog subscribes to presence and **transitions itself into 4c** when the agent goes
offline. A dead end that makes you retry is the failure this design exists to fix. The
store already pushes presence, so this is wiring rather than new plumbing — but the
transition must **re-fetch the deletion counts**, because an agent can change the world
on its way out.

`Esc` closes both states.

## Cleanup, taken first

Carried out of 2c, and this is the phase for both:

- **`key={name}` on the room route.** It deletes the `prevRoom` ref, the `roomChanged`
  parameter threaded through `classifyArrival`, and the bug class behind the room-switch
  scroll bug — where a paging correction in flight applied one room's height delta to
  another's DOM node. This phase already touches routing to add agent detail, and 2f's
  composer will want per-room draft state, hitting the same seam.
- **Unify the store mocking in tests.** Three patterns exist across three files because
  `useStore` exports a module-level singleton built from the real `createLive`/
  `fetchRail`. This phase adds a batch of component tests; the alternative is a fourth
  copy.

## Testing

**Rust tests for all three endpoints**, including the online-refusal race explicitly.
The existing HTML flow's regression test for that race gives both precedent and a known
shape.

**Component tests for the modal**, where the interesting logic is: `delete` disabled
until the typed name matches exactly; `Enter` inert before that; `Esc` closes; a
presence push transitions 4d into 4c and triggers a counts re-fetch; a server refusal
renders 4d regardless of what the client believed.

**A manual pass, and this phase's most-specified behaviour is genuinely stageable** —
unlike the room state flags, which took three phases to be seen at all. Hold a chat
connection open so an agent is online, open the delete dialog to get the refused state,
kill the connection, and watch the dialog transition itself. The pass also covers the
footer trap, modal focus, and `Esc`.

**Safety:** this is the first phase adding a destructive endpoint. The manual pass really
deletes an agent, so it runs against a throwaway data directory, never a bus whose
contents matter.

## Deliverable

Selecting an agent renders its detail screen — identity, volume, rooms, event history —
in both live and tombstone forms. The delete modal counts real rows at open, refuses an
online agent while explaining the mechanism, transitions itself to confirmable when that
agent goes offline, and on completion removes the agent and returns to a console that
reflects it.

## Out of scope

The files tab and the `files · N` control in the room header; the new-bus and empty-room
empty states; the composer; light mode; removing the old HTML delete page. Named
explicitly so the implementation plan cannot quietly absorb them.

## Consequences accepted

- The console still cannot send; that waits for 2f.
- Two delete paths exist simultaneously — the old HTML page and the console modal —
  until the old UI is retired.
- The volume strip's idle/dead colours and the blue/green active split remain
  unimplemented, with no trigger stated by the design.
- The kind→hue mapping for event history is our invention, constrained by the handoff's
  hue families but not specified by it.
