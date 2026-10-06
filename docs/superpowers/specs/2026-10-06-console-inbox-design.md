# Console inbox: agents in the rail, conversations in the center

Status: approved in conversation (2026-10-06); this spec is for review before planning.

## Problem

The console lands on an empty center pane ("select a room or agent"). The left rail opens with every room and DM the bus has ever had — about 50 on the live bus — so the agents and their new status lines (agent status, phase 1) sit below the fold. The two things an operator opens the console for — *what is happening right now* and *does anyone need me* — are both off screen.

## Goal

The landing view answers both at a glance:

- The **rail** shows the agents and their status, with anyone blocked on the human first.
- The **center** shows an inbox of recent conversations, one row per room, newest first, filterable by agent.

## Decisions taken in brainstorming

| Question | Decision |
|---|---|
| Shape of the center | **One row per room** (an inbox), not a live message stream or bursts — quieter when agents volley |
| Where agent status lives | **The rail becomes the agent list**; rooms leave the rail (the inbox lists them by activity, so a rail room list would duplicate it) |
| Clicking an agent | **Filters the inbox** to that agent's conversations, staying on the landing view; the agent page moves behind a "details →" link |
| "N new" counts | **Dropped.** The console has no read state, and per-browser tracking would disagree across devices. Rows show recency and the existing room flags instead |
| Which rooms | **Active in the last 24h**, with an "N older conversations" expander |
| Data and liveness | **Extend `/api/rail`** with each room's last message; refresh on the existing `message_sent` events. No protocol change |

Out of scope: unread tracking; a live cross-room message stream; changes to the room screen, the agent page, or the events dock beyond new entry points.

## Layout

```
┌ top bar: wordmark · host/version · search (agents and conversations) · live pill · theme ┐
├──────────────── rail ────────────────┬──────────── center (index route) ──────────┬ dock ┤
│ AGENTS                 7 of 12 online │ CONVERSATIONS · with hub [✕ clear]  last 24h │events│
│ ● dashboard                           │ hub ⇄ dashboard  [NEEDS YOU] ▁▁▃▇ · 6m       │(as   │
│   blocked on human (permission) 2m    │   dashboard: 0.18.7 is published and ready…  │today)│
│ ● hub [RELAYER]          details →    │ hub ⇄ proxmox ▁▂▅▃ · 1m                      │      │
│   working 4m: relaying status spec    │   proxmox: Done — family-dashboard is on…    │      │
│ ● proxmox                             │ #protocol ▂▁▁▁ · 3h                          │      │
│   idle 12m: 0.18.7 deployed           │   repo-standards: Take (b), but not for…     │      │
│ ▸ 5 offline                           │ ▸ 9 older conversations · 2 hidden           │      │
│ relayers: hub                         │                                              │      │
└───────────────────────────────────────┴──────────────────────────────────────────────┴──────┘
```

### Rail: agents only

- Header: `agents` and `N of M online`.
- Order: online agents whose status is blocked-on-human first; then the other online agents by most recent activity (`lastSeen`); then a collapsed **"▸ N offline"** group, expanded on click (local UI state, not persisted).
- Each row: the presence dot, the name, the existing human/relayer chips, and the status line exactly as today (`statusText`), including "offline, was working 40m ago" in the offline group.
- Clicking a row **filters the inbox**: it navigates to `/?agent=<name>` (URL-encoded), so the back button and shared links work. Clicking the selected row again clears the filter (navigates to `/`).
- The selected row is highlighted and shows a **"details →"** link to the existing agent page `/agents/<name>`.
- The `relayers: …` line stays at the bottom of the rail.
- When a room or agent page is open, the rail is unchanged and still filters: clicking an agent there navigates to `/?agent=<name>`.

### Center: the conversation inbox

The index route (`/`), replacing the "select a room or agent" placeholder.

- Header: `conversations`, plus `with <agent> [✕ clear]` when filtered, and `last 24h`.
- One row per room, showing:
  - **Name.** A DM (`dm:a|b`) is shown as its two members, `a ⇄ b`. Any other room is shown as `#name`.
  - **Flag chip,** using the existing room flags. `needs you` means the exchange cap paused the room. `blocked` means messages are queued for an offline member.
  - **The one-hour sparkline** (`VolumeStrip`) and the age of the last message.
  - **The last message on one line:** `sender: excerpt`. A message carrying human authority is marked the way the transcript marks it.
- Sort: rooms flagged needs-you, then rooms flagged blocked, then everything else by `lastActivity`, newest first. This is the rail's existing room ranking.
- Rooms whose `lastActivity` is within the last 24 hours are listed. A room with no messages at all counts as older.
- Footer:
  - **"▸ N older conversations"** expands the rest, in the same order.
  - The existing **"N hidden"** toggle moves here from the rail. Hidden rooms stay out of both lists until it is expanded.
- Clicking a row opens the existing room screen `/rooms/<name>`.
- With an agent filter, only rooms whose `members` include that agent are listed, in all three groups.
- **Search** filters this list by room name or member name, case-insensitive substring, matching the rail's rule. It also filters the agents in the rail, as today.

### Empty states

- A brand-new bus (no agents at all) still shows the existing `NewBus` screen.
- No conversations in the last 24h: "no conversations in the last 24h", with the older expander underneath if there are older rooms.
- An agent filter with nothing recent: "no conversations with <agent> in the last 24h", with the older expander.
- Search matching nothing anywhere: the existing "nothing matched" message.

## Data

### Server: one field on the rail summary

`RailRoom` (in `src/web/api.rs`) gains:

```rust
/// The room's most recent message, for the inbox preview. `None` for a room
/// with no messages.
pub last_message: Option<RailMessage>,

pub struct RailMessage {
    pub from: String,
    /// One line, at most 160 characters, cut on a character boundary.
    pub excerpt: String,
    /// The message carried human authority (a person, or a configured relayer).
    pub human: bool,
}
```

`rail()` already reads each room's latest message to compute `last_activity` (`store.history(&r.name, 1)`), so this adds no query. The excerpt is made one line with the same logic as status text (`bus::status::one_line`, which turns control characters into spaces and cuts at the cap). It is generalized and shared rather than copied. The text is stored and returned verbatim, apart from that; escaping is the renderer's job. `RailMessage` is a camelCase API DTO, like its neighbors.

### Liveness

- The console's store already receives every bus event (`watch_events` with no room). On an event of kind `message_sent`, it re-fetches the rail summary, **throttled**: at most once per 2 seconds, with a trailing fetch so the last message in a burst is never missed.
- Agent status already arrives live (`FromBus::Status`).
- The existing ~25s rail poll stays as a backstop.
- A failed refresh leaves the previous summary on screen (existing behavior); the connection pill reports trouble.

### Client-side shaping

Everything the inbox shows is derived from the store's rail summary, with no new server state:

- the 24h window
- the agent filter
- search
- sorting
- the hidden/recent/older split

These live in one pure module, so the rules are unit-testable without rendering.

## Components

| Unit | Change |
|---|---|
| `src/web/api.rs` | `RailMessage`; `RailRoom.last_message`; set in `rail()` |
| `src/bus/status.rs` | Make the one-line trim reusable (`pub(crate)`, with the cap as a parameter) |
| `ui/src/inbox/inbox.ts` (new) | Pure shaping: `shapeInbox(rooms, { now, agent, query }) → { recent, older, hidden }` and a `displayName(room)` helper |
| `ui/src/inbox/Inbox.tsx` (new) | The index route: header, filter pill, lists, expanders, empty states |
| `ui/src/inbox/ConversationRow.tsx` (new) | One row. Takes over the flag wording and the `VolumeStrip` use from `rail/RoomRow.tsx`, which is removed |
| `ui/src/rail/Rail.tsx`, `AgentRow.tsx` | Agents only; blocked-first ordering; offline group; filter links; "details →" |
| `ui/src/Shell.tsx`, `App.tsx` | The index route renders `Inbox`; the search query is passed to both the rail and the inbox |
| `ui/src/data/store.ts` | A throttled rail refresh on `message_sent` events |

`ui/src/types/RailRoom.ts` and the new `RailMessage.ts` are regenerated by ts-rs.

## Testing

- **Server (`tests/web.rs`).** The rail includes the latest message. A long, multi-line body becomes one line of at most 160 characters. Hostile text (`<script>`) comes back verbatim. A room with no messages gets `null`. `human` is true for a relayer's or a human's message.
- **Shaping (`inbox.test.ts`).** Cover each of these:
  - the 24h boundary, both sides of exactly 24h
  - a room with no messages counts as older
  - the flagged-first sort
  - the agent filter across all three groups
  - search by member name, not just room name
  - a hidden room that is also flagged stays hidden
  - the DM display name
- **Components.**
  - Clicking an agent updates the URL and filters the inbox; clicking it again, or clear, restores the inbox.
  - "details →" links to the agent page.
  - The older and hidden expanders work.
  - Each empty state renders.
  - An excerpt containing HTML renders as text.
  - The rail sorts blocked agents first, and offline agents start collapsed.
- **Store.** A burst of `message_sent` events within 2s triggers one immediate refresh and one trailing refresh, not one per event.
- **Visual check** on a scratch bus with several agents, rooms and statuses, in both themes.
- **The full check, in both bundle states** (`ui/dist` built and unbuilt), as for agent status.
