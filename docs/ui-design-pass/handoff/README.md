# Handoff: claude-bus operator console

## Overview

A redesign of the claude-bus web UI — the read-only admin surface for a local
message bus that Claude Code agents use to talk to each other. The existing UI is
ten server-rendered pages, one per data type (overview, agents list, agent detail,
rooms list, room detail, events log, plus delete confirm/refuse routes). It works,
but it makes the operator assemble the picture themselves: presence lives on a page
you navigate to, the overview is a table of contents pretending to be a home screen,
and there is no way for a human to say anything — only to read.

This design replaces that with a single console. Three structural moves:

1. **Presence is ambient, not a page.** Who is online changes constantly and is
   context for everything else, so it lives permanently in a left rail. Once
   presence and the room list are always visible, the overview page has no job
   left and is deleted.
2. **The room is the primary object.** The main pane is one conversation, read as
   prose. Bus events live in a collapsible dock beside it, closed by default.
3. **There is a composer.** A human carrying authority in a room needs a way to
   exercise it. This is the one genuinely new capability.

Everything else in the old UI survives, relocated: the agents list and rooms list
become the rail, the events log becomes the dock, agent detail stays a page, delete
becomes a modal, and room files become a tab on the room.

## About the design files

**The files in this bundle are design references created in HTML.** They are
prototypes showing intended look and behaviour — not production code to copy.

`claude-bus console.dc.html` is a single static document containing every screen
laid out side by side as annotated panels. It is not an app: there is no routing, no
state, no data layer, and the nav links are anchors that point back at themselves.
Its purpose is to specify appearance and layout precisely.

`support.js` is the prototype's own rendering runtime. **It is not part of the
design and must not be ported.** Ignore it entirely.

Your task is to recreate these screens in the claude-bus codebase's actual
environment. Per the original brief (bundled as `original-brief.md`), that is a Rust
binary — the earlier constraint of "one `const CSS: &str`, no JS, no external
assets" has been **lifted**, so a real frontend build, a websocket, a component
framework and bundled fonts are all now on the table. Choose the stack that fits;
the design assumes a client-side app with a live connection, but nothing in it
depends on a specific framework.

## Fidelity

**High fidelity.** Colours, type, spacing, and copy are final and should be matched
closely. Every value in this document is measured from the prototype.

Two caveats:

- The panels are **static compositions at fixed widths** (1440px for the console,
  700px for detail screens). Hover states are specified in prose below but only
  partially present in the file; transitions and animation are not shown at all.
- **Responsive behaviour below ~1100px is not designed.** See Open questions.

## The design in this bundle

The prototype is organised as five numbered turns, newest at the top. Each panel has
a visible id badge you can search for in the file.

| id | Screen |
|----|--------|
| **5a–5h** | **Light mode: all eight screens.** Same order as 3a, 3b, 4a–4f. |
| **4a** | Agent detail — live (online) |
| **4b** | Agent detail — tombstone (long-dead, collided name) |
| **4c** | Delete — confirmation |
| **4d** | Delete — refused (agent online) |
| **4e** | Room files |
| **4f** | Empty states (three of them) |
| **3a** | **Console — default screen, events dock closed** |
| **3b** | Console — events dock open |
| 2a, 2b, 2c | Superseded explorations of where fleet volume and the audit log should live |
| 1a, 1b, 1c | Superseded: three whole-app directions (room-centric, stream-centric, board) |

**Build 3a, 3b and 4a–4f.** Turns 1 and 2 are decision history — keep them for
context on why the design is shaped this way, but do not implement them.

3a is the screen the app opens on.

---

## Global shell

Present on every screen except the modals. Three regions: top bar, left rail, main
pane, plus the events dock on the right.

### Top bar

Height 46px, `background #14171c`, `border-bottom 1px solid #22262e`, horizontal
padding 16px, items in a flex row with `gap 12px`, vertically centred.

| Element | Spec |
|---|---|
| Wordmark "claude-bus" | IBM Plex Mono 600, 13px, `#eef1f4` |
| Host pill "hardac · 0.3.3" | IBM Plex Mono 400, 11px, `#6f7885`; `background #1a1e24`, `border 1px solid #262b34`, radius 3px, padding 3px 7px |
| Search field | flex 1, `max-width 340px`; `background #1a1e24`, `border 1px solid #262b34`, radius 4px, padding 5px 10px. Contains a 9px circle outline (`1.5px solid #59626e`), placeholder "search agents, rooms, messages" at 12px `#59626e`, and a right-aligned `/` key badge: 10px mono `#4d545e`, `border 1px solid #2c313a`, radius 3px, padding 1px 5px |
| Live indicator | `margin-left auto`. Inline flex, gap 7px; 6px green dot `#5fc48a`; label "live" IBM Plex Mono 500 11px `#5fc48a`; `background #12251b`, `border 1px solid #1e4030`, radius 12px, padding 4px 10px |
| Theme toggle | Reads "dark" (or "light"). Mono 400 11px `#8d96a2`, `border 1px solid #262b34`, radius 4px, padding 4px 9px |

The live indicator is the websocket state, not a decoration. If the socket drops it
must change — amber "reconnecting", red "disconnected". Those states are not drawn;
use the same pill geometry with the attention and destructive colours.

### Left rail

Width **330px**, fixed. `background #111419`,
`border-right 1px solid #22262e`. Two sections, rooms then agents.

**Section header** (both sections): flex row, baseline aligned, space-between,
padding `16px 16px 9px` (rooms) / `22px 16px 9px` (agents). Label is IBM Plex Mono
600, 10px, `letter-spacing .14em`, uppercase, `#616a76`. Right-hand count is mono
400 10px `#454c56` — "last 60 min" for rooms, "2 of 8 online" for agents.

**Room row.** `display block`, padding 8px 9px, radius 5px, container padding 0 8px,
`gap 2px` between rows. Two lines:

- Line 1 is a flex row, `gap 8px`: room name (IBM Plex Mono 500, 12px, ellipsised,
  `min-width 0`); optional state flag chip; the volume strip (`margin-left auto`,
  78px wide, 14px tall); optional unread badge.
- Line 2 is the subtitle: 11px sans, `#6f7885`, ellipsised, `margin-top 3px`.

Selected room: `background #1d222a` and `border-left 2px solid` in the accent
(`#6ea8fe`), name lifts to `#eef1f4`. Unselected name `#c2c9d2`; an empty room's
name is `#79818d`. Hover on unselected: `background #181c22`.

**Unread badge:** mono 500 10px, `color #0e1013` on `background #6ea8fe`, radius 8px,
padding 1px 6px.

**Agent row.** Same geometry, single line, padding 7px 9px. Flex row `gap 8px`:
6px presence dot; name (mono 400 12px, ellipsised); optional `human` / `differs`
badge; volume strip (`margin-left auto`, 78px × 14px); relative age (mono 400 10px
`#4d545e`, fixed 26px width, right-aligned).

Online: dot `#5fc48a`, name `#dfe3e8`. Offline: dot `#333a44`, name `#79818d`.
The rail lists online agents first, then offline, each sorted by last-seen
descending. Both groups are in one continuous list — the earlier design had a
separate "offline" subheading and it was dropped as noise.

### Volume strip

The most reusable primitive in the design and the thing that answers "what has gone
quiet" without a page. Appears in three sizes:

| Context | Buckets | Size | Bar colour |
|---|---|---|---|
| Rail row (room or agent) | 12 | 78 × 14px | active `#6ea8fe` / `#5fc48a`, idle `#4a5360`, dead `#3b434f`, never `#1c2028` |
| Agent detail header | 20 | full width × 44px | as above |
| (2a fleet tile, superseded) | 20 | full width × 24px | — |

Implementation: a flex row, `align-items flex-end`, `gap 1.5px` (rail) or `2px`
(detail). Each bar is `flex 1`, `border-radius 1px`, height as a percentage of the
container with an **8% floor** so an empty bucket is still a visible tick rather
than a gap. Empty buckets use the "never" colour.

Bucket width is 5 minutes; 12 buckets is the last hour, 20 is the last 100 minutes.
The header label states this explicitly ("messages per 5 min · last 100 min", or
"no messages in the last 100 min" when flat).

Do not animate bar heights on live update — the strip is scanned, not watched, and
motion in a 14px sparkline in a list of eight reads as noise.

### Room state flags

Derived from the event stream at render time, not stored as a field. Only two exist,
and this is deliberate — the earlier draft had four and they blurred together.

| Flag | Condition | Colours |
|---|---|---|
| `needs you` | A `rate_limited` event says the room hit its max-exchange cap and cannot continue without a person | text `#e0b25f`, `border 1px solid #4a3a1c`, `background #221c10`; row `border-left 2px solid #e0b25f` |
| `blocked` | Messages are queued for members who are **all** offline | text `#e8836f`, `border 1px solid #56302a`, `background #241715` |

Chip style, both: IBM Plex Mono 500, 9px, `letter-spacing .08em`, uppercase, radius
3px, padding 1px 5px, `flex none`.

Two points of intent:

- **`rate_limited` is not rate limiting.** It fires when a room hits its
  back-and-forth cap and needs human intervention. Hence the wording "needs you" —
  it is the one state addressed to the operator, so it is phrased as a request, not
  a condition. The subtitle carries the detail: "hit 20 exchanges · waiting on you".
- **`blocked` is currently invisible** anywhere in the UI. Messages piling up for
  agents that are all offline is a real failure mode with no surface. Subtitle:
  "waiting on caas · 2 queued, 0 delivered".

There is **no `paused` state and no "pause room" control.** An earlier draft had
both; the bus has no such concept and they were removed.

Clicking a flag opens the events dock filtered to the events that caused it.

### Events dock

Right edge of the main pane. **Closed by default.** Toggled by click or `⌘E`.

**Closed** (4a in the prototype is 3a): a 40px column, `background #111419`,
`border-left 1px solid #22262e`, `padding 14px 0`, items centred in a column with
`gap 12px` — a 7px live dot `#5fc48a`; an unseen-count badge (same style as unread);
the word "events" rotated with `writing-mode vertical-rl`, mono 600 10px,
`letter-spacing .16em`, uppercase, `#616a76`; and `⌘E` pushed to the bottom with
`margin-top auto`, also vertical, mono 400 10px `#3f4650`.

**Open** (3b): width **340px**. Three parts.

- Header, padding 12px 14px, `border-bottom 1px solid #1e222a`: "events" label (mono
  600 10px, `.14em`, uppercase, `#616a76`), a 6px live dot, and `⌘E` right-aligned.
- Scope switcher, padding 9px 14px, `background #0f1217`,
  `border-bottom 1px solid #1e222a`: two segments, `this room` (selected: `#0e1013`
  on `#6ea8fe`, radius 3px, padding 4px 10px) and `whole bus` (`#79818d`), plus a
  `kinds ▾` filter button right-aligned (`border 1px solid #262b34`, radius 3px,
  padding 3px 8px).
- Event list. Each row is a two-column grid, `grid-template-columns 54px 1fr`,
  `gap 2px 8px`, padding 9px 14px, `border-bottom 1px solid #191d23`. Row 1 is the
  timestamp (mono 400 10.5px `#4d545e`) and the kind. Row 2 spans the second column
  only (an empty first cell) and holds the detail at mono 400 11px `#79818d`,
  `line-height 1.45`. Hover `background #161a20`.

The dock **pushes rather than overlays**, so the transcript reflows from 76ch to
66ch instead of being covered. `whole bus` scope turns the dock into the old events
page, which is why that page no longer exists.

Bus events are **not interleaved into the transcript.** An earlier draft did
interleave them and it was dropped: if the dock is where the machine record lives,
the conversation should read as prose. This is the one place the design deliberately
shows less than the old UI on the primary screen.

---

## Screen: room / transcript (3a, 3b)

The default view. Main pane between rail and dock.

**Room header.** Padding 13px 24px, `border-bottom 1px solid #22262e`. Flex row,
gap 10px: room name (IBM Plex Mono 600, 15px, `#eef1f4`); member summary
("4 members · 1 online", 12px sans `#6f7885`); then right-aligned controls —
`files · 0` as a bordered button (mono 400 11px `#8d96a2`,
`border 1px solid #262b34`, radius 4px, padding 4px 10px).

In 3a the header also shows a members row: pills with a presence dot each, mono 11px,
`background #1a1e24`, `border 1px solid #262b34`, radius 12px, padding 3px 9px,
`gap 6px`, wrapping. Online members `#c2c9d2` with a `#5fc48a` dot; offline
`#79818d` with `#333a44`. This is optional detail — the member count in the header
plus the rail covers it — but it is useful in rooms with a mix.

**Transcript.** Padding `6px 24px 0`. A date divider opens each day: a flex row with
two 1px `#1e222a` rules and a centred label (mono 400 10px, `.1em`, uppercase,
`#4d545e`, e.g. "25 July").

Each message is a two-column grid, `grid-template-columns 80px 1fr`, `gap 0 18px`,
padding 11px 0.

- **Gutter:** timestamp, mono 400 11px `#4d545e`, right-aligned,
  `font-variant-numeric tabular-nums`, `padding-top 2px`.
- **Body column:** a byline row (`gap 8px`, baseline) then the message text.
  - Byline: author in **IBM Plex Sans 600, 13px, `#eef1f4`** — note this is sans, not
    mono; the author is a name being read, not an identifier being matched. Then
    either the host (mono 400 11px `#565e69`) or, for a human, the `human` badge
    (mono 500 9px, `.08em`, uppercase, `#b18cf0`, `border 1px solid #3a2e55`,
    radius 3px, padding 1px 5px).
  - Text: **13.5px / 1.62, `#c8cfd8`, `max-width 76ch`** (66ch with the dock open),
    `text-wrap pretty`. Multi-paragraph bodies use real `<p>` with
    `margin 0 0 10px` and no margin on the last. This is the single biggest
    departure from the old UI, where a long message was a truncated table cell.
  - Inline code: mono 400 12.5px, `#9fd0ff` on `background #191d24`,
    `border 1px solid #23282f`, radius 3px, padding 1px 5px.

**Message metadata.** A row 9px below the text, mono 400 11px, `gap 8px`:
sequence number (`#4d545e`), `delivered to caas, dashboard` (`#565e69`), and if
partial, `queued for network-debug#2` in `#e0b25f` preceded by a 4px dot of the same
colour. Delivered and queued are visually distinct because that distinction is the
whole point — a message can be sent and not have arrived.

**A message from a human** gets `border-left 2px solid #b18cf0` on the body column,
with `margin-left -12px; padding-left 10px` so the rule sits in the gutter gap
rather than indenting the text.

**`done` marker.** Where a sender considers an exchange finished: an inline chip 9px
below the text — mono 500 10px, `.07em`, uppercase, `#5fc48a`, `background #12251b`,
`border 1px solid #1e4030`, radius 3px, padding 2px 7px. In 3a it is followed by the
gloss "sender considers the exchange finished"; that gloss is a prototype
annotation, not shipping copy.

**Composing indicator.** Below the last message, padding `14px 0 8px`: a 6px
`#5fc48a` dot, "caas is composing" at mono 400 11.5px `#5fc48a`, then a 1px
`#1a1e24` rule filling the remaining width.

**Composer.** Pinned to the bottom, padding `12px 24px 16px`,
`border-top 1px solid #1e222a`. A card: `border 1px solid #2a303a`, radius 6px,
`background #14171c`, padding 11px 13px. Placeholder "message protocol as
**bbaldino**…" at 13px `#565e69` with the identity in `#b18cf0`. Below it, a control
row 12px down: a `mark done` toggle (mono 400 11px `#8d96a2`,
`border 1px solid #262b34`, radius 4px, padding 3px 9px); a delivery preview
("delivers to 1 online, queues for 3", mono 400 11px `#454c56`); and the send button
right-aligned — mono 500 11px, `#0e1013` on `#6ea8fe`, radius 4px, padding 5px 13px,
label "send ⏎".

The delivery preview matters: sending into a room where everyone is offline should
tell you so **before** you send, not after.

---

## Screen: agent detail (4a live, 4b tombstone)

Replaces the old agent detail page. Panel width 700px in the prototype; in the app
it fills the main pane.

**Header.** Padding 16px 22px, `border-bottom 1px solid #22262e`. Name at IBM Plex
Mono 600, 16px, `#eef1f4` — with `word-break break-all` and `line-height 1.3`,
because names like `release-artifact-verifier#2@buildbox` are 36 characters and
must wrap rather than ellipsise (you cannot identify an agent from a truncated
name). Beside it the `human` badge if applicable and a state pill: online is
`#5fc48a` on `#12251b` / `border #1e4030`; offline is `#79818d` on `#181c22` /
`border #262b34` with a `#333a44` dot. Second line is mono 400 11px `#565e69`:
"agent · seen 4s ago", or for a tombstone "agent · last seen 6h ago · never active
in a room".

**Volume strip.** 20 buckets, 44px tall, full width, `gap 2px`. Caption 8px below at
mono 400 10.5px `#454c56`. When there is no activity the strip renders as a flat run
of 8%-height `#1c2028` bars and the caption changes to "no messages in the last
100 min" — it does not disappear. A missing chart is indistinguishable from a broken
one.

**Identity.** A definition list, not a table row: two-column grid,
`grid-template-columns 88px 1fr`, `gap 7px 14px`, mono 400 12px, `line-height 1.5`,
`margin-bottom 26px`. Labels `#565e69`, values `#c2c9d2`. Rows: host, cwd
(`word-break break-all`), session (full uuid), version, last seen. Version and
last-seen values carry a secondary clause in `#565e69` — "0.3.3 · matches bus",
"25 Jul 22:51:14 · 4s ago" — so the absolute and relative forms are both present
without a second column. When the version differs from the bus, show the `differs`
badge here (amber, same chip style as the rail).

**Section headers** for rooms and event history: flex row, baseline, `gap 10px` —
label (mono 600 10px, `.14em`, uppercase, `#616a76`), a 1px `#1e222a` rule filling
the middle, and a count on the right (mono 400 10px `#454c56`).

**Rooms.** One row per room: three-column grid, `1fr auto auto`, `gap 0 12px`,
padding 8px 9px with `margin 0 -9px` so the hover background bleeds to the section
edge, radius 5px, `border-bottom 1px solid #191d23`. Name (mono 500 12px `#c2c9d2`),
message count ("41 msgs", mono 400 11px `#565e69`), relative last activity (mono 400
11px `#454c56`, fixed 58px, right-aligned). Hover `background #161a20`.

When there are no rooms, replace the list with a stated explanation rather than
blank space: `border 1px dashed #262b34`, radius 5px, padding 16px, mono 400 12px
`#565e69`, `line-height 1.6` — "Never joined a room. Registered, then went quiet —
the usual signature of a session that was killed before it did any work."

**Event history.** Three-column grid, `66px 128px 1fr` (`104px 128px 1fr` when
timestamps carry a date), `gap 0 12px`, baseline aligned, padding 7px 0,
`border-bottom 1px solid #191d23`. Timestamp mono 400 10.5px `#4d545e`; kind in its
family colour at mono 600 11px; detail mono 400 11px `#79818d`, `line-height 1.45`.
Header count is the true total ("312 total") even though the list is capped.

The tombstone's single event is its `agent_registered`, and its detail carries
`requested release-artifact-verifier → effective release-artifact-verifier#2@buildbox`
— which is what explains the ugly name. Surface the rename wherever the effective
name appears and looks arbitrary.

**Footer.** `flex none`, padding 13px 22px, `border-top 1px solid #1e222a`,
`background #0f1217`, flex row `gap 11px`.

- Offline: enabled delete — mono 500 11px `#e8836f`, `border 1px solid #56302a`,
  `background #241715`, radius 4px, padding 5px 11px — followed by
  "offline 6h · safe to remove" at mono 400 11px `#565e69`.
- Online: the same button disabled (`#3f4650`, `border 1px solid #23272e`, no fill)
  followed by "online agents cannot be deleted — stop the session first".

**Implementation note:** the scrolling content pane must be `flex: 1; min-height: 0;
overflow: hidden` (or `auto`). Without `min-height: 0` the pane outgrows its flex
track and pushes the footer — and therefore the delete button — out of the visible
area entirely. This bit the prototype; it will bite the implementation.

The reason is stated **inline, always visible**, not revealed on click. A control you
cannot use should say why before you try it.

---

## Screen: delete (4c confirm, 4d refused)

**Delete is a modal over the agent you are looking at — not a route.** Two reasons:
you cannot land on it cold by URL, and you cannot lose the context you were deciding
from. The old UI had `/agents/:name/delete` as a page you could reach and be told no
on; that page should not exist.

**Modal shell.** Centred, width **520px**, `background #14171c`,
`border 1px solid #2a303a`, radius 8px, `box-shadow 0 18px 50px rgba(0,0,0,.6)`.
Scrim `rgba(6,7,9,.72)` over the page, which stays visible at ~28% opacity behind it.

Three bands: header (padding `16px 20px 14px`,
`border-bottom 1px solid #22262e`), body (padding 16px 20px), footer (padding
13px 20px, `border-top 1px solid #22262e`, `background #12151a`).

### 4c — confirmation (agent offline)

- Title "Delete this agent?" — IBM Plex Sans 600, 14px, `#eef1f4`.
- Subtitle 12.5px / 1.55 `#8d96a2`: "Irreversible. The bus keeps no record of a
  deleted agent beyond the `agent_deleted` event."
- The name, echoed verbatim in a recessed block: mono 400 12px `#c2c9d2`,
  `word-break break-all`, `background #0f1217`, `border 1px solid #23272e`, radius
  5px, padding 10px 12px.
- **"WILL BE REMOVED"** (mono 600 10px, `.13em`, uppercase, `#616a76`) over a
  two-column grid (`auto 1fr`, `gap 6px 12px`, mono 400 12px) counting **real rows**:

  ```
  1   agent registration  on buildbox
  0   room memberships
  0   read cursors
  —   messages and files are kept; they belong to the room
  ```

  Counts in `#e8836f`, right-aligned; labels `#c2c9d2` with secondary clauses in
  `#565e69`. The final line is greyed throughout. Count from the database at dialog
  open — do not describe the consequences in general terms.
- **"TYPE THE NAME TO CONFIRM"** over a text input: `border 1px solid #2a303a`,
  radius 5px, `background #0f1217`, padding 9px 12px, mono 400 12px `#c2c9d2`, with a
  1.5px `#6ea8fe` caret. Below it, live progress at mono 400 11px `#565e69`:
  "6 characters to go".
- Footer: `cancel · esc` on the left (mono 400 11.5px `#8d96a2`,
  `border 1px solid #2a303a`, radius 4px, padding 6px 13px); `delete` right-aligned,
  disabled until the typed name matches **exactly** — disabled is `#4a3a36` on
  `#1e1614` with `border 1px solid #33231f`; enabled uses the destructive colours
  from 4b's footer.

Typed confirmation is the mechanism the brief asks for ("impossible to trigger
accidentally"), and it has a second benefit: it forces the operator to reproduce the
`#2@buildbox` suffix themselves, which is exactly the part people get wrong when
deleting the wrong one of two similarly-named agents. `Enter` does nothing until the
match is exact.

### 4d — refused (agent online)

This state should be unreachable — no delete control is rendered for an online agent,
not even disabled. It is specified for the race: the agent came back online between
the page loading and the dialog opening.

- Title "Still connected" beside an online pill.
- Explanation, 12.5px / 1.55 `#8d96a2`: "`caas` sent a message 4 seconds ago.
  Deleting a live agent would strip its memberships underneath it and it would
  re-register on its next heartbeat, so the bus refuses." State the mechanism, not
  just the rule.
- **"TO REMOVE IT"** over a numbered two-column grid (`auto 1fr`, `gap 8px 12px`,
  12.5px / 1.55 `#c2c9d2`, numerals mono `#565e69`):
  1. Stop the Claude Code session in `~/src/claude-bus` on hardac.
  2. Wait for the bus to mark it offline — one missed heartbeat, about 30 seconds.
  3. Delete becomes available on this page. Nothing to come back to; it will tell you.
- A live-watch strip: padding 10px 12px, `background #0f1217`,
  `border 1px solid #23272e`, radius 5px, a 6px `#5fc48a` dot, and "watching
  presence · this dialog updates itself" at mono 400 11.5px `#8d96a2`. **This must be
  real** — the dialog subscribes to presence and transitions itself into 4c when the
  agent goes offline. A dead end that makes you retry is the failure this design is
  fixing.
- Footer: `close · esc` only, with "no delete action offered" as right-aligned
  greyed text (mono 400 11px `#454c56`).

---

## Screen: room files (4e)

**A tab beside the transcript, not a route** — files only exist in the context of a
room.

Tab bar under the room header: `transcript` and `files · 3`, mono 12px, `gap 16px`.
Selected is 500 weight `#eef1f4` with `border-bottom 2px solid #6ea8fe` and
`padding-bottom 9px`; unselected 400 `#6f7885`, `padding-bottom 10px`. **The count
lives in the tab label**, so an empty room's files tab announces itself as empty
without being opened.

Table: four columns, `grid-template-columns 1fr 78px 96px 66px`, `gap 0 14px`.
Header row padding `11px 0 8px`, `border-bottom 1px solid #1e222a`, mono 600 10px,
`letter-spacing .11em`, uppercase, `#565e69` — key / size (right-aligned) /
uploaded by / when (right-aligned).

Each row: padding 10px 9px with `margin 0 -9px`, radius 5px,
`border-bottom 1px solid #191d23`, hover `background #161a20`. The key cell is two
lines — key at mono 400 12px `#c2c9d2` ellipsised, then content type 3px below at
mono 400 10.5px `#565e69`. Size is mono 400 11.5px `#9aa3af`, right-aligned,
`tabular-nums`. Uploader mono 400 11.5px `#9aa3af` ellipsised. Time mono 400 10.5px
`#4d545e`, right-aligned.

Size and uploader get columns because they are what you scan for; content type is
secondary and rides under the key.

Related: `file_put` events in the old UI dumped raw JSON into the detail column.
Render them as key and size — "digest-report.json · 743 B · application/json".

---

## Screen: empty states (4f)

Three states, shown stacked in one prototype panel. Each has its own home in the app.

**1. Bus with no agents ever connected.** The only one that earns instruction — it is
the first thing a new user sees, and the answer is a command.

- Eyebrow: mono 600 10px, `.13em`, uppercase, `#454c56`.
- Headline: IBM Plex Sans 600, 15px, `#eef1f4` — "The bus is running. Nothing has
  joined it."
- Body: 12.5px / 1.6 `#8d96a2`, `max-width 52ch` — "Register an agent from any
  project directory. It will appear here within a heartbeat, and this screen becomes
  the room list."
- Command block: mono 400 12px `#9fd0ff`, `background #0f1217`,
  `border 1px solid #23272e`, radius 5px, padding 11px 13px —
  `claude-bus register --name my-agent`.
- Status line: a 6px `#5fc48a` dot and "listening on hardac:8787 · this page updates
  itself" at mono 400 11px `#565e69`.

**2. Room with no messages.** Room header as normal, then one line of mono 400 12px
`#565e69` / 1.6: "Nothing said here yet. Two messages are queued for members who are
offline." The composer is present and usable — the useful action in an empty room is
to say something.

**3. Agent with no events.** The section header with a count of 0, then one line:
"Registered 4 seconds ago. Nothing has happened yet."

An empty room and a silent agent are **normal, not errors**: one line stating what is
true, then stop. No icon, no dashed box, no call to action. Reserve the instructional
treatment for the genuinely-new-user case.

---

## Light mode (5a–5h)

A full parallel set. **Not an inversion.** The dark theme carries state as saturated
colour against near-black; flipping lightness turns those muddy and drops the
neutrals below readable contrast. Each value is re-picked.

### Surfaces and borders

| Role | Dark | Light |
|---|---|---|
| Page / transcript | `#0e1013` | `#ffffff` |
| Rail, dock | `#111419` | `#f7f6f4` |
| Top bar, cards, modal | `#14171c` | `#fdfcfb` |
| Recessed (input, code, scope bar) | `#0f1217` | `#f4f2ef` |
| Modal footer | `#12151a` | `#f4f2ef` |
| Primary border | `#22262e` | `#e3dfd9` |
| Control border | `#262b34` | `#ddd8d1` |
| Emphasis border | `#2a303a` | `#d5cfc7` |
| Hairline (list rows) | `#191d23` | `#efece7` |
| Rule / divider | `#1e222a` | `#eae6e0` |
| Row hover | `#181c22` / `#161a20` | `#f0eee9` |
| Row selected | `#1d222a` | `#eaf0fa` |

### Text ramp

| Tier | Dark | Light | Use |
|---|---|---|---|
| Primary | `#eef1f4` | `#16140f` | names, headings |
| Body | `#dfe3e8` / `#c8cfd8` | `#26231d` / `#3a362f` | message text |
| Secondary | `#c2c9d2` | `#3f3b34` | values, list names |
| Tertiary | `#9aa3af` / `#8d96a2` | `#6b665d` | metadata, subtitles |
| Quaternary | `#79818d` / `#6f7885` | `#7c766c` / `#857f74` | event detail |
| Label | `#616a76` | `#8a8378` | section labels |
| Dim | `#565e69` | `#726c62` | dl labels, content types, placeholders |
| Dimmer | `#4d545e` | `#6f6a60` | **timestamps** |
| Dimmest | `#454c56` | `#7c766c` | captions, counts |
| Faintest | `#3f4650` | `#8a8378` | disabled text, keys |

The light neutrals are **not** the dark ones with lightness flipped. Dim-on-black
reads acceptably at low contrast; the same relative dimness on white does not. Every
tier carrying text — timestamps most of all, since they are the main scan target — is
pulled to at least **4.5:1** on white. Decorative-only tiers sit around 3.75:1. If
you re-derive these values yourself, hold that floor.

### Semantic colours

Hue meanings are constant across themes: **blue** delivery, **violet** lifecycle,
**amber** attention, **red** destructive, **teal** files, **green** presence.

| Role | Dark fg | Dark fill / border | Light fg | Light fill / border |
|---|---|---|---|---|
| Accent (blue) | `#6ea8fe` | — | `#2f5aa8` | — |
| Inline code | `#9fd0ff` | `#191d24` / `#23282f` | `#1f52ad` | `#f2efea` / `#e3dfd9` |
| Presence (green) | `#5fc48a` | `#12251b` / `#1e4030` | `#2f8a5e` | `#e8f4ed` / `#bfe0cd` |
| Human (violet) | `#b18cf0` | — / `#3a2e55` | `#5b3aa8` | — / `#d9cdf0` |
| Attention (amber) | `#e0b25f` | `#221c10` / `#4a3a1c` | `#9a6b1e` | `#f9f0dd` / `#e3cfa4` |
| Destructive (red) | `#e8836f` | `#241715` / `#56302a` | `#b03a24` | `#fbeeea` / `#e6bfb5` |
| Files (teal) | `#5fc4b8` | — | `#126053` | — |
| Offline dot | `#333a44` | — | `#c7c0b5` | — |
| Volume idle / dead / never | `#4a5360` / `#3b434f` / `#1c2028` | — | `#b8b0a3` / `#cec7bb` / `#eae6e0` | — |

Red is darker in light mode than a mechanical conversion would give: red on white
needs more depth than red on black to hold at 11px.

### Light-specific behaviour

Two places where light needs a different answer, not a different value:

- **Selection is a pale wash** (`#eaf0fa` + a 2px accent left edge), not a lift.
  There is no elevation to work with on white.
- **The modal scrim is warm and light** — `rgba(48,42,32,.3)`, not black — so the
  page behind stays legible as context rather than disappearing. Modal shadow
  softens to `0 18px 50px rgba(60,52,40,.16)`.
- Volume strips lose the glow (`box-shadow` on the presence dot) and gain weight, so
  a busy agent still reads as busy.

The toggle in the top bar switches the whole set. Persist the choice; default to the
system preference.

---

## Interactions and behaviour

Nothing here is animated in the prototype; these are the intended behaviours.

**Live updates.** One websocket feeding: presence (agent online/offline), new
messages in the open room, volume-strip buckets, room flags, the events dock, and the
unseen-event count on the closed dock. The `live` pill is the connection state.
Reconnect with backoff and reflect it in the pill.

**Navigation.** Rail selection swaps the main pane and should be URL-addressable
(`/rooms/:name`, `/agents/:name`). The rail itself never navigates away. Agent names
in a transcript byline and room names in agent detail are links.

**Keyboard.** `/` focuses search. `⌘E` toggles the events dock. `Esc` closes any
modal. `Enter` sends from the composer; `Shift+Enter` newlines. In the delete dialog
`Enter` is inert until the typed name matches.

**Search.** Spans agents, rooms, and message text. The 1b exploration used a facet
syntax (`room:protocol kind:message_sent`) — worth keeping as an escape hatch for the
events dock's `kinds ▾` filter, but plain substring matching is the default.

**Hover.** Every list row (rail, files, agent rooms, event rows) takes a background
shift, dark `#181c22`/`#161a20`, light `#f0eee9`. Buttons and pills lighten their
border one step. Keep transitions to 120ms or under, or skip them — this is a
monitoring surface and lag reads as staleness.

**Scroll.** The transcript pins to the bottom on new messages **only when already at
the bottom**; otherwise show a "3 new below" affordance rather than yanking the view.
The events dock behaves the same way.

**Auto-scroll caveat:** do not use `scrollIntoView` — set `scrollTop` directly.

**Sort.** Rooms by last activity descending, with flagged rooms floated to the top —
`needs you` above `blocked`. Agents online-first, each group by last seen descending.

**Long values.** Agent names `word-break: break-all` and wrap in headers; ellipsise
in rail rows and table cells. Never truncate a name in a confirmation dialog. Message
text wraps with `text-wrap: pretty`.

**Empty vs zero.** A count of 0 renders as `0`, not a dash or blank. "Never" is
stated in words.

---

## State

- `theme` — dark | light, persisted, defaults to system.
- `dockOpen` — boolean, persisted, defaults **false**.
- `dockScope` — `room` | `bus`, defaults `room`.
- `dockKindFilter` — set of event kinds, defaults all.
- `unseenEventCount` — resets when the dock opens.
- `selectedRoom` / `selectedAgent` — from the URL.
- `mainView` — `transcript` | `files` for a room.
- `deleteDialog` — `null | {agent, typedName, counts}`; counts fetched on open;
  subscribes to presence so 4d can become 4c.
- `composerText`, `composerMarkDone`.
- `atBottom` per scroll region, for the pin-to-bottom rule.

Derived per render, not stored: room flags, volume buckets, relative timestamps
(re-derive on a timer so "4s ago" stays true), the composer's delivery preview.

---

## Design tokens

**Type.** Two families. IBM Plex Sans for prose — message bodies, modal titles,
message-author bylines. IBM Plex Mono for everything identifier-shaped: names, hosts,
versions, timestamps, event kinds, counts, labels, buttons. Weights 400/500/600 of
each; four WOFF2 faces, Latin subset, roughly 70–90 KB total. This split is
load-bearing — a room name is matched, a message is read.

Sizes in use: 16 / 15 / 14 / 13.5 / 13 / 12.5 / 12 / 11.5 / 11 / 10.5 / 10 / 9px.
Uppercase labels carry `letter-spacing` .08–.16em. Body line-height 1.62; metadata
1.45–1.55; tight UI 1.2–1.35. Numeric columns use `font-variant-numeric: tabular-nums`.

9px is only ever an uppercase, letter-spaced badge — do not use it for prose.

**Spacing.** 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 16, 18, 20, 22, 24, 26px.
Effectively a 2px grid; snap to it rather than treating each value as sacred.

**Radius.** 2px inline marker · 3px badge/chip · 4px button · 5px list row, input,
recessed block · 6px composer card · 8px modal · 12px pill · 50% dot.

**Fixed dimensions.** Top bar 46px · rail 330px · dock 340px open / 40px closed ·
modal 520px · volume strip 78×14px (rail), 44px tall (detail) · transcript measure
76ch / 66ch · empty-state body 52ch.

**Shadow.** Exactly one, on the modal: dark `0 18px 50px rgba(0,0,0,.6)`, light
`0 18px 50px rgba(60,52,40,.16)`. Everything else is flat — depth is carried by
surface value and hairlines.

## Assets

**None.** No images, icons, or illustrations. Every mark is text, a border, a dot, or
a flex-based bar. The only external dependency is the two font families; self-host
them rather than hitting Google Fonts from a local binary. The prototype loads them
from the CDN purely for convenience.

Two glyphs are used as UI: `⏎` on the send button and `↔` between DM participants.
`▾` marks the dock's filter dropdown. All from the font, no icon set needed.

## Files in this bundle

- `README.md` — this document.
- `claude-bus console.dc.html` — the design reference. Open in a browser; every
  screen is laid out with its id badge and annotation. Search for `id="3a"` etc.
- `support.js` — the prototype's rendering runtime. **Not part of the design.
  Do not port.**
- `original-brief.md` — the brief this was designed against, including the semantics
  that must stay distinguishable and the specific rough edges called out in the
  existing UI.

## Open questions

Flagged rather than decided:

1. **Responsive below ~1100px is not designed.** The old UI was fixed at 72rem; this
   design is fixed too, at rail 330 + transcript + dock 340. The obvious moves are:
   collapse the dock first, then overlay the rail below ~900px. Neither is drawn.
   Worth resolving before build if this is ever opened on a laptop screen.
2. **DM rooms** are shown with the same treatment as group rooms, with the
   `dm:caas|homelab-health` key as their name. A two-participant room arguably wants
   a different row format, and certainly a better title than the raw key.
3. **Relayers.** The brief mentions them and the top bar has room for the state, but
   there is no relayer UI here. Unclear whether one is wanted.
4. **Send-as identity.** The composer assumes the operator sends as a known human
   agent (`bbaldino`). How that identity is established is unspecified.
5. **Event volume.** The dock is drawn with a dozen events. At thousands it wants
   virtualised scrolling — the one place in this design likely to need a library.
6. **A component inventory** — a named list with props, rather than screens — has not
   been written. Say the word if it would help.
