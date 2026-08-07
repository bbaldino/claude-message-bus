import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { expect, test, vi } from 'vitest'
import { RoomScreen } from './RoomScreen'

const base = {
  rail: {
    rooms: [
      { name: 'protocol', members: ['caas', 'hub'], lastActivity: 5, buckets: [1], flag: null },
    ],
    agents: [
      {
        name: 'caas',
        host: 'h',
        version: '1',
        online: true,
        isHuman: false,
        lastSeen: 5,
        buckets: [1],
      },
      {
        name: 'hub',
        host: 'h',
        version: '1',
        online: false,
        isHuman: false,
        lastSeen: 4,
        buckets: [0],
      },
    ],
  },
  events: [],
  roomEvents: [
    {
      id: 9,
      kind: 'message_sent',
      agent: 'caas',
      room: 'protocol',
      detail: { msg_id: 1, delivered_to: ['hub'], queued_for: ['network-debug#2'] },
      createdAt: 1,
    },
  ],
  messages: [
    {
      id: 1,
      room: 'protocol',
      from: 'caas',
      body: 'hello',
      done: false,
      human: false,
      createdAt: 1_700_000_000_000,
    },
    {
      id: 2,
      room: 'protocol',
      from: 'bbaldino',
      body: 'ack',
      done: true,
      human: true,
      createdAt: 1_700_000_100_000,
    },
  ],
  room: 'protocol',
  connection: 'live',
  hasMoreHistory: false,
  loadingOlder: false,
}

vi.mock('../useStore', () => ({
  useStore: () => base,
  store: { loadOlder: vi.fn() },
}))

const renderScreen = () =>
  render(
    <MemoryRouter>
      <RoomScreen />
    </MemoryRouter>,
  )

test('the header summarises membership from the rail', () => {
  renderScreen()
  expect(screen.getByText('2 members · 1 online')).toBeDefined()
})

test('renders a delivery line correlated from the event, with queued distinct', () => {
  renderScreen()
  expect(screen.getByText(/delivered to hub/)).toBeDefined()
  expect(screen.getByText(/queued for network-debug#2/)).toBeDefined()
})

test('a message with no correlating event renders no delivery line', () => {
  // Paging past the room-events window is expected to leave these absent, not
  // wrong — so nothing may be invented for them.
  renderScreen()
  const rows = screen.getAllByTestId('message-row')
  expect(rows[1].textContent).not.toMatch(/delivered to/)
})

test('a human message is marked as such', () => {
  renderScreen()
  expect(screen.getByText('human')).toBeDefined()
})

test('a done message shows the done chip without the prototype gloss', () => {
  renderScreen()
  expect(screen.getByText('done')).toBeDefined()
  expect(screen.queryByText(/sender considers/)).toBeNull()
})

test('a date divider opens the day', () => {
  renderScreen()
  expect(screen.getAllByTestId('date-divider').length).toBeGreaterThan(0)
})
