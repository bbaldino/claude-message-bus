import { screen } from '@testing-library/react'
import { beforeEach, expect, test } from 'vitest'
import { renderWithStore } from '../testing/fakeStore'
import type { State } from '../data/store'
import { RoomScreen } from './RoomScreen'
import styles from './Transcript.module.css'

// Mutable, like the dock's own test: `dockOpen` drives the same narrow/push
// decision in both RoomScreen and EventsDock, and this pins that RoomScreen
// reads it from the store rather than tracking its own copy.
let dockOpen = false

const base: Partial<State> = {
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

beforeEach(() => {
  dockOpen = false
})

const renderScreen = () => renderWithStore(<RoomScreen />, { ...base, dockOpen })

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

test('the transcript body narrows when the dock is open, driven by the store value', () => {
  dockOpen = true
  renderScreen()
  const bodies = document.querySelectorAll(`.${styles.body}`)
  expect(bodies.length).toBeGreaterThan(0)
  bodies.forEach((el) => expect(el.classList.contains(styles.bodyNarrow)).toBe(true))
})

test('the transcript body stays wide when the dock is closed', () => {
  dockOpen = false
  renderScreen()
  const bodies = document.querySelectorAll(`.${styles.body}`)
  expect(bodies.length).toBeGreaterThan(0)
  bodies.forEach((el) => expect(el.classList.contains(styles.bodyNarrow)).toBe(false))
})
