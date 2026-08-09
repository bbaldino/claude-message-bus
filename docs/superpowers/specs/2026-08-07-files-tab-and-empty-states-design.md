# Files tab and empty states (phase 2e)

## Context

Phase 2d built the agent detail screen and the delete flow. What remains of the
handoff's detail group is the room files tab and two of the three empty states — the
third, an agent with no events, landed with 2d.

### The decomposition, as it now stands

| | Piece | Status |
|---|---|---|
| 1 | Frontend infrastructure | done |
| 2a | Data layer | done |
| 2b | Shell | done |
| 2c | Room screen and events dock | done |
| 2d | Agent detail, delete modal, agent empty state | done |
| **2e** | **Files tab, new-bus and empty-room states. This spec.** | |
| 2f | Composer | |
| 2g | Light mode | |

## The finding that reshaped this phase

The handoff's new-bus empty state — the one state it says "earns instruction", because
it is the first thing a new user sees — instructs them to run:

```
claude-bus register --name my-agent
```

**There is no `register` subcommand.** The mental model behind it is also wrong: agents
are not registered directly on this bus. `claude-bus init` writes the MCP config into a
project, and the agent registers itself when a Claude Code session launches there.

So the copy is wrong twice over, on the highest-stakes screen for being wrong. It is
also exactly the defect class 2d's final review identified — copy verified as *present*
rather than *true*. The manual pass in that phase confirmed a hard-coded hostname
rendered; it did not confirm the hostname was right.

**The replacement** points at the real command and carries the real host and port from
`/api/meta`, so it is copy-pasteable rather than illustrative:

```
claude-bus init --bus ws://<host>:<port>/ws
```

with a body explaining that the agent appears once a Claude Code session launches in
that directory — the mechanism, not just the instruction.

## The files tab

**One endpoint: `GET /api/rooms/{name}/files`**, a thin DTO over the existing
`Store::list_files`. `FileRow` already carries key, size, content type, uploader and
`updated_at`.

**A new DTO, not the existing `FileInfo`.** That type exists and looks like a fit, but
it belongs to `src/proto.rs` — it is a websocket protocol type, and its wire form is
snake_case (`content_type`, `updated_by`) because that is what the protocol speaks.
Every REST DTO in this app is camelCase. Reusing it would introduce one snake_case
shape into an otherwise camelCase API, which is the drift `src/web/api.rs`'s module
header exists to prevent.

**A tab, not a route.** The handoff is explicit: files exist only in the context of a
room. It is `mainView: 'transcript' | 'files'` as component state, which is also what
the handoff's own State list specifies. This is a deliberate exception to the
console's otherwise URL-addressable navigation, and the design's reason — files are
not a place, they are a facet of a room — is good enough to follow.

**Loaded eagerly when the room opens**, not on tab click. The count lives in the tab
label (`files · 3`), and the handoff's stated purpose for that is so an empty room's
files tab announces itself as empty *without being opened*. A lazy fetch cannot do
that.

**`files · N` returns to the room header.** 2c omitted it deliberately — there was no
endpoint to back the count, and rendering `files · 0` against an unknown number would
have been the UI stating something it could not know. That omission was recorded as
arriving "with the files screen, alongside its endpoint". This is that.

**The list is read-only.** The handoff's table has no download affordance, and no HTTP
route serves file bytes at all. Adding one means a new endpoint, a content-disposition
decision, and deciding how to serve arbitrary agent-uploaded bytes from an
unauthenticated origin. That is its own piece of work, not a rider on this one.

## The empty states

**New-bus**, shown at the index route when `rail.agents` is empty. It replaces "select
a room or agent", which is unhelpful advice on a bus with nothing to select. Eyebrow,
headline "The bus is running. Nothing has joined it.", body, the corrected command
block, and the status line with its live dot.

**Empty room: one line, "Nothing said here yet."**

The handoff's example reads "Nothing said here yet. Two messages are queued for
members who are offline." The second sentence is dropped, deliberately.

A queued message is a stored message awaiting delivery to an offline member — it is in
the room. A room whose transcript is empty therefore has nothing queued, necessarily.
The two clauses cannot both be true, and implementing the second would be building a
branch that can never render. This is the same call made on the composing indicator in
2c: do not build what structurally cannot fire.

If that reading of the data model is wrong, the cost is one missing sentence and it is
recoverable. The alternative is dead code shaped like a feature.

**The handoff also says the composer is present and usable in an empty room** — "the
useful action in an empty room is to say something." True, and 2f's. In 2e an empty
room states its emptiness and stops, which is honest for a console that cannot send.

**No icon, no dashed box, no call to action** on the empty room. An empty room is
normal, not an error: one line stating what is true, then stop. The new-bus state is
the single exception the handoff carves out, and it earns it.

## Verification, changed deliberately

2d's final review found one Critical and three Important defects that eight per-task
reviews and a full manual browser pass all missed. Every one was on the failure side;
every happy path worked first time. This phase makes failure verification explicit.

**Component tests must cover the files fetch failing**, not only succeeding. A failed
fetch must not render an empty table: "this room has no files" and "we could not find
out" are different facts and must not look the same. That is the same principle the
delete preview already enforces, and the reasoning the HTML confirm page documents.

**Rust tests**: a room with files returns them with correct sizes and uploaders; a room
with none returns an empty list rather than a 404; an unknown room behaves predictably
rather than 500ing.

**The manual pass includes deliberately breaking things** — stopping the bus with the
files tab open, and opening a room that does not exist — and looking at what the
operator actually sees.

**The new-bus state is trivially stageable**: a fresh data directory *is* a bus nothing
has joined. Compare the room state flags, which took three phases to be seen rendered
at all. There is no excuse for shipping this one unverified.

**One check nothing else in this effort has needed:** the command block must be
copy-pasteable and correct. Copy what the screen renders, run it, and confirm it does
what the screen claims. That is precisely the check that would have caught
`claude-bus register` before it reached a spec.

## Deliverable

A room with files shows them in a table with the count in its tab label; a room without
announces that from the label without being opened; a bus with nothing on it tells a
new user what to actually run, with a command that works; and an empty room says so in
one line.

## Out of scope

File download and the endpoint it would require; the composer; light mode;
message-text search; removing the old HTML delete page or files page. Named explicitly
so the implementation plan cannot quietly absorb them.

## Consequences accepted

- Files can be seen but not retrieved from the console. The CLI remains the way to
  fetch one.
- The files tab is the console's one piece of navigation that is not URL-addressable.
- An empty room shows no composer until 2f, so the state the handoff describes as
  "say something" currently offers no way to.
