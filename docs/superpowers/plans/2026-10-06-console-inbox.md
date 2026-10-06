# Console Inbox Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The console's landing view shows agents (with status) in the rail and an inbox of recent conversations in the center, filterable by clicking an agent.

**Architecture:**
- **Server:** one field. `/api/rail` rooms gain `lastMessage` (sender, a one-line excerpt, the human flag), taken from the latest-message read `rail()` already does.
- **Inbox shaping:** a pure module derives the inbox from the store's rail summary: the 24h window, the agent filter, search, the flagged-first sort, and the hidden/recent/older split.
- **Rail:** becomes agents-only. Clicking an agent sets `/?agent=<name>`.
- **Liveness:** the store re-fetches the rail summary on `message_sent` events, throttled.

**Tech Stack:** Rust (axum, ts-rs); React 19 + TypeScript + React Router + Vitest in `ui/`.

**Spec:** `docs/superpowers/specs/2026-10-06-console-inbox-design.md`

## Global Constraints

- Format Rust with `cargo +nightly fmt`. Clippy must be clean under `cargo +stable clippy --all-targets --all-features -- -D warnings`.
- TypeScript uses prettier `{ "singleQuote": true, "semi": false, "printWidth": 100 }` (already `ui/.prettierrc`). Run `npm run format:check`.
- Add Rust dependencies only with `cargo add`; add UI dependencies only with `npm install`. This plan needs none.
- `ts-rs` regenerates `ui/src/types/` during `cargo test`. Commit the generated files. CI fails on `git diff --exit-code -- ui/src/types`.
- **CI's Rust job runs with `ui/dist` unbuilt.** Before pushing, run `find ui/dist -mindepth 1 ! -name .gitkeep -delete && cargo test`, then `make ui` to restore. No test may depend on the bundle.
- The excerpt is **one line, at most 160 characters**, cut on a character boundary, using the same logic as status text.
- The recent window is **24 hours**, inclusive: a room whose last message is exactly 24h old counts as recent.
- The refresh on `message_sent` is throttled to **at most once per 2 seconds**, with a trailing refresh so the last message of a burst is never missed.
- The agent filter is the URL query parameter **`agent`** on the index route: `/?agent=<url-encoded name>`.
- CSS uses only tokens that already exist in `ui/src/theme.css`, in both themes. Never invent a color.
- Message text is rendered as text, never HTML.
- Never run `claude-bus serve` against port 7777. Its `--help` starts a real server. Stop scratch servers by PID, not with `pkill -f`.

## Review Focus

1. **An agent name with `#`, `&`, `?` or spaces** (for example `caas#2`) as the filter. The URL must round-trip it exactly, and the selected-row highlight must match. The test goes in Task 4.
2. **A DM room whose name has more than two members, or no `dm:` prefix.** `displayName` must not throw or produce `undefined`. The test goes in Task 2.
3. **A bus where every room is older than 24h** (the common case after a weekend). The inbox must show the empty line *and* the "N older" expander, never a blank pane. The test goes in Task 3.
4. **Nested links.** The agent row's "details →" must not be an `<a>` inside the row's `<a>` (invalid HTML, and the click target is ambiguous). The test goes in Task 4.
5. **A `message_sent` burst while the rail fetch is slow or failing.** There must be no unbounded fetch pile-up and no exception, and the previous rail must stay on screen. The test goes in Task 5.

---

### Task 1: Server — `lastMessage` on the rail summary

**Files:**
- Modify: `src/bus/status.rs` (make `one_line` reusable: `pub(crate) fn one_line(s: &str, cap: usize) -> String`)
- Modify: `src/web/api.rs` (`RailMessage`, `RailRoom.last_message`, `rail()`)
- Modify: UI test fixtures that build a `RailRoom` literal: add `lastMessage: null`. Run `cd ui && npx tsc --noEmit -p .` to find them; the change is mechanical.
- Test: `tests/web.rs`

**Interfaces:**
- Produces:
  - Rust: `pub struct RailMessage { pub from: String, pub excerpt: String, pub human: bool }`, a camelCase ts-rs export.
  - `RailRoom.last_message: Option<RailMessage>`, which appears in TS as `lastMessage: RailMessage | null`.
  - `pub(crate) const EXCERPT_CAP: usize = 160`, in `api.rs`.

- [ ] **Step 1: Write the failing tests** (append to `tests/web.rs`, near the other `/api/rail` tests)

```rust
#[tokio::test]
async fn the_rail_carries_each_rooms_last_message_as_a_one_line_excerpt() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .append_message("protocol", "caas", "first", false, false)
            .await
            .unwrap();
        let long = format!("line one\nline two {}", "x".repeat(400));
        store
            .append_message("protocol", "dashboard", &long, false, false)
            .await
            .unwrap();
        store
            .append_message("quiet-but-human", "bbaldino", "<script>hi</script>", false, true)
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;
    let rail = common::get_json(port, "/api/rail").await;
    let rooms = rail["rooms"].as_array().unwrap();
    let room = |n: &str| rooms.iter().find(|r| r["name"] == n).unwrap().clone();

    let p = room("protocol");
    assert_eq!(p["lastMessage"]["from"], "dashboard", "{p}");
    let excerpt = p["lastMessage"]["excerpt"].as_str().unwrap();
    assert!(!excerpt.contains('\n'), "one line: {excerpt:?}");
    assert_eq!(excerpt.chars().count(), 160, "capped at 160: {excerpt:?}");
    assert!(excerpt.starts_with("line one line two"), "{excerpt:?}");
    assert_eq!(p["lastMessage"]["human"], false);

    let h = room("quiet-but-human");
    assert_eq!(
        h["lastMessage"]["excerpt"], "<script>hi</script>",
        "verbatim; escaping is the renderer's job"
    );
    assert_eq!(h["lastMessage"]["human"], true);
}

#[tokio::test]
async fn a_room_with_no_messages_has_a_null_last_message() {
    let dir = tempfile::tempdir().unwrap();
    let port = start(dir.path()).await;
    let mut a = common::connect(port, "caas").await;
    common::next_event(&mut a).await;
    common::send(
        &mut a,
        &claude_bus::proto::ToBus::Join {
            req_id: 1,
            room: "empty".into(),
        },
    )
    .await;
    assert!(
        common::wait_until(|| async {
            common::get_json(port, "/api/rail").await["rooms"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "empty")
        })
        .await
    );
    let rail = common::get_json(port, "/api/rail").await;
    let empty = rail["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "empty")
        .unwrap()
        .clone();
    assert!(empty["lastMessage"].is_null(), "{empty}");
}
```

Check `Store::append_message`'s real signature in `src/store/mod.rs` (it is `(room, from, body, done, human)`). If it differs, adapt the call, not the store.

- [ ] **Step 2: Run them and confirm they fail**

Run: `cargo test --test web the_rail_carries a_room_with_no_messages`
Expected: FAIL, because `lastMessage` is null or missing.

- [ ] **Step 3: Implement**

`src/bus/status.rs`: change `fn one_line(s: &str, cap: usize) -> String` to `pub(crate) fn one_line(...)`, and add a doc line saying the rail's message excerpt shares it.

`src/web/api.rs`, beside `RailRoom`:

```rust
/// The excerpt length for the inbox preview. One line, cut on a character
/// boundary by the same rule as status text (`bus::status::one_line`).
pub(crate) const EXCERPT_CAP: usize = 160;

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct RailMessage {
    pub from: String,
    /// One line, at most `EXCERPT_CAP` characters. Verbatim apart from that:
    /// escaping is the renderer's job.
    pub excerpt: String,
    /// The message carried human authority (a person, or a configured relayer).
    pub human: bool,
}
```

Add to `RailRoom`:

```rust
    /// The room's most recent message, for the console inbox. `None` for a room
    /// with no messages.
    pub last_message: Option<RailMessage>,
```

In `rail()`, replace the `last_activity` computation so the one `history(&r.name, 1)` read feeds both fields:

```rust
        let last = app
            .store
            .history(&r.name, 1)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .pop();
        let last_activity = last.as_ref().map(|m| m.created_at);
        let last_message = last.map(|m| RailMessage {
            from: m.from_agent,
            excerpt: crate::bus::status::one_line(&m.body, EXCERPT_CAP),
            human: m.human,
        });
```

and add `last_message,` to the `RailRoom { .. }` literal. The store's message row (`store::MessageRow`) names its sender `from_agent`; the API field stays `from`.

- [ ] **Step 4: Run the tests and fix the fixtures**

Run: `cargo test --test web` and `cargo test --lib`. Both should pass, and `ui/src/types/RailRoom.ts` and `RailMessage.ts` are regenerated.

Then run `cd ui && npx tsc --noEmit -p .` and add `lastMessage: null` to every `RailRoom` literal it flags. Then run `npm test`; all of it should pass.

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt
git add src tests ui/src
git commit -m "feat: the rail summary carries each room's last message"
```

---

### Task 2: Inbox shaping module

**Files:**
- Create: `ui/src/inbox/inbox.ts`
- Create: `ui/src/inbox/inbox.test.ts`

**Interfaces:**
- Consumes: `RailRoom` (with `lastMessage`), from Task 1.
- Produces:

```ts
export const RECENT_WINDOW_MS: number // 24h
export type InboxGroups = { recent: RailRoom[]; older: RailRoom[]; hidden: RailRoom[] }
export function shapeInbox(
  rooms: RailRoom[],
  opts: { now: number; agent: string | null; query: string },
): InboxGroups
export function displayName(room: RailRoom): string
```

- [ ] **Step 1: Write the failing tests** (`ui/src/inbox/inbox.test.ts`)

```ts
import { expect, test } from 'vitest'
import type { RailRoom } from '../types/RailRoom'
import { RECENT_WINDOW_MS, displayName, shapeInbox } from './inbox'

const NOW = 1_800_000_000_000
const room = (over: Partial<RailRoom> & { name: string }): RailRoom => ({
  members: [],
  lastActivity: NOW - 60_000,
  buckets: [],
  flag: null,
  hidden: false,
  lastMessage: null,
  ...over,
})
const names = (rs: RailRoom[]) => rs.map((r) => r.name)
const shape = (rooms: RailRoom[], agent: string | null = null, query = '') =>
  shapeInbox(rooms, { now: NOW, agent, query })

test('the 24h window is inclusive at exactly 24h, and a room with no messages is older', () => {
  const g = shape([
    room({ name: 'edge', lastActivity: NOW - RECENT_WINDOW_MS }),
    room({ name: 'past', lastActivity: NOW - RECENT_WINDOW_MS - 1 }),
    room({ name: 'silent', lastActivity: null }),
  ])
  expect(names(g.recent)).toEqual(['edge'])
  expect(names(g.older)).toEqual(['past', 'silent'])
})

test('flagged rooms sort first, needs-you above blocked, then by last activity', () => {
  const g = shape([
    room({ name: 'plain-new', lastActivity: NOW - 1_000 }),
    room({ name: 'blocked', lastActivity: NOW - 9_000, flag: { kind: 'blocked', queued: 1, waitingOn: ['x'] } }),
    room({ name: 'needs', lastActivity: NOW - 8_000, flag: { kind: 'needsYou', exchanges: 20 } }),
    room({ name: 'plain-old', lastActivity: NOW - 5_000 }),
  ])
  expect(names(g.recent)).toEqual(['needs', 'blocked', 'plain-new', 'plain-old'])
})

test('the agent filter keeps only rooms the agent is a member of, in every group', () => {
  const g = shape(
    [
      room({ name: 'a', members: ['hub', 'x'] }),
      room({ name: 'b', members: ['x'] }),
      room({ name: 'c', members: ['hub'], lastActivity: null }),
      room({ name: 'd', members: ['hub'], hidden: true }),
    ],
    'hub',
  )
  expect(names(g.recent)).toEqual(['a'])
  expect(names(g.older)).toEqual(['c'])
  expect(names(g.hidden)).toEqual(['d'])
})

test('search matches a member name as well as the room name, case-insensitively', () => {
  const g = shape(
    [
      room({ name: 'protocol', members: ['caas'] }),
      room({ name: 'other', members: ['Dashboard'] }),
      room({ name: 'nope', members: ['x'] }),
    ],
    null,
    ' DASH ',
  )
  expect(names(g.recent)).toEqual(['other'])
})

test('a hidden room stays hidden even when flagged', () => {
  const g = shape([room({ name: 'h', hidden: true, flag: { kind: 'needsYou', exchanges: 3 } })])
  expect(names(g.hidden)).toEqual(['h'])
  expect(g.recent).toEqual([])
})

test('display names: DMs by their members, rooms with a #, odd names never break', () => {
  expect(displayName(room({ name: 'dm:hub|proxmox' }))).toBe('hub ⇄ proxmox')
  expect(displayName(room({ name: 'dm:a|b|c' }))).toBe('a ⇄ b ⇄ c')
  expect(displayName(room({ name: 'dm:' }))).toBe('dm:')
  expect(displayName(room({ name: 'protocol' }))).toBe('#protocol')
  expect(displayName(room({ name: 'dm:caas|network-debug#2' }))).toBe('caas ⇄ network-debug#2')
})
```

- [ ] **Step 2: Confirm they fail**

Run: `cd ui && npx vitest run src/inbox/inbox.test.ts`
Expected: FAIL, because the module does not exist yet.

- [ ] **Step 3: Implement** (`ui/src/inbox/inbox.ts`)

```ts
import type { RailRoom } from '../types/RailRoom'

/// Conversations active within this window are "recent"; the rest sit behind
/// the "older" expander. Inclusive at exactly 24h.
export const RECENT_WINDOW_MS = 24 * 60 * 60 * 1000

export type InboxGroups = { recent: RailRoom[]; older: RailRoom[]; hidden: RailRoom[] }

/// The rail's room ranking, kept identical: `needs you` is addressed to the
/// operator, so it outranks `blocked`, which reports a stuck delivery.
function rank(room: RailRoom): number {
  if (room.flag?.kind === 'needsYou') return 0
  if (room.flag?.kind === 'blocked') return 1
  return 2
}

function matchesQuery(room: RailRoom, q: string): boolean {
  if (q === '') return true
  return (
    room.name.toLowerCase().includes(q) || room.members.some((m) => m.toLowerCase().includes(q))
  )
}

export function shapeInbox(
  rooms: RailRoom[],
  { now, agent, query }: { now: number; agent: string | null; query: string },
): InboxGroups {
  const q = query.trim().toLowerCase()
  const sorted = rooms
    .filter((r) => agent === null || r.members.includes(agent))
    .filter((r) => matchesQuery(r, q))
    .sort((a, b) => rank(a) - rank(b) || (b.lastActivity ?? 0) - (a.lastActivity ?? 0))
  const groups: InboxGroups = { recent: [], older: [], hidden: [] }
  for (const r of sorted) {
    if (r.hidden) groups.hidden.push(r)
    else if (r.lastActivity !== null && now - r.lastActivity <= RECENT_WINDOW_MS)
      groups.recent.push(r)
    else groups.older.push(r)
  }
  return groups
}

/// A DM (`dm:a|b`) reads as its members; anything else as a channel. Parsed from
/// the name rather than `members`, because membership changes (a deleted agent)
/// while the room's identity does not. A malformed `dm:` name falls back to the
/// raw name rather than rendering nothing.
export function displayName(room: RailRoom): string {
  if (room.name.startsWith('dm:')) {
    const parts = room.name.slice(3).split('|').filter((p) => p !== '')
    return parts.length > 0 ? parts.join(' ⇄ ') : room.name
  }
  return `#${room.name}`
}
```

- [ ] **Step 4: Run the tests**

Run: `cd ui && npx vitest run src/inbox/inbox.test.ts && npm run typecheck && npm run format:check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ui/src/inbox
git commit -m "feat: inbox shaping for the console landing view"
```

---

### Task 3: The inbox view (index route)

**Files:**
- Create: `ui/src/inbox/Inbox.tsx`, `ui/src/inbox/ConversationRow.tsx`, `ui/src/inbox/Inbox.module.css`, `ui/src/inbox/Inbox.test.tsx`
- Modify: `ui/src/App.tsx` (the index route renders `Inbox`)
- Modify: `ui/src/Shell.tsx` (pass the search query to the outlet: `<Outlet context={{ query }} />`; remove `MainPlaceholder` and move its new-bus logic into `Inbox`)
- Modify: `ui/src/testing/fakeStore.tsx` (`renderWithStore` gains an optional third argument `{ route?: string }`, passed to `MemoryRouter`'s `initialEntries`)
- Modify: `ui/src/Shell.test.tsx` and `App.test.tsx`, wherever they assert `main-placeholder`

**Interfaces:**
- Consumes: `shapeInbox`, `displayName` and `RECENT_WINDOW_MS` (Task 2); `RailRoom.lastMessage` (Task 1).
- Produces:
  - `export function Inbox()`, which reads `useStore().rail`, `useSearchParams().get('agent')`, `useOutletContext<{ query: string }>()`, falling back to `''` when there is no outlet context (tests), and `useTicker(1000)`.
  - `export function ConversationRow({ room, now, dimmed? })`.
  - Test ids: `inbox-row` (one per row, `data-room=<name>`), `inbox-older-toggle`, `inbox-hidden-toggle`, `inbox-empty`, `inbox-filter-clear`.

- [ ] **Step 1: Write the failing tests** (`ui/src/inbox/Inbox.test.tsx`)

```tsx
import { fireEvent, screen } from '@testing-library/react'
import { expect, test } from 'vitest'
import { renderWithStore } from '../testing/fakeStore'
import type { RailRoom } from '../types/RailRoom'
import type { RailSummary } from '../types/RailSummary'
import { Inbox } from './Inbox'

const now = Date.now()
const room = (over: Partial<RailRoom> & { name: string }): RailRoom => ({
  members: [],
  lastActivity: now - 60_000,
  buckets: [0],
  flag: null,
  hidden: false,
  lastMessage: null,
  ...over,
})
const agent = { name: 'hub', host: 'h', version: null, online: true, isHuman: false, isRelayer: false, lastSeen: now, buckets: [0], status: null }
const rail = (rooms: RailRoom[]): RailSummary => ({ rooms, agents: [agent], relayers: [] })
const rowNames = () => screen.queryAllByTestId('inbox-row').map((r) => r.getAttribute('data-room'))

test('rows show the display name and the last message as text', () => {
  renderWithStore(<Inbox />, {
    rail: rail([
      room({
        name: 'dm:hub|proxmox',
        members: ['hub', 'proxmox'],
        lastMessage: { from: 'proxmox', excerpt: '<img src=x onerror=alert(1)>', human: false },
      }),
    ]),
  })
  expect(screen.getByText('hub ⇄ proxmox')).toBeDefined()
  expect(screen.getByText(/proxmox: <img src=x/)).toBeDefined()
  expect(document.querySelector('img')).toBeNull()
})

test('the agent filter comes from the URL and can be cleared', () => {
  renderWithStore(
    <Inbox />,
    { rail: rail([room({ name: 'a', members: ['hub'] }), room({ name: 'b', members: ['x'] })]) },
    { route: '/?agent=hub' },
  )
  expect(rowNames()).toEqual(['a'])
  expect(screen.getByText(/with hub/)).toBeDefined()
  fireEvent.click(screen.getByTestId('inbox-filter-clear'))
  expect(rowNames()).toEqual(['a', 'b'])
})

test('an agent name with URL metacharacters round-trips as the filter', () => {
  renderWithStore(
    <Inbox />,
    { rail: rail([room({ name: 'r', members: ['caas#2 &x?'] }), room({ name: 's', members: ['caas'] })]) },
    { route: `/?agent=${encodeURIComponent('caas#2 &x?')}` },
  )
  expect(rowNames()).toEqual(['r'])
})

test('older conversations sit behind an expander', () => {
  renderWithStore(<Inbox />, {
    rail: rail([room({ name: 'new' }), room({ name: 'old', lastActivity: now - 3 * 86_400_000 })]),
  })
  expect(rowNames()).toEqual(['new'])
  fireEvent.click(screen.getByTestId('inbox-older-toggle'))
  expect(rowNames()).toEqual(['new', 'old'])
})

test('when everything is older than 24h the pane says so and still offers the expander', () => {
  renderWithStore(<Inbox />, {
    rail: rail([room({ name: 'old', lastActivity: now - 3 * 86_400_000 })]),
  })
  expect(screen.getByTestId('inbox-empty').textContent).toMatch(/no conversations in the last 24h/)
  expect(screen.getByTestId('inbox-older-toggle').textContent).toMatch(/1 older/)
})

test('an empty filtered view names the agent', () => {
  renderWithStore(<Inbox />, { rail: rail([room({ name: 'b', members: ['x'] })]) }, { route: '/?agent=hub' })
  expect(screen.getByTestId('inbox-empty').textContent).toMatch(
    /no conversations with hub in the last 24h/,
  )
})

test('hidden rooms sit behind their own toggle', () => {
  renderWithStore(<Inbox />, { rail: rail([room({ name: 'v' }), room({ name: 'h', hidden: true })]) })
  expect(rowNames()).toEqual(['v'])
  fireEvent.click(screen.getByTestId('inbox-hidden-toggle'))
  expect(rowNames()).toContain('h')
})

test('a bus with no agents shows the new-bus screen', () => {
  renderWithStore(<Inbox />, { rail: { rooms: [], agents: [], relayers: [] } })
  expect(screen.queryByTestId('inbox-empty')).toBeNull()
  // NewBus renders its own copy; assert on a stable piece of it.
  expect(document.body.textContent).toMatch(/claude-bus init|no agents/i)
})
```

Before writing the last assertion, read `ui/src/empty/NewBus.tsx` and `ui/src/empty/Empty.test.tsx`, and assert on whatever stable text or test id the existing NewBus tests use.

- [ ] **Step 2: Confirm they fail**

Run: `cd ui && npx vitest run src/inbox/Inbox.test.tsx`
Expected: FAIL, because the component is missing.

- [ ] **Step 3: Implement**

`ui/src/inbox/ConversationRow.tsx`:

```tsx
import { Link } from 'react-router-dom'
import type { RailRoom } from '../types/RailRoom'
import { Chip } from '../ui/Chip'
import { age } from '../ui/time'
import { VolumeStrip } from '../rail/VolumeStrip'
import { displayName } from './inbox'
import styles from './Inbox.module.css'

/// The flag detail line, moved here from the rail's room row. Composed client-side
/// so the copy stays design-owned; `delivered` is a literal 0 because `blocked`
/// means every member is offline.
function flagDetail(room: RailRoom): string | null {
  if (!room.flag) return null
  if (room.flag.kind === 'needsYou') return `hit ${room.flag.exchanges} exchanges · waiting on you`
  return `waiting on ${room.flag.waitingOn.join(', ')} · ${room.flag.queued} queued, 0 delivered`
}

export function ConversationRow({
  room,
  now,
  dimmed = false,
}: {
  room: RailRoom
  now: number
  dimmed?: boolean
}) {
  const detail = flagDetail(room)
  const m = room.lastMessage
  return (
    <Link
      to={`/rooms/${encodeURIComponent(room.name)}`}
      className={`${styles.row} ${room.flag?.kind === 'needsYou' ? styles.flagNeedsYou : ''} ${dimmed ? styles.dimmed : ''}`}
      data-testid="inbox-row"
      data-room={room.name}
    >
      <div className={styles.rowLine}>
        <span className={styles.name}>{displayName(room)}</span>
        {room.flag && (
          <Chip tone={room.flag.kind === 'needsYou' ? 'attention' : 'destructive'}>
            {room.flag.kind === 'needsYou' ? 'needs you' : 'blocked'}
          </Chip>
        )}
        <div className={styles.spacer} />
        <VolumeStrip buckets={room.buckets} variant="rail" />
        {room.lastActivity !== null && (
          <span className={styles.age}>{age(room.lastActivity, now)}</span>
        )}
      </div>
      <div className={styles.preview}>
        {m ? `${m.from}${m.human ? ' (human)' : ''}: ${m.excerpt}` : 'no messages yet'}
      </div>
      {detail && <div className={styles.detail}>{detail}</div>}
    </Link>
  )
}
```

`ui/src/inbox/Inbox.tsx`:

```tsx
import { useState } from 'react'
import { useOutletContext, useSearchParams } from 'react-router-dom'
import { NewBus } from '../empty/NewBus'
import { useTicker } from '../ui/time'
import { useStore } from '../useStore'
import { ConversationRow } from './ConversationRow'
import { shapeInbox } from './inbox'
import styles from './Inbox.module.css'

export function Inbox() {
  const { rail } = useStore()
  const now = useTicker(1000)
  const [params, setParams] = useSearchParams()
  const agent = params.get('agent')
  // Outside the Shell (component tests) there is no outlet context.
  const query = useOutletContext<{ query: string } | undefined>()?.query ?? ''
  const [showOlder, setShowOlder] = useState(false)
  const [showHidden, setShowHidden] = useState(false)

  // Before the rail loads we know nothing; the new-bus screen then would tell a
  // populated bus's owner that nothing has joined it.
  if (!rail) return null
  if (rail.agents.length === 0) return <NewBus />

  const { recent, older, hidden } = shapeInbox(rail.rooms, { now, agent, query })

  return (
    <section className={styles.inbox}>
      <div className={styles.header}>
        <span className={styles.label}>
          conversations
          {agent !== null && (
            <>
              {' '}· with {agent}{' '}
              <button
                className={styles.clear}
                data-testid="inbox-filter-clear"
                onClick={() => setParams({})}
              >
                ✕ clear
              </button>
            </>
          )}
        </span>
        <span className={styles.count}>last 24h</span>
      </div>
      {recent.length === 0 && (
        <p className={styles.empty} data-testid="inbox-empty">
          {query.trim() !== ''
            ? `no conversations match "${query.trim()}"`
            : agent !== null
              ? `no conversations with ${agent} in the last 24h`
              : 'no conversations in the last 24h'}
        </p>
      )}
      {recent.map((r) => (
        <ConversationRow key={r.name} room={r} now={now} />
      ))}
      {older.length > 0 && (
        <button
          className={styles.toggle}
          data-testid="inbox-older-toggle"
          onClick={() => setShowOlder(!showOlder)}
        >
          {showOlder ? '▾' : '▸'} {older.length} older conversation{older.length === 1 ? '' : 's'}
        </button>
      )}
      {showOlder && older.map((r) => <ConversationRow key={r.name} room={r} now={now} />)}
      {hidden.length > 0 && (
        <button
          className={styles.toggle}
          data-testid="inbox-hidden-toggle"
          onClick={() => setShowHidden(!showHidden)}
        >
          {showHidden ? '▾' : '▸'} {hidden.length} hidden
        </button>
      )}
      {showHidden && hidden.map((r) => <ConversationRow key={r.name} room={r} now={now} dimmed />)}
    </section>
  )
}
```

The `inbox-older-toggle` test reads "1 older": the label "▸ 1 older conversation" matches `/1 older/`.

`ui/src/inbox/Inbox.module.css`: model it on `ui/src/rail/Rail.module.css`, reusing the same tokens and sizes for header, label, count, row, rowLine, spacer, flagNeedsYou and hiddenToggle, so the center reads as the same design system. Specifics:
- `.inbox`: padding 0 0 24px, overflow-y auto.
- `.header`: like `.railHeader`.
- `.label` / `.count`: like `.railLabel` / `.railCount`.
- `.row`: like `.row`, plus `display: block`.
- `.name`: like `.rowName`.
- `.age`: like `.agentAge`.
- `.preview`: mono 11px, color `var(--text-tertiary)`, single line with `white-space: nowrap; overflow: hidden; text-overflow: ellipsis`.
- `.detail`: like `.rowSubtitle`.
- `.toggle`: like `.hiddenToggle`.
- `.dimmed`: like `.rowHidden`.
- `.empty`: like `.railEmpty`.
- `.clear`: a small unstyled button, color `var(--text-tertiary)`, underline on hover.

Copy exact token names from `Rail.module.css`; use no token that is absent from `theme.css`.

`ui/src/testing/fakeStore.tsx`:

```tsx
export function renderWithStore(
  ui: ReactElement,
  patch: Partial<State> = {},
  { route = '/' }: { route?: string } = {},
) {
  setStoreState(patch)
  return render(<MemoryRouter initialEntries={[route]}>{ui}</MemoryRouter>)
}
```

`ui/src/Shell.tsx`:
- Delete `MainPlaceholder`; its new-bus logic now lives in `Inbox`.
- Render `<Outlet context={{ query }} />`.
- Keep `<Rail query={query} />`.

`ui/src/App.tsx`: `<Route index element={<Inbox />} />`, importing from `./inbox/Inbox`.

Update `Shell.test.tsx` and `App.test.tsx`: replace each `main-placeholder` assertion with an `Inbox` assertion (for example `inbox-row` or `inbox-empty`) that tests the same intent. Read each test to understand what it guarded before changing it.

- [ ] **Step 4: Run the tests**

Run: `cd ui && npm run typecheck && npm run format:check && npm test`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add ui/src
git commit -m "feat: conversation inbox as the console landing view"
```

---

### Task 4: The rail becomes the agent list

**Files:**
- Modify: `ui/src/rail/Rail.tsx` (agents only; the online/offline split; offline collapsed; the relayers line stays)
- Modify: `ui/src/rail/AgentRow.tsx` (the row links to the filter; "details →" is a sibling link, not nested)
- Modify: `ui/src/rail/Rail.module.css` (`.offlineToggle`, `.details`, and a `.row.selected` reuse)
- Delete: `ui/src/rail/RoomRow.tsx` (its wording now lives in `ConversationRow`)
- Modify: `ui/src/rail/Rail.test.tsx` (remove the room tests that Task 3 now covers; add agent tests)

**Interfaces:**
- Consumes: the `agent` URL parameter convention; `statusText`.
- Produces: test ids `rail-agent-row` (`data-agent=<name>`), `rail-agent-details`, `rail-offline-toggle`, plus the existing `agent-name`, `agent-status-<name>` and `relayer-note`.

- [ ] **Step 1: Write the failing tests** (add to `ui/src/rail/Rail.test.tsx`; reuse the file's existing fixtures and helpers)

```tsx
test('the rail lists agents only: blocked first, then online, offline collapsed', () => {
  const mk = (name: string, online: boolean, blocked = false, lastSeen = 1) => ({
    name, host: 'h', version: null, online, isHuman: false, isRelayer: false, lastSeen, buckets: [0],
    status: blocked
      ? { state: 'blocked_on_human' as const, since: 0, last_heartbeat: null, reason: 'permission_prompt', text: null, text_at: null, quiet: false }
      : null,
  })
  renderWithStore(<Rail />, {
    rail: { rooms: [], relayers: [], agents: [mk('idle-online', true, false, 9), mk('gone', false), mk('waiting', true, true, 1)] },
  })
  expect(screen.queryByTestId('room-name')).toBeNull()
  const visible = screen.getAllByTestId('agent-name').map((n) => n.textContent)
  expect(visible).toEqual(['waiting', 'idle-online'])
  fireEvent.click(screen.getByTestId('rail-offline-toggle'))
  expect(screen.getAllByTestId('agent-name').map((n) => n.textContent)).toEqual([
    'waiting',
    'idle-online',
    'gone',
  ])
})

test('a search expands the offline group so matches are visible', () => {
  renderWithStore(<Rail query="gon" />, {
    rail: { rooms: [], relayers: [], agents: [{ ...rail.agents[0], name: 'gone', online: false }] },
  })
  expect(screen.getAllByTestId('agent-name').map((n) => n.textContent)).toEqual(['gone'])
})

test('an agent row links to the inbox filter, and toggles off when selected', () => {
  const a = { ...rail.agents[0], name: 'caas#2', online: true }
  const { unmount } = renderWithStore(<Rail />, { rail: { rooms: [], relayers: [], agents: [a] } })
  expect(screen.getByTestId('rail-agent-row').getAttribute('href')).toBe(
    `/?agent=${encodeURIComponent('caas#2')}`,
  )
  unmount()
  renderWithStore(
    <Rail />,
    { rail: { rooms: [], relayers: [], agents: [a] } },
    { route: `/?agent=${encodeURIComponent('caas#2')}` },
  )
  const row = screen.getByTestId('rail-agent-row')
  expect(row.getAttribute('href')).toBe('/')
  expect(row.className).toMatch(/selected/)
  const details = screen.getByTestId('rail-agent-details')
  expect(details.getAttribute('href')).toBe(`/agents/${encodeURIComponent('caas#2')}`)
  // Never an anchor inside an anchor.
  expect(details.closest('a')?.parentElement?.closest('a')).toBeNull()
  expect(row.querySelector('a')).toBeNull()
})
```

Adapt the `rail.agents[0]` references to whatever agent fixture this file already defines. Its top-level `rail` constant already includes `status: null`, `isRelayer` and the rest.

- [ ] **Step 2: Confirm they fail**

Run: `cd ui && npx vitest run src/rail/Rail.test.tsx`

- [ ] **Step 3: Implement**

`ui/src/rail/AgentRow.tsx`. The row wrapper is a `div`; the main `Link` and the details `Link` are siblings:

```tsx
import { Link, useMatch, useSearchParams } from 'react-router-dom'
// (keep the existing imports: RailAgent, Chip, age, statusText, styles, VolumeStrip)

export function AgentRow({ agent, now }: { agent: RailAgent; now: number }) {
  const onIndex = useMatch('/') !== null
  const [params] = useSearchParams()
  const selected = onIndex && params.get('agent') === agent.name
  const to = selected ? '/' : `/?agent=${encodeURIComponent(agent.name)}`
  return (
    <div className={`${styles.agentRowWrap} ${selected ? styles.selected : ''}`}>
      <Link
        to={to}
        className={`${styles.row} ${styles.agentRow} ${selected ? styles.selected : ''}`}
        data-testid="rail-agent-row"
        data-agent={agent.name}
      >
        {/* … the existing rowLine and status line, unchanged … */}
      </Link>
      {selected && (
        <Link
          to={`/agents/${encodeURIComponent(agent.name)}`}
          className={styles.details}
          data-testid="rail-agent-details"
        >
          details →
        </Link>
      )}
    </div>
  )
}
```

Move the existing JSX (the `rowLine` div and the status div) into the main `Link` unchanged.

`ui/src/rail/Rail.tsx`:
- Remove `rank`, `sortRooms`, the rooms section and the hidden-rooms toggle (they live in the inbox now), and the `RoomRow` import.
- Keep `sortAgents` and `matches`.
- Split the sorted, filtered agents into `online` and `offline`. Render the online agents, then a toggle button `▸ N offline` (`▾` when open; `data-testid="rail-offline-toggle"`). Expand the offline list when the toggle is open **or** when `query.trim() !== ''`.
- Header: `agents` and `{online.length} of {total} online`.
- When the query matches no agent, show `nothing matched "<query>"` in the rail, as before.
- Keep the relayers line.
- Keep the existing `.railHeader.agents` header styling and drop the top padding override if the agents header is now first.

`ui/src/rail/Rail.module.css`:
- Add `.agentRowWrap { position: relative; }`.
- Add `.details`: absolutely positioned right 12px, top 8px; mono 10px; `var(--text-tertiary)`; underlined on hover.
- Add `.offlineToggle`, mirroring `.hiddenToggle`.
- Remove room-only rules no longer used (`.rowSubtitle`, `.flagNeedsYou`, `.rowHidden`, `.hiddenToggle`) only if nothing else references them. Grep first; the inbox has its own CSS module.

Delete `ui/src/rail/RoomRow.tsx`. Then grep for `RoomRow` and update or delete any remaining references.

`Rail.test.tsx`: delete the room-specific tests:
- flagged sort
- blocked subtitle
- needs-you subtitle
- room no-activity
- room link encoding
- room/agent same-name selection
- hidden-room footer and expansion
- the "query matching only agents still shows rooms" test

Task 3's inbox tests and Task 2's shaping tests now cover each of these behaviors. List each deleted test and its replacement in your report. Keep and adapt the agent tests:
- sorting
- the online/offline styles
- the human/relayer badges
- the ticker
- the `#` encoding: an agent row now links to `/?agent=…`, not `/agents/…`, so the expectation changes; keep the intent, "encoded correctly"
- the relayers line

- [ ] **Step 4: Run the tests**

Run: `cd ui && npm run typecheck && npm run format:check && npm test`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add -A ui/src
git commit -m "feat: the rail is the agent list, and clicking an agent filters the inbox"
```

---

### Task 5: Refresh the rail on `message_sent`, throttled

**Files:**
- Modify: `ui/src/data/store.ts`
- Test: `ui/src/data/store.test.ts`

**Interfaces:**
- Produces: `export const RAIL_EVENT_THROTTLE_MS = 2000` in `store.ts`; internal `refreshRailSoon()`.

- [ ] **Step 1: Write the failing tests** (`ui/src/data/store.test.ts`; use the file's `fakeLive`/`makeStore`)

```ts
test('a burst of message_sent events causes one immediate and one trailing rail refresh', async () => {
  vi.useFakeTimers()
  try {
    const fetchRail = vi.fn(async () => emptyRail)
    const store = makeStore({ fetchRail })
    await store.start() // the start fetch
    fetchRail.mockClear()
    const ev = (id: number) => ({
      type: 'event', id, kind: 'message_sent', agent: 'a', room: 'r', detail: {}, created_at: 0,
    })
    for (let i = 1; i <= 10; i++) live.emit('event', ev(i))
    expect(fetchRail).toHaveBeenCalledTimes(1) // immediate
    await vi.advanceTimersByTimeAsync(RAIL_EVENT_THROTTLE_MS)
    expect(fetchRail).toHaveBeenCalledTimes(2) // one trailing, not ten
    store.stop()
  } finally {
    vi.useRealTimers()
  }
})

test('other event kinds do not refresh the rail', async () => {
  const fetchRail = vi.fn(async () => emptyRail)
  const store = makeStore({ fetchRail })
  await store.start()
  fetchRail.mockClear()
  live.emit('event', { type: 'event', id: 1, kind: 'ack', agent: 'a', room: 'r', detail: {}, created_at: 0 })
  expect(fetchRail).not.toHaveBeenCalled()
  store.stop()
})

test('a failing rail fetch during a burst keeps the previous rail and throws nothing', async () => {
  vi.useFakeTimers()
  try {
    let fail = false
    const fetchRail = vi.fn(async () => {
      if (fail) throw new Error('down')
      return emptyRail
    })
    const store = makeStore({ fetchRail })
    await store.start()
    const before = store.getState().rail
    fail = true
    for (let i = 1; i <= 5; i++)
      live.emit('event', { type: 'event', id: i, kind: 'message_sent', agent: 'a', room: 'r', detail: {}, created_at: 0 })
    await vi.advanceTimersByTimeAsync(RAIL_EVENT_THROTTLE_MS * 3)
    expect(store.getState().rail).toBe(before)
    expect(fetchRail.mock.calls.length).toBeLessThanOrEqual(3) // start + immediate + trailing
    store.stop()
  } finally {
    vi.useRealTimers()
  }
})
```

Check how this file reads state (it may be `store.getState()` or a `subscribe` pattern) and how `live` is shared across tests (it is a module-level `fakeLive()` instance), and match that. Import `RAIL_EVENT_THROTTLE_MS` from `./store`.

- [ ] **Step 2: Confirm they fail**

Run: `cd ui && npx vitest run src/data/store.test.ts`

- [ ] **Step 3: Implement** (`ui/src/data/store.ts`)

At module level:

```ts
/// A message changes the inbox (last message, activity, flags), but a burst of
/// sends must not become a burst of `/api/rail` fetches: at most one per this
/// interval, plus one trailing fetch so the burst's last message always lands.
export const RAIL_EVENT_THROTTLE_MS = 2000
```

Inside `createStore`, after `refreshRail` is defined:

```ts
  let lastEventRefresh = 0
  let trailingRefresh: ReturnType<typeof setTimeout> | null = null
  const refreshRailSoon = () => {
    const wait = lastEventRefresh + RAIL_EVENT_THROTTLE_MS - Date.now()
    if (wait <= 0) {
      lastEventRefresh = Date.now()
      void refreshRail()
      return
    }
    if (trailingRefresh) return
    trailingRefresh = setTimeout(() => {
      trailingRefresh = null
      lastEventRefresh = Date.now()
      void refreshRail()
    }, wait)
  }
```

In the existing `'event'` handler, after `setState(...)`: `if (event.kind === 'message_sent') refreshRailSoon()`. `refreshRail` is defined later in the function body; it is called only at event time, so the forward reference is safe. Mirror the comment the file already uses for `repairRoom`.

In `stop()`: `if (trailingRefresh) { clearTimeout(trailingRefresh); trailingRefresh = null }`.

- [ ] **Step 4: Run the tests**

Run: `cd ui && npm run typecheck && npm run format:check && npm test`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add ui/src/data
git commit -m "feat: the inbox refreshes on message_sent events, throttled"
```

---

### Task 6: Visual check, docs, and the final gate

**Files:**
- Modify: `docs/DEPLOY.md`. In "Reading the record afterwards", add one sentence: the console opens on the agent list (with status) and an inbox of the last 24h of conversations; clicking an agent filters the inbox.

- [ ] **Step 1: Visual check on a scratch bus**

```bash
make ui && cargo build
SCRATCH=$(mktemp -d)
./target/debug/claude-bus serve --port 17787 --data "$SCRATCH/data" > "$SCRATCH/bus.log" 2>&1 &
BUS_PID=$!
```

Then populate it. Use a few raw-websocket agents, for example a small Node script with the global `WebSocket`, Node 22 or later. They register, join rooms, send messages (including a long multi-line one and one to an offline agent so a room shows `blocked`), and send `status` frames (one `blocked_on_human`).

Open `http://localhost:17787/` (Playwright MCP if available) and check:
- the rail lists agents, blocked first, with offline collapsed
- the inbox lists recent rooms with previews and flags
- clicking an agent filters and highlights it, and "details →" opens the agent page
- a new message updates the inbox within about 2s
- both themes look right (the theme toggle in the top bar)

Then `kill $BUS_PID` and `rm -rf "$SCRATCH"`.

- [ ] **Step 2: The full gate, in both bundle states**

```bash
find ui/dist -mindepth 1 ! -name .gitkeep -delete
cargo +nightly fmt --check
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
git diff --exit-code -- ui/src/types
(cd ui && npm ci && npm run typecheck && npm run format:check && npm test && npm run build)
cargo test
```

Expected: everything passes in both states.

- [ ] **Step 3: Commit**

```bash
git add docs/DEPLOY.md
git commit -m "docs: the console's landing view"
```
