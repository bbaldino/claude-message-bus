# Console shell (phase 2b)

## Context

The redesign handoff (`docs/ui-design-pass/handoff/`) replaces ten server-rendered
pages with a single console. Phase 2a built its data layer: volume aggregates, room
state flags, four JSON endpoints, two opt-in websocket subscriptions, and a typed
client with an observable store. Nothing of the design is rendered yet — `/app`
currently dumps the rail summary as raw JSON.

This spec covers the **global shell**: the top bar and the left rail, plus the
foundations every later screen builds on.

### The decomposition, as it now stands

| | Piece | Status |
|---|---|---|
| 1 | Frontend infrastructure | done |
| 2a | Data layer | done |
| **2b** | **Shell — top bar, rail, tokens, fonts, routing, `Unwatch`. This spec.** | |
| 2c | Room screen and events dock (3a, 3b) | |
| 2d | Detail screens (4a–4f) | |
| 2e | Composer | |
| 2f | Light mode (5a–5h) | |

2b was split out of what was originally one "shell + room screen" phase. The room
screen's message rendering is the most detail-dense part of the whole handoff, and
the shell is independently demonstrable without it.

## Gaps between the design and the bus, resolved here

Four things the handoff assumes that the system does not currently provide. Three
are settled by this spec; the fourth belongs to 2c.

**The composing indicator is dropped.** The handoff shows "caas is composing" below
the last message. The bus has no typing or composing concept — no event, no protocol
variant — and agents are Claude Code sessions that would never emit one. Building it
would light the indicator only for a human typing in the web UI, never for the agents
the operator is actually watching. An indicator structurally incapable of showing the
thing it names is worse than its absence.

**Search is scoped to rooms and agents.** The top bar's placeholder reads "search
agents, rooms, messages", but there is no search endpoint; rooms and agents filter
client-side from the rail summary, message text has no data path. 2b filters what it
can and the placeholder says so rather than promising messages.

**`ToBus::Unwatch` is added here.** `Registry::watch` only ever inserts, so selecting
rooms A then B leaves an observer subscribed to both. 2a added a client-side filter
that discards pushes for unselected rooms, which hides the symptom while the
subscription set keeps growing — and undercuts the opt-in narrowing that justified
the subscription design. The rail drives selection, so the fix belongs with it.

**Per-message delivery metadata** (`delivered to caas · queued for network-debug#2`)
is 2c's problem, resolved as client-side correlation: `message_sent` events carry
`{msg_id, delivered_to, queued_for}`, and 2a's `kind` filter makes
`/api/events?room=X&kind=message_sent` a dense correlation stream. Recorded here
because the decision was made during this phase's design. It is also strictly better
than putting the fields on the message DTO: a live-pushed `FromBus::Message` cannot
carry delivery info — the bus fans the message out and *then* records who it reached —
so under a DTO approach a just-arrived message would show nothing until a refetch,
while the correlating event arrives moments later over the same socket.

Its one caveat, for 2c: deep scrollback leaves the correlated window and those
messages show no delivery line. Mitigations if it matters — raise the limit, or add
`before` to the events endpoint.

## Decisions

### Theming: custom properties from the start

The handoff's palette becomes CSS custom properties on `:root` in `ui/src/theme.css`.
Every component references `var(--…)`; no component contains a hex value. Light mode
(2f) then adds one `[data-theme='light']` block and changes no component.

Names follow the handoff's own vocabulary — it already speaks in surfaces, a text
ramp and semantic colours — so `--surface-rail`, `--text-primary`, `--accent`,
`--flag-needs-you`. The accepted cost is one indirection when cross-checking a
component against the prototype.

Rejected: hardcoding the hex values now and extracting variables in 2f. It is more
faithful while building, but makes 2f a sweep of every component written in 2b and
2c rather than an additive palette.

### Fonts: self-hosted, never the CDN

IBM Plex Sans and IBM Plex Mono, weights 400/500/600, Latin subset — four WOFF2
files under `ui/public/fonts/`, referenced from `@font-face`. Roughly 70–90 KB, which
is larger than everything else this phase adds.

The prototype loads them from Google Fonts for convenience and the handoff explicitly
says not to copy that. The bus commonly runs on a LAN with no outbound access, so a
CDN font does not degrade — it fails.

The split is load-bearing rather than decorative: mono for anything identifier-shaped
(names, hosts, timestamps, counts, event kinds), sans for prose. A room name is
matched; a message is read.

### Routing: React Router, based at `/app`

Two routes — `/app/rooms/:name` and `/app/agents/:name` — with the rail rendered
outside the outlet, which is what "the rail itself never navigates away" means
structurally.

`wouter` would do this in ~2 KB against React Router's ~20 KB and was considered;
React was chosen for ecosystem reach and the router is where that reasoning applies
most directly. 20 KB is noise beside the fonts.

The basename must match Vite's `base` of `/app/`. The SPA is served there while the
original UI still holds `/`, and a mismatch breaks deep links in exactly the way
phase 1's catch-all route test was written to catch.

### Rendered but inert

- **The theme toggle.** It occupies space in the top bar's layout; omitting it changes
  the bar's geometry. It reads "dark" and does nothing until 2f.
- **The search field.** Filters rooms and agents; does not claim messages.

## Components

Four, each with one responsibility.

**`Rail`** — the two sections with their headers and counts ("last 60 min" for rooms,
"2 of 8 online" for agents), and the sort rules: rooms by last activity descending
with flagged rooms floated to the top (`needs you` above `blocked`); agents
online-first, each group by last seen descending, in one continuous list. An earlier
design draft had a separate "offline" subheading and it was dropped as noise.

**`RoomRow`** and **`AgentRow`** — deliberately separate rather than one
parameterised component. They share a geometry but diverge in enough details — unread
badge, state-flag chip, two-line subtitle versus one-line relative age — that a shared
component would be mostly branches.

**`VolumeStrip`** — the shared primitive, taking buckets and a size. 2b needs the
78×14 twelve-bucket rail variant; the 44px twenty-bucket detail variant arrives with
the agent screen and is a prop, not a rewrite. Bars are `flex: 1` with an 8% height
floor so an empty bucket is a visible tick rather than a gap.

**Subtitles are composed in the client**, which is what 2a's "data, not sentences"
decision exists to allow. `/api/rail` ships
`{ kind: "blocked", queued: 2, waitingOn: ["caas"] }` and the row renders "waiting on
caas · 2 queued, 0 delivered". `delivered` is rendered as a literal `0` because
`blocked` is defined as every member being offline, making it necessarily zero — the
server does not send a constant.

## Live behaviour

The rail re-fetches every ~25s and applies presence pushes immediately; both are
already built in 2a's store. Components subscribe to the store — nothing fetches on
its own, which is the property that stops two views disagreeing about what is current.

**The `live` pill is the websocket state, not decoration** — green `live`, amber
`reconnecting`, red `disconnected`. 2a made the red state genuinely reachable when the
reconnect backoff saturates, so all three can occur.

**Volume strips do not animate on update.** The handoff is explicit: the strip is
scanned rather than watched, and motion in a 14px sparkline across eight rows reads as
noise. Stated here because live data invites a transition by default.

## Testing

**vitest against components**, with a fake store: rows render from a seeded rail
summary; a presence push flips a dot; flagged rooms sort above unflagged and
`needs you` above `blocked`; an empty room's name takes the dimmed colour; a
40-character agent name ellipsises rather than breaking the row; the top bar reflects
each of the three connection states. The last two are the handoff's own stated edge
cases.

**Rust integration tests for `Unwatch`**: a watching observer stops receiving a room's
messages after unwatching, and unwatching one room does not disturb another the same
observer still watches. The second is the shape of bug an `Unwatch` implementation
actually produces.

**One manual pass against a real bus**, as an explicit step in the plan: build the
frontend, rebuild the binary, seed a couple of rooms, then confirm the rail populates,
a state flag renders, a deep link to `/app/rooms/:name` works on a cold load, and the
pill goes amber when the bus is killed and green when it returns. Deep links and
reconnect are both things component tests structurally cannot cover.

### The risk this phase carries that earlier ones did not

This is the first phase whose output is judged by eye against a fixed-fidelity design.
The handoff gives exact pixel and colour values, so "matches the design" is a checkable
claim — but nothing automated checks it, and a reviewer reading a diff of CSS custom
properties cannot tell whether the rail *looks* right. The manual pass is the only gate
on visual fidelity, which is why it is a step in the plan rather than left to whoever
happens to look.

## Deliverable

`/app` serves the console frame: the top bar with a live pill and host, the rail
listing every room and agent with real volume strips and state flags, selection
driving the URL, and a main pane containing a placeholder that names the selected room
or agent. 2a's raw-JSON screen is deleted.

**The placeholder is the honest artifact of a split phase** — not a screen, a labelled
hole that 2c fills. It should not be polished.

## Out of scope

The room screen and transcript; the events dock; the composer; light mode;
message-text search; the agent-detail volume strip variant. Named explicitly so the
implementation plan cannot quietly absorb them.

## Consequences accepted

- The console is not usable for reading conversations until 2c. The shell is
  demonstrable, not useful.
- Every colour is indirected through a custom property, so cross-checking a component
  against the prototype takes one lookup.
- The theme toggle and the search field are visible and partly inert, which a first-time
  viewer may read as broken. The alternative — omitting them — changes the top bar's
  specified geometry.
