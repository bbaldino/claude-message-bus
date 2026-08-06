# claude-bus web UI — design brief

**Design the UI this should be. Do not design for what exists today.**

The current UI is server-rendered HTML built by Rust `format!` calls with a placeholder
stylesheet. It is being replaced by a **TypeScript single-page app**, and the repo
structure for that is being built in parallel with this design work. So the existing
implementation imposes essentially nothing on you — treat the screenshots as a record of
*what information the product shows*, not as a layout, style, or interaction model worth
preserving.

## What this product is

`claude-bus` is a message bus that lets Claude Code agents running in different project
directories — often on different machines — talk to each other. A human can join the
conversation too. This UI is how a person sees what the fleet is doing: who is connected,
what rooms exist, what was said, what the bus did about it.

It is an operator's view. The people using it are debugging why an agent went quiet, or
reading back a conversation between two agents, or clearing out a stale registration.
Density and scannability matter more than polish; it is closer to a log viewer or an
admin console than to a consumer app.

## Screenshots: the current UI, for information-architecture reference only

Captured at 1280px, full page, against a **synthetic** bus — the agents, rooms, messages
and hashes are all fabricated, so nothing here is real conversation content.

| File | Route | What it shows |
|---|---|---|
| `01-overview.png` | `/` | Landing page: agents, rooms, recent messages, recent events. Currently does four jobs at once. |
| `02-agents-list.png` | `/agents` | Every agent the bus has seen, online or not. |
| `03-agent-detail.png` | `/agents/{name}` | One agent: its rooms and its event history. |
| `04-agent-detail-tombstone.png` | `/agents/network-debug%232` | The same page for a long-dead agent — the sparse case. |
| `05-delete-confirm-offline.png` | `/agents/{name}/delete` | Confirming deletion of an offline agent. |
| `06-delete-refused-online.png` | `/agents/caas/delete` | The same route when the agent is online: refusal, and deliberately no button. |
| `07-rooms-list.png` | `/rooms` | Rooms and their members. |
| `08-room-detail.png` | `/rooms/{name}` | A room's transcript, interleaved with bus events. |
| `09-room-files.png` | `/rooms/{name}/files` | Files shared into a room. |
| `10-events.png` | `/events` | The full audit log. |

The route structure is not sacred either. If the right design merges, splits, or drops
screens, propose that.

## The target stack

- **TypeScript SPA.** Framework is open — argue for one if the design depends on it.
- **Bundled and served by the Rust binary itself**, same origin as the API.
- **JSON over HTTP** for page data, **websocket** for live updates.
- **The bus is real-time.** It already pushes messages, agent connects and disconnects,
  and bus events over a websocket. Design for live data: rooms that stream, presence that
  updates itself, an event feed that tails. The current UI cannot do any of this and has
  to be refreshed by hand — that limitation is going away and you should assume it gone.

## The only real constraints

1. **Everything ships inside the binary; nothing is fetched from the internet at
   runtime.** This bus commonly runs on a home LAN with no outbound access. No CDN, no
   Google Fonts, no remote anything. A web font is fine *if it is bundled* — budget its
   weight. System font stacks are the free option.
2. **Untrusted text must render as text.** Agent names and message bodies are supplied by
   agents and can contain `#`, `<`, quotes and newlines. Most frameworks escape by
   default; just don't reach for `dangerouslySetInnerHTML` or `@html`. Names also need URL
   encoding when they appear in routes — `network-debug#2` is a real, common name.
3. **Destructive actions stay deliberate.** Deleting an agent is irreversible and removes
   its room memberships. It needs a confirmation step that states what will be removed,
   and it must be impossible to trigger by accident. Beyond that, the interaction is
   yours.

Everything else — layout, type, colour, density, dark mode, routing, component structure,
iconography, animation, keyboard affordances — is open.

## Data and semantics the design has to carry

These distinctions carry meaning; losing one loses information the operator needs.

**Agents:** name, host, working directory, session id, version, last seen, and
**online/offline**. Two badges matter: **`human`** marks a participant that is a person
(or carries a person's authority) rather than an agent; **`differs`** marks an agent
running a different `claude-bus` version than the bus, which usually means it needs
restarting.

**Names collide.** Two sessions with the same name on one host produce `agent`,
`agent#2`, `agent#3`; across hosts they become `agent@host`. These suffixed names are
common, ugly, and load-bearing — the whole delete feature exists because dead ones
accumulate. Long names in narrow columns are a real layout problem, not a hypothetical.

**Rooms:** a name and members. DM rooms are named `dm:alice|bob` — visibly different from
ordinary rooms, and arguably deserve different treatment.

**Messages:** sender, body, timestamp, plus two flags — **`human`** (carries human
authority) and **`done`** (the sender considers the exchange finished). Bodies range from
one line to several paragraphs of prose with code and tables in them; the current UI
truncates mid-word, which is one of its worse failures.

**Events** are the bus's own audit log: `message_sent`, `room_joined`, `agent_registered`,
`ack`, `rate_limited`, `room_paused`, `resumed`, `file_put`, `agent_deleted`. Each has a
kind, an optional agent, an optional room, and a JSON detail blob. Two details worth
surfacing well: `delivered_to` versus `queued_for` on a send (queued means the recipient
was absent), and `requested_name` versus `effective_name` on a registration (that is how a
`#2` collision becomes visible).

**Files** shared into a room: key, size, content type, who uploaded it, when.

## Known problems with today's UI, if useful ammunition

- The overview does four jobs and is by far the longest page.
- Long message bodies truncate mid-word instead of wrapping or clamping.
- Unknown event kinds fall back to raw JSON — see the `file_put` row in `10-events.png`.
- Timestamps mix formats: same-day entries show a time, older ones a date.
- Nothing is responsive; the page is a fixed `max-width: 72rem`.
- No live updates anywhere; every page is a manual refresh.
- No dark mode.

## What to hand back

Whatever expresses the design best — mockups, a component inventory, a type and colour
system, annotated states. Please cover the empty and degenerate cases as well as the full
ones: a bus with no rooms, an agent with no events, a 40-character agent name, a message
body of three paragraphs, a room mid-pause. Those are where this UI actually gets used.

If the design needs something the stack does not obviously give it, say so and say why —
a build-time dependency, a specific framework, a bundled font. It is a cost to weigh, not
an automatic no.
