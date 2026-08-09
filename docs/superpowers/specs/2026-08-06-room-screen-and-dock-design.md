# Room screen and events dock (phase 2c)

## Context

Phase 2b built the console's frame: design tokens, bundled fonts, routing, the left
rail and the top bar. Selecting a room writes the URL and the main pane names it —
against a placeholder that was always meant to be temporary.

This phase fills it. The transcript is the densest screen in the design handoff
(`docs/ui-design-pass/handoff/README.md`, sections 3a and 3b) and the reason the
console exists: a place where a long agent message reads as prose rather than as a
truncated table cell.

### The decomposition, as it now stands

| | Piece | Status |
|---|---|---|
| 1 | Frontend infrastructure | done |
| 2a | Data layer | done |
| 2b | Shell — tokens, fonts, routing, rail, top bar, `Unwatch` | done |
| **2c** | **Room screen and events dock. This spec.** | |
| 2d | Detail screens (agent detail, delete, files) | |
| 2e | Composer | |
| 2f | Light mode | |

**2c is read-only.** The handoff pins a composer to the bottom of this screen, but it
stays in 2e. The transcript's job is reading, and it is independently useful without
sending — the console becomes worth opening at the end of this phase rather than the
one after.

## What the backend already provides

No Rust changes this phase — a first for this effort, and stated explicitly so nobody
goes looking for backend work.

- `GET /api/rooms/{name}/messages` already accepts `before` for backward paging
  (`src/web/api.rs`, `TranscriptQuery`). 2a built it; only the TypeScript client
  omitted the parameter.
- `GET /api/events` already filters by `room` and `kind`.
- `message_sent` events already carry `{msg_id, delivered_to, queued_for, done}`
  (`src/bus/commands.rs`), which is what makes delivery correlation possible.

## The gap this phase closes first

`selectRoom` clears the message list and watches the room, but **never fetches
history**. Nothing populates a transcript today. Room selection will now also load
the room's recent messages and its events.

## Decisions

### Two event lists, not one

The store keeps a single global `events` array capped at 500, fed by a `watch_events`
subscription with `room: null`. Three consumers now want it: the dock at scope `this
room`, the dock at scope `whole bus`, and the transcript's delivery correlation.

Deriving all three from that one array is simplest and has a silent failure: the cap
is shared across every room, so a busy fleet evicts the `message_sent` events the open
room needs. Delivery lines would stop appearing on older messages, intermittently and
invisibly, and the UI would imply a message had not been delivered when it had.

So: `events` stays as the global live feed for `whole bus`. A new `roomEvents` is
fetched on room selection and appended to by live pushes whose `room` matches. The
dock's `this room` scope and the correlation map both read it. Scope switching needs
no fetch because both lists are already in memory, and the open room's correlation
data cannot be evicted by unrelated traffic.

Rejected alternatives: one list with client-side filtering (the eviction bug above);
refetching on every scope switch (a round-trip on a segmented control, which the
handoff warns reads as staleness, and the live feed still needs merging).

### Correlation is derived, not stored

A `Map<msg_id, {deliveredTo, queuedFor}>` computed from `roomEvents` where
`kind === 'message_sent'`. `Event.detail` is `unknown` on the generated type, so it is
narrowed at that boundary — the same discipline the existing push handlers use.

**Accepted consequence:** paging back beyond the room-events window leaves messages
with no delivery line. Absent, not wrong. The mitigation would be a `before` parameter
on the events endpoint, which does not exist and is not worth building on speculation.

### Scrollback is lazy upward paging

Pages of 100, fetched as the reader scrolls up rather than eagerly. A short page means
the beginning has been reached. Scroll position is restored in the same frame as the
prepend — content added above the viewport moves it otherwise, which is the fiddly
part of this feature and the reason it gets an explicit test.

### Message bodies get a small, owned markdown subset

The handoff specifies inline-code styling, which implies parsing. Agent prose really
does contain markdown: this project's own bus traffic routinely carries fenced code
blocks, backticked identifiers, bullet lists and bold.

**In:** paragraphs, fenced code blocks, bullet and numbered lists, inline code,
`**bold**`.

**Out:** links, italics, tables, images, blockquotes, raw HTML. Anything unmatched
falls through as literal text, so an unsupported construct renders exactly as a
no-parsing implementation would have shown it. Nothing is swallowed.

**No library.** That subset is around a hundred lines with tests. `react-markdown` is
the standard answer and is safe by default, but it pulls the remark ecosystem — on the
order of the whole font payload this effort just spent work trimming — for constructs
we have deliberately excluded.

**Security, stated precisely because it was initially overstated in discussion:** the
rule that matters is no `dangerouslySetInnerHTML`, anywhere. React escapes text nodes
by construction, so with every fragment emitted as a React element there is nothing to
inject, regardless of what any sender writes. Markdown rendering is dangerous only
through three specific doors — raw-HTML passthrough, `dangerouslySetInnerHTML`, and
`javascript:` URLs in links — and this subset opens none of them. Parsing markdown is
therefore a scope decision, not a safety one.

Links are a deliberate omission rather than an oversight: a bare URL reads fine as
text, and auto-linking is where a URL-scheme check becomes load-bearing.

### The `files · 0` button is omitted

The handoff puts it in the room header. There is an HTML `/rooms/{name}/files` page
but no JSON endpoint, so the SPA cannot know the count, and rendering `files · 0` when
the real number might be five is the failure this effort has now avoided three times
(the composing indicator, the search placeholder, the `/` key badge). It arrives in 2d
with the files screen, which is where its endpoint belongs.

## Components

**Room header** — name in mono 600 15px, then a member summary composed client-side:
`RailRoom.members` gives the roster and the rail's agents give presence, so "4 members
· 1 online" needs no new data. Member pills are included; the handoff calls them
optional, but a room's mix of online and offline members is precisely what this
console exists to show.

**Transcript** — date dividers derived from `createdAt`; messages as an 80px/1fr grid
with a right-aligned tabular timestamp in the gutter. The byline's author is **IBM
Plex Sans 600, not mono** — the handoff's deliberate inversion, because an author is a
name being read rather than an identifier being matched. A human's message takes a
violet left rule pulled into the gutter gap by a negative margin so the text does not
indent. Body text is 13.5px/1.62, `max-width` 76ch, 66ch with the dock open.

**Message metadata** — sequence number, `delivered to …`, and if partial `queued for
…` in amber with a leading dot. Delivered and queued are visually distinct because
that distinction is the point: a message can be sent and not have arrived.

**`done` chip** — the green marker where a sender considers an exchange finished. The
handoff's trailing gloss ("sender considers the exchange finished") is a prototype
annotation and is not shipped.

**Events dock** — 40px closed and closed by default, with a live dot, unseen-count
badge, "EVENTS" rotated via `writing-mode: vertical-rl`, and the toggle hint pinned to
the bottom. 340px open, with a header, a scope switcher and the event list. It
**pushes rather than overlays**, so the transcript's `max-width` is a function of dock
state rather than a constant. `dockOpen` persists and defaults to false.

**The toggle chord is platform-correct, which the handoff is not.** The handoff
specifies `⌘E` throughout. This bus runs on Linux, where there is no Command key and a
`⌘` glyph in the label is simply wrong. Bind `Ctrl+E` on non-Mac platforms and `⌘E` on
Mac, detected once from the platform, and render the label to match what the reader
can actually press. The same rule applies to any later chord — `Esc` and `Enter` need
no such treatment, but anything modifier-based does.

Both bindings must ignore the keystroke while focus is in a text input, for the same
reason the `/` search shortcut does: otherwise the composer in 2e cannot type the
letter.

The `kinds ▾` filter derives its options from the loaded events rather than a
hardcoded list, so a new event kind on the bus appears without a frontend change.
`unseenEventCount` resets when the dock opens and counts against the current scope, so
the badge agrees with what opening it will show.

**The composing indicator stays cut**, as decided in 2b: the bus has no typing
concept, so it could only ever light for a human in the web UI and never for the
agents being watched.

## Cleanup, taken first

Three items carried forward from 2b, done before the transcript exists rather than
migrated afterwards:

- **CSS Modules across the console.** Every class is currently global — `.row`,
  `.dot`, `.flag`, `.search`. The transcript, the dock and later the composer all want
  those words. Vite supports `*.module.css` with no configuration.
- **Hoist `useTicker` and `age()` out of `rail/`.** The transcript's relative
  timestamps and the dock need the same once-per-second re-derivation.
- **Extract one chip rule.** `.flag` and `.badge-human` already duplicate it; the
  `done` chip and event kinds would make four copies.

## Testing

Component tests cover the parser (one per construct, plus the fallthrough case —
that unmatched syntax renders as literal text is the property that makes shipping an
incomplete subset safe), the correlation map, date dividers, scope switching, and the
unseen-count reset.

**One honest gap: jsdom has no layout.** `scrollHeight` and `clientHeight` are zero
there, so pin-to-bottom cannot be meaningfully tested in it. Rather than write tests
that pass vacuously, the scroll decision is extracted into a pure function — given
`scrollTop`, `scrollHeight`, `clientHeight` and whether new messages arrived, return
pin / show-affordance / do-nothing — and tested exhaustively. The DOM wiring around it
stays thin and is covered by a manual pass.

**A manual pass against a real bus** is a named step in the plan, as in 2b: a room
with real traffic renders with correct bylines and delivery lines; the dock toggle
chord works and its label shows the chord this platform actually uses; the transcript
reflows rather than being covered; scrolling up loads older messages without the view
jumping; a message arriving while scrolled up shows the affordance instead of yanking
the view.

2b's manual pass could not stage a room state flag, so `needs you` and `blocked` chips
remain unit-tested only. If this phase's seeding manages to produce one, confirm it —
but do not treat it as a gate, since the same obstacle applies: the chat CLI connects
as human, which resets the exchange-cap counter.

## Deliverable

Selecting a room renders its transcript — bylines, delivery lines, date dividers,
`done` chips, human rules — with the events dock toggling on `⌘E` and switching scope
without a fetch. Scrolling up loads older messages. 2b's main-pane placeholder is
deleted.

## Out of scope

The composer; the files button and screen; agent detail; delete; light mode; markdown
links; event-log paging; message-text search. Named explicitly so the implementation
plan cannot quietly absorb them.

## Consequences accepted

- The console still cannot send. It becomes genuinely useful for reading, which is
  most of what a monitoring surface is for, but sending waits for 2e.
- Messages older than the room-events window show no delivery line.
- The markdown subset is deliberately incomplete; unsupported constructs render as
  literal text.
- Pin-to-bottom and scroll anchoring have no automated gate beyond the pure decision
  function. The manual pass is the only check on the wiring.
