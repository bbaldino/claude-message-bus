import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, expect, test, vi } from 'vitest'
import { renderWithStore } from '../testing/fakeStore'
import { Rail } from './Rail'
import { AgentRow } from './AgentRow'
import styles from './Rail.module.css'
import type { RailSummary } from '../types/RailSummary'
import type { RailAgent } from '../types/RailAgent'

const rail: RailSummary = {
  rooms: [
    {
      name: 'quiet',
      members: ['a'],
      lastActivity: 9,
      buckets: [0],
      flag: null,
      hidden: false,
      lastMessage: null,
    },
    {
      name: 'stuck',
      members: ['a'],
      lastActivity: 1,
      buckets: [1],
      flag: { kind: 'needsYou', exchanges: 20 },
      hidden: false,
      lastMessage: null,
    },
    {
      name: 'waiting',
      members: ['a'],
      lastActivity: 2,
      buckets: [1],
      flag: { kind: 'blocked', queued: 2, waitingOn: ['caas'] },
      hidden: false,
      lastMessage: null,
    },
  ],
  agents: [
    {
      name: 'offline-one',
      host: 'h',
      version: '0.3.3',
      online: false,
      isHuman: false,
      isRelayer: false,
      lastSeen: 5,
      buckets: [0],
      status: null,
    },
    {
      name: 'online-one',
      host: 'h',
      version: '0.3.3',
      online: true,
      isHuman: false,
      isRelayer: false,
      lastSeen: 1,
      buckets: [1],
      status: null,
    },
  ],
  relayers: [],
}

function renderRail(query?: string) {
  return renderWithStore(<Rail query={query} />, { rail })
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

test('online agents sort above offline, and offline agents start collapsed', () => {
  renderRail()
  const names = () => screen.getAllByTestId('agent-name').map((n) => n.textContent)
  expect(names()).toEqual(['online-one'])
  expect(screen.getByTestId('rail-offline-toggle').textContent).toMatch(/1 offline/)
  fireEvent.click(screen.getByTestId('rail-offline-toggle'))
  expect(names()).toEqual(['online-one', 'offline-one'])
})

test('the agent section counts how many are online', () => {
  renderRail()
  const header = screen.getByTestId('agents-header')
  expect(within(header).getByText('1 of 2 online')).toBeDefined()
})

test('a query filters agents by a case-insensitive substring match', () => {
  renderRail('ONLINE')
  expect(screen.getAllByTestId('agent-name').map((n) => n.textContent)).toEqual(['online-one'])
})

test('the rail lists agents only, no rooms', () => {
  renderRail()
  expect(screen.queryAllByTestId('room-name')).toEqual([])
  expect(screen.queryByText('rooms')).toBeNull()
})

test('a search expands the offline group so matches are visible', () => {
  renderRail('offline')
  expect(screen.getAllByTestId('agent-name').map((n) => n.textContent)).toEqual(['offline-one'])
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
    isRelayer: false,
    lastSeen: 1,
    buckets: [0],
    status: null,
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
    isRelayer: false,
    lastSeen: 1,
    buckets: [0],
    status: null,
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
    isRelayer: false,
    lastSeen: 1,
    buckets: [0],
    status: null,
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
    isRelayer: false,
    lastSeen: 1,
    buckets: [0],
    status: null,
  })
  expect(screen.queryByText('human')).toBeNull()
})

test('a configured relayer renders the relayer badge', () => {
  renderAgentRow({
    name: 'hub',
    host: 'h',
    version: null,
    online: true,
    isHuman: false,
    isRelayer: true,
    lastSeen: 1,
    buckets: [0],
    status: null,
  })
  expect(screen.getByText('relayer')).toBeDefined()
  // A relayer is not a human, and must not be dressed as one.
  expect(screen.queryByText('human')).toBeNull()
})

test('an agent that is not a relayer renders no relayer badge', () => {
  renderAgentRow({
    name: 'caas',
    host: 'h',
    version: null,
    online: true,
    isHuman: false,
    isRelayer: false,
    lastSeen: 1,
    buckets: [0],
    status: null,
  })
  expect(screen.queryByText('relayer')).toBeNull()
})

test('the rail states the configured relayers', () => {
  renderWithStore(<Rail />, { rail: { ...rail, relayers: ['hub', 'relay-2'] } })
  expect(screen.getByTestId('relayer-note').textContent).toBe('relayers: hub, relay-2')
})

test('a bus with no relayers says so rather than omitting the line', () => {
  renderRail()
  expect(screen.getByTestId('relayer-note').textContent).toBe('relayers: (none)')
})

test('a relayer name no agent uses is still stated, with nothing badged', () => {
  // The mistyped `--relayer hubb`: the line is the only visible trace of it.
  renderWithStore(<Rail />, { rail: { ...rail, relayers: ['hubb'] } })
  expect(screen.getByTestId('relayer-note').textContent).toBe('relayers: hubb')
  expect(screen.queryByText('relayer')).toBeNull()
})

test('a shared ticker re-derives relative age on an interval, with no store update', () => {
  // At t=60s: both fixture agents (lastSeen 1ms, 5ms) read "59s". After the
  // ticker fires once more, at t=61s, both cross into "1m" — purely from the
  // clock advancing, not from `rail` changing.
  vi.useFakeTimers()
  vi.setSystemTime(60_000)
  const { unmount } = renderRail()
  fireEvent.click(screen.getByTestId('rail-offline-toggle'))
  const ages = () => screen.getAllByTestId('agent-age').map((el) => el.textContent)

  expect(ages()).toEqual(['59s', '59s'])

  act(() => {
    vi.advanceTimersByTime(1000)
  })
  expect(ages()).toEqual(['1m', '1m'])

  unmount()
  expect(vi.getTimerCount()).toBe(0)
})

test('an agent name containing # is percent-encoded in its link', () => {
  const { container } = renderAgentRow({
    name: 'network-debug#2',
    host: 'h',
    version: null,
    online: true,
    isHuman: false,
    isRelayer: false,
    lastSeen: 1,
    buckets: [0],
    status: null,
  })
  expect(container.querySelector('a')?.getAttribute('href')).toBe('/?agent=network-debug%232')
})

test('an agent row links to the inbox filter, and toggles off when selected', () => {
  const a = { ...rail.agents[1], name: 'caas#2 &x?' }
  const enc = encodeURIComponent('caas#2 &x?')
  const { unmount } = renderAgentRow(a)
  expect(screen.getByTestId('rail-agent-row').getAttribute('href')).toBe(`/?agent=${enc}`)
  expect(screen.queryByTestId('rail-agent-details')).toBeNull()
  unmount()

  renderAgentRow(a, Date.now(), `/?agent=${enc}`)
  const row = screen.getByTestId('rail-agent-row')
  expect(row.getAttribute('href')).toBe('/')
  expect(row.classList.contains(styles.selected)).toBe(true)
  expect(screen.getByTestId('rail-agent-details').getAttribute('href')).toBe(`/agents/${enc}`)
})

test('the details link is never an anchor nested inside the row anchor', () => {
  const a = { ...rail.agents[1], name: 'hub' }
  renderAgentRow(a, Date.now(), '/?agent=hub')
  const row = screen.getByTestId('rail-agent-row')
  const details = screen.getByTestId('rail-agent-details')
  expect(row.querySelector('a')).toBeNull()
  expect(row.contains(details)).toBe(false)
})

test('an agent is not shown selected on a room page, and its link still filters', () => {
  const a = { ...rail.agents[1], name: 'shared' }
  renderAgentRow(a, Date.now(), '/rooms/shared?agent=shared')
  const row = screen.getByTestId('rail-agent-row')
  expect(row.classList.contains(styles.selected)).toBe(false)
  expect(row.getAttribute('href')).toBe('/?agent=shared')
})

test('a query matching nothing at all shows a message referencing it, not two empty sections', () => {
  renderRail('zzz-no-such-thing')
  expect(screen.getByText('nothing matched "zzz-no-such-thing"')).toBeDefined()
  expect(screen.queryByTestId('agents-header')).toBeNull()
})

test('an empty query shows every room and agent with no "nothing matched" message', () => {
  renderRail('')
  expect(screen.queryByText(/nothing matched/)).toBeNull()
})

test('a blocked agent sorts first and shows its status line', () => {
  const blocked = {
    ...rail.agents[0],
    name: 'needs-me',
    online: true,
    lastSeen: 0,
    status: {
      state: 'blocked_on_human' as const,
      since: Date.now() - 120_000,
      last_heartbeat: null,
      reason: 'permission_prompt',
      text: null,
      text_at: null,
      quiet: false,
    },
  }
  renderWithStore(<Rail />, { rail: { ...rail, agents: [...rail.agents, blocked] } })
  const names = screen.getAllByTestId('agent-name').map((n) => n.textContent)
  expect(names[0]).toBe('needs-me')
  expect(screen.getByTestId('agent-status-needs-me').textContent).toMatch(
    /^blocked on human \(permission_prompt\)/,
  )
})

test('status text is rendered as text, never as HTML', () => {
  const a = {
    ...rail.agents[1],
    status: {
      state: 'idle' as const,
      since: Date.now(),
      last_heartbeat: null,
      reason: null,
      text: '<img src=x onerror=alert(1)>',
      text_at: Date.now(),
      quiet: false,
    },
  }
  const { container } = renderWithStore(<Rail />, { rail: { ...rail, agents: [a] } })
  expect(container.querySelector('img')).toBeNull()
  expect(screen.getByTestId(`agent-status-${a.name}`).textContent).toContain('<img')
})
