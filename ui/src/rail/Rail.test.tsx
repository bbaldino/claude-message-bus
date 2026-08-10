import { act, render, screen, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, test, vi } from 'vitest'
import { renderWithStore } from '../testing/fakeStore'
import { Rail } from './Rail'
import { RoomRow } from './RoomRow'
import { AgentRow } from './AgentRow'
import styles from './Rail.module.css'
import type { RailSummary } from '../types/RailSummary'
import type { RailRoom } from '../types/RailRoom'
import type { RailAgent } from '../types/RailAgent'

const rail: RailSummary = {
  rooms: [
    { name: 'quiet', members: ['a'], lastActivity: 9, buckets: [0], flag: null, hidden: false },
    {
      name: 'stuck',
      members: ['a'],
      lastActivity: 1,
      buckets: [1],
      flag: { kind: 'needsYou', exchanges: 20 },
      hidden: false,
    },
    {
      name: 'waiting',
      members: ['a'],
      lastActivity: 2,
      buckets: [1],
      flag: { kind: 'blocked', queued: 2, waitingOn: ['caas'] },
      hidden: false,
    },
  ],
  agents: [
    {
      name: 'offline-one',
      host: 'h',
      version: '0.3.3',
      online: false,
      isHuman: false,
      lastSeen: 5,
      buckets: [0],
    },
    {
      name: 'online-one',
      host: 'h',
      version: '0.3.3',
      online: true,
      isHuman: false,
      lastSeen: 1,
      buckets: [1],
    },
  ],
}

function renderRail(query?: string) {
  return renderWithStore(<Rail query={query} />, { rail })
}

function renderRoomRow(room: RailRoom, path = '/') {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <RoomRow room={room} />
    </MemoryRouter>,
  )
}

function renderAgentRow(agent: RailAgent, now = Date.now(), path = '/') {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <AgentRow agent={agent} now={now} />
    </MemoryRouter>,
  )
}

afterEach(() => {
  // Belt-and-braces: any test that reaches for fake timers restores real ones,
  // even if it fails before its own cleanup runs.
  vi.useRealTimers()
})

test('flagged rooms float above unflagged, needs-you above blocked', () => {
  renderRail()
  const names = screen.getAllByTestId('room-name').map((n) => n.textContent)
  expect(names).toEqual(['stuck', 'waiting', 'quiet'])
})

test('a blocked room composes its subtitle from the flag data', () => {
  // The server ships data, not sentences — this is where the sentence is written.
  renderRail()
  expect(screen.getByText('waiting on caas · 2 queued, 0 delivered')).toBeDefined()
})

test('a needs-you room states the exchange count', () => {
  renderRail()
  expect(screen.getByText('hit 20 exchanges · waiting on you')).toBeDefined()
})

test('online agents sort above offline', () => {
  renderRail()
  const names = screen.getAllByTestId('agent-name').map((n) => n.textContent)
  expect(names).toEqual(['online-one', 'offline-one'])
})

test('the agent section counts how many are online', () => {
  renderRail()
  const header = screen.getByTestId('agents-header')
  expect(within(header).getByText('1 of 2 online')).toBeDefined()
})

test('a query filters both rooms and agents by a case-insensitive substring match', () => {
  renderRail('ONLINE')
  expect(screen.getAllByTestId('agent-name').map((n) => n.textContent)).toEqual(['online-one'])
  // None of the room fixtures contain "online" — the rooms section empties out.
  expect(screen.queryAllByTestId('room-name')).toEqual([])
})

test('an empty query restores every room and agent', () => {
  renderRail('')
  expect(screen.getAllByTestId('room-name')).toHaveLength(3)
  expect(screen.getAllByTestId('agent-name')).toHaveLength(2)
})

test('the agent count reflects the filtered list, not the full one', () => {
  // Unfiltered this fixture is "1 of 2 online" (see the test above). Filtering
  // down to a single, online agent must move the denominator to 1, not leave
  // it reporting against the full unfiltered set of 2.
  renderRail('online-one')
  const header = screen.getByTestId('agents-header')
  expect(within(header).getByText('1 of 1 online')).toBeDefined()
})

test('an online agent name is styled distinguishably from an offline one', () => {
  renderAgentRow({
    name: 'online-agent',
    host: 'h',
    version: null,
    online: true,
    isHuman: false,
    lastSeen: 1,
    buckets: [0],
  })
  const el = screen.getByTestId('agent-name')
  expect(el.classList.contains(styles.online)).toBe(true)
  expect(el.classList.contains(styles.offline)).toBe(false)
})

test('an offline agent name carries the offline class instead', () => {
  renderAgentRow({
    name: 'offline-agent',
    host: 'h',
    version: null,
    online: false,
    isHuman: false,
    lastSeen: 1,
    buckets: [0],
  })
  const el = screen.getByTestId('agent-name')
  expect(el.classList.contains(styles.offline)).toBe(true)
  expect(el.classList.contains(styles.online)).toBe(false)
})

test('an agent flagged as human renders the human badge', () => {
  renderAgentRow({
    name: 'bbaldino',
    host: 'h',
    version: null,
    online: true,
    isHuman: true,
    lastSeen: 1,
    buckets: [0],
  })
  expect(screen.getByText('human')).toBeDefined()
})

test('a non-human agent renders no human badge', () => {
  renderAgentRow({
    name: 'caas',
    host: 'h',
    version: null,
    online: true,
    isHuman: false,
    lastSeen: 1,
    buckets: [0],
  })
  expect(screen.queryByText('human')).toBeNull()
})

test('a shared ticker re-derives relative age on an interval, with no store update', () => {
  // At t=60s: both fixture agents (lastSeen 1ms, 5ms) read "59s". After the
  // ticker fires once more, at t=61s, both cross into "1m" — purely from the
  // clock advancing, not from `rail` changing.
  vi.useFakeTimers()
  vi.setSystemTime(60_000)
  const { unmount } = renderRail()
  const ages = () => screen.getAllByTestId('agent-age').map((el) => el.textContent)

  expect(ages()).toEqual(['59s', '59s'])

  act(() => {
    vi.advanceTimersByTime(1000)
  })
  expect(ages()).toEqual(['1m', '1m'])

  unmount()
  expect(vi.getTimerCount()).toBe(0)
})

test('a room with no last activity renders its name as silent', () => {
  renderRoomRow({
    name: 'ghost',
    members: ['a'],
    lastActivity: null,
    buckets: [0],
    flag: null,
    hidden: false,
  })
  expect(screen.getByTestId('room-name').classList.contains(styles.empty)).toBe(true)
})

test('a room name with special characters is percent-encoded in its link', () => {
  const { container } = renderRoomRow({
    name: 'dm:a|b',
    members: ['a', 'b'],
    lastActivity: 5,
    buckets: [0],
    flag: null,
    hidden: false,
  })
  expect(container.querySelector('a')?.getAttribute('href')).toBe('/rooms/dm%3Aa%7Cb')
})

test('an agent name containing # is percent-encoded in its link', () => {
  const { container } = renderAgentRow({
    name: 'network-debug#2',
    host: 'h',
    version: null,
    online: true,
    isHuman: false,
    lastSeen: 1,
    buckets: [0],
  })
  expect(container.querySelector('a')?.getAttribute('href')).toBe('/agents/network-debug%232')
})

test('a room and an agent sharing a name are each selected only on their own route', () => {
  // Nothing in the data model stops a room and an agent from sharing a name,
  // so the selected-row check has to key off which route family is active
  // (via useMatch), not just compare a bare `:name` param.
  const room: RailRoom = {
    name: 'shared',
    members: ['a'],
    lastActivity: 1,
    buckets: [0],
    flag: null,
    hidden: false,
  }
  const agent: RailAgent = {
    name: 'shared',
    host: 'h',
    version: null,
    online: true,
    isHuman: false,
    lastSeen: 1,
    buckets: [0],
  }

  const room1 = renderRoomRow(room, '/rooms/shared')
  expect(room1.container.querySelector('a')?.classList.contains(styles.selected)).toBe(true)
  room1.unmount()

  const agent1 = renderAgentRow(agent, Date.now(), '/rooms/shared')
  expect(agent1.container.querySelector('a')?.classList.contains(styles.selected)).toBe(false)
  agent1.unmount()

  const agent2 = renderAgentRow(agent, Date.now(), '/agents/shared')
  expect(agent2.container.querySelector('a')?.classList.contains(styles.selected)).toBe(true)
  agent2.unmount()

  const room2 = renderRoomRow(room, '/agents/shared')
  expect(room2.container.querySelector('a')?.classList.contains(styles.selected)).toBe(false)
  room2.unmount()
})

test('a query matching nothing at all shows a message referencing it, not two empty sections', () => {
  renderRail('zzz-no-such-thing')
  expect(screen.getByText('nothing matched "zzz-no-such-thing"')).toBeDefined()
  expect(screen.queryByText('rooms')).toBeNull()
  expect(screen.queryByTestId('agents-header')).toBeNull()
})

test('a query matching only agents still shows the (empty) rooms section as normal', () => {
  // Only the fully-empty case gets the message — a query that legibly narrows
  // one section to nothing is a real result, not something to explain away.
  renderRail('online-one')
  expect(screen.queryByText(/nothing matched/)).toBeNull()
})

test('an empty query shows every room and agent with no "nothing matched" message', () => {
  renderRail('')
  expect(screen.queryByText(/nothing matched/)).toBeNull()
})
