import { act, render, screen, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, test, vi } from 'vitest'
import { Rail } from './Rail'
import { RoomRow } from './RoomRow'
import { AgentRow } from './AgentRow'
import type { RailSummary } from '../types/RailSummary'
import type { RailRoom } from '../types/RailRoom'
import type { RailAgent } from '../types/RailAgent'

const rail: RailSummary = {
  rooms: [
    { name: 'quiet', members: ['a'], lastActivity: 9, buckets: [0], flag: null },
    {
      name: 'stuck',
      members: ['a'],
      lastActivity: 1,
      buckets: [1],
      flag: { kind: 'needsYou', exchanges: 20 },
    },
    {
      name: 'waiting',
      members: ['a'],
      lastActivity: 2,
      buckets: [1],
      flag: { kind: 'blocked', queued: 2, waitingOn: ['caas'] },
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

vi.mock('../useStore', () => ({
  useStore: () => ({ rail, events: [], messages: [], room: null, connection: 'live' }),
}))

function renderRail() {
  return render(
    <MemoryRouter>
      <Rail />
    </MemoryRouter>,
  )
}

function renderRoomRow(room: RailRoom) {
  return render(
    <MemoryRouter>
      <RoomRow room={room} />
    </MemoryRouter>,
  )
}

function renderAgentRow(agent: RailAgent, now = Date.now()) {
  return render(
    <MemoryRouter>
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
  expect(el.classList.contains('online')).toBe(true)
  expect(el.classList.contains('offline')).toBe(false)
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
  expect(el.classList.contains('offline')).toBe(true)
  expect(el.classList.contains('online')).toBe(false)
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
  renderRoomRow({ name: 'ghost', members: ['a'], lastActivity: null, buckets: [0], flag: null })
  expect(screen.getByTestId('room-name').classList.contains('empty')).toBe(true)
})

test('a room name with special characters is percent-encoded in its link', () => {
  const { container } = renderRoomRow({
    name: 'dm:a|b',
    members: ['a', 'b'],
    lastActivity: 5,
    buckets: [0],
    flag: null,
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
