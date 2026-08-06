import { render, screen, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { expect, test, vi } from 'vitest'
import { Rail } from './Rail'
import type { RailSummary } from '../types/RailSummary'

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
