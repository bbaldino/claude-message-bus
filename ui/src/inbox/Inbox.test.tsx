import { fireEvent, screen } from '@testing-library/react'
import { expect, test } from 'vitest'
import { renderWithStore } from '../testing/fakeStore'
import type { RailAgent } from '../types/RailAgent'
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
const agent: RailAgent = {
  name: 'hub',
  host: 'h',
  version: null,
  online: true,
  isHuman: false,
  isRelayer: false,
  lastSeen: now,
  buckets: [0],
  status: null,
}
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
    {
      rail: rail([
        room({ name: 'r', members: ['caas#2 &x?'] }),
        room({ name: 's', members: ['caas'] }),
      ]),
    },
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
  renderWithStore(
    <Inbox />,
    { rail: rail([room({ name: 'b', members: ['x'] })]) },
    { route: '/?agent=hub' },
  )
  expect(screen.getByTestId('inbox-empty').textContent).toMatch(
    /no conversations with hub in the last 24h/,
  )
})

test('hidden rooms sit behind their own toggle', () => {
  renderWithStore(<Inbox />, {
    rail: rail([room({ name: 'v' }), room({ name: 'h', hidden: true })]),
  })
  expect(rowNames()).toEqual(['v'])
  fireEvent.click(screen.getByTestId('inbox-hidden-toggle'))
  expect(rowNames()).toContain('h')
})

test('a flagged room shows its chip and the flag detail', () => {
  renderWithStore(<Inbox />, {
    rail: rail([
      room({ name: 'stuck', flag: { kind: 'blocked', queued: 2, waitingOn: ['caas'] } }),
    ]),
  })
  expect(screen.getByText('blocked')).toBeDefined()
  expect(screen.getByText('waiting on caas · 2 queued, 0 delivered')).toBeDefined()
})

test('a bus with no agents shows the new-bus screen', () => {
  renderWithStore(<Inbox />, { rail: { rooms: [], agents: [], relayers: [] } })
  expect(screen.queryByTestId('inbox-empty')).toBeNull()
  expect(screen.getByText('The bus is running. Nothing has joined it.')).toBeDefined()
})

// Moved from Shell.test.tsx with the placeholder it tested. Ordering matters:
// `Inbox` must check `!rail` before it reads `rail.agents` — the other way round
// crashes on a null rail, and short of crashing would tell a populated bus's
// owner that nothing has joined it on every page load until the rail arrives.

test('a null rail renders neither the new-bus state nor the inbox', () => {
  renderWithStore(<Inbox />, { rail: null })
  expect(screen.queryByTestId('inbox-empty')).toBeNull()
  expect(screen.queryByText('The bus is running. Nothing has joined it.')).toBeNull()
})

test('a populated rail renders the inbox, not the new-bus state', () => {
  renderWithStore(<Inbox />, { rail: rail([room({ name: 'a' })]) })
  expect(rowNames()).toEqual(['a'])
  expect(screen.queryByText('The bus is running. Nothing has joined it.')).toBeNull()
})
