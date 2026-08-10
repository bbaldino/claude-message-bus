import { fireEvent, screen } from '@testing-library/react'
import { beforeEach, expect, test, vi } from 'vitest'
import { renderWithStore, storeActions } from '../testing/fakeStore'
import type { State } from '../data/store'
import { RoomScreen } from './RoomScreen'
import styles from './Transcript.module.css'

// Mutable, like the dock's own test: `dockOpen` drives the same narrow/push
// decision in both RoomScreen and EventsDock, and this pins that RoomScreen
// reads it from the store rather than tracking its own copy.
let dockOpen = false

// None of these tests are about the files list — they exercise the
// transcript. Without this, `RoomScreen`'s files effect (see RoomScreen.tsx)
// still fires a real `fetch` on every render here, which this suite has no
// server to answer; it fails, and renders "could not read the file list",
// a line no test below intends. Stubbed to succeed with an empty list, the
// same value Files.test.tsx uses for its own "no files" case.
beforeEach(() => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response('[]', { headers: { 'content-type': 'application/json' } }),
  )
})

const base: Partial<State> = {
  rail: {
    rooms: [
      {
        name: 'protocol',
        members: ['caas', 'hub'],
        lastActivity: 5,
        buckets: [1],
        flag: null,
        hidden: false,
      },
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

const emptyRoomRail = {
  rooms: [
    {
      name: 'protocol',
      members: ['caas'],
      lastActivity: 5,
      buckets: [1],
      flag: null,
      hidden: false,
    },
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
  ],
}

test('an empty room says so in one line, with no call to action', async () => {
  renderWithStore(<RoomScreen />, {
    rail: emptyRoomRail,
    room: 'protocol',
    messages: [],
    roomLoad: 'ready',
  })
  expect(await screen.findByText('Nothing said here yet.')).toBeDefined()
  // Normal, not an error: no dashed box, no button, no instruction.
  expect(screen.queryByRole('button', { name: /send|invite|start/i })).toBeNull()
})

test('a room still loading shows neither the empty line nor a failure', () => {
  renderWithStore(<RoomScreen />, {
    rail: emptyRoomRail,
    room: 'protocol',
    messages: [],
    roomLoad: 'loading',
  })
  expect(screen.queryByText('Nothing said here yet.')).toBeNull()
  expect(screen.queryByText(/could not load the transcript/)).toBeNull()
})

test('a failed load says so, and is not mistaken for an empty room', () => {
  // "Nothing was said" and "we could not find out" are different facts —
  // neither the empty-room line nor a transcript may render for a failed load.
  renderWithStore(<RoomScreen />, {
    rail: emptyRoomRail,
    room: 'protocol',
    messages: [],
    roomLoad: 'failed',
  })
  expect(screen.getByText(/could not load the transcript/)).toBeDefined()
  expect(screen.queryByText('Nothing said here yet.')).toBeNull()
  expect(screen.queryAllByTestId('message-row').length).toBe(0)
})

test('a normal room render shows no files-list failure line', async () => {
  // This suite stubs `fetch` (see the top-level `beforeEach`) precisely so
  // this is a meaningful assertion: unstubbed, every render here attempted a
  // real files request, which always failed in this environment (jsdom has
  // no server to answer `/api/...`) and rendered this same line, once the
  // rejection had a tick to settle — an accident of the test environment,
  // not something any of these tests intended to cover.
  renderScreen()
  await new Promise((resolve) => setTimeout(resolve, 0))
  expect(screen.queryByText(/could not read the file list/)).toBeNull()
})

test('a byline names the host a message came from, humans included', async () => {
  renderWithStore(<RoomScreen />, {
    room: 'protocol',
    roomLoad: 'ready',
    rail: {
      rooms: [],
      agents: [
        {
          name: 'bbaldino',
          host: 'web',
          version: null,
          online: true,
          isHuman: true,
          lastSeen: 0,
          buckets: [],
        },
        {
          name: 'ci-runner',
          host: 'scratch',
          version: null,
          online: true,
          isHuman: false,
          lastSeen: 0,
          buckets: [],
        },
      ],
    },
    messages: [
      {
        id: 1,
        room: 'protocol',
        from: 'bbaldino',
        body: 'hi',
        done: false,
        human: true,
        createdAt: 0,
      },
      {
        id: 2,
        room: 'protocol',
        from: 'ci-runner',
        body: 'yo',
        done: false,
        human: false,
        createdAt: 0,
      },
    ],
  })
  // The human keeps its chip AND gains its host — the two are not alternatives.
  expect(await screen.findByText('bbaldino@web')).toBeDefined()
  expect(screen.getByText('human')).toBeDefined()
  expect(screen.getByText('ci-runner@scratch')).toBeDefined()
})

test('an already-qualified name is not qualified twice', async () => {
  // When the registry disambiguated, `from` is ALREADY `bbaldino@web`. Appending
  // the host again would render `bbaldino@web@web`.
  renderWithStore(<RoomScreen />, {
    room: 'protocol',
    roomLoad: 'ready',
    rail: {
      rooms: [],
      agents: [
        {
          name: 'bbaldino@web',
          host: 'web',
          version: null,
          online: true,
          isHuman: true,
          lastSeen: 0,
          buckets: [],
        },
      ],
    },
    messages: [
      {
        id: 1,
        room: 'protocol',
        from: 'bbaldino@web',
        body: 'hi',
        done: false,
        human: true,
        createdAt: 0,
      },
    ],
  })
  expect(await screen.findByText('bbaldino@web')).toBeDefined()
  expect(screen.queryByText('bbaldino@web@web')).toBeNull()
})

test('a message from an agent no longer in the rail shows its bare name', async () => {
  renderWithStore(<RoomScreen />, {
    room: 'protocol',
    roomLoad: 'ready',
    rail: { rooms: [], agents: [] },
    messages: [
      {
        id: 1,
        room: 'protocol',
        from: 'departed',
        body: 'bye',
        done: false,
        human: false,
        createdAt: 0,
      },
    ],
  })
  expect(await screen.findByText('departed')).toBeDefined()
})

test('the tab bar offers hide for a visible room and unhide for a hidden one', async () => {
  renderWithStore(<RoomScreen />, {
    room: 'protocol',
    roomLoad: 'ready',
    rail: {
      rooms: [
        {
          name: 'protocol',
          members: [],
          lastActivity: null,
          buckets: [],
          flag: null,
          hidden: false,
        },
      ],
      agents: [],
    },
    messages: [],
  })
  expect(await screen.findByRole('button', { name: 'hide' })).toBeDefined()
  expect(screen.queryByRole('button', { name: 'unhide' })).toBeNull()
})

test('clicking hide asks the store to hide this room', async () => {
  renderWithStore(<RoomScreen />, {
    room: 'protocol',
    roomLoad: 'ready',
    rail: {
      rooms: [
        {
          name: 'protocol',
          members: [],
          lastActivity: null,
          buckets: [],
          flag: null,
          hidden: false,
        },
      ],
      agents: [],
    },
    messages: [],
  })
  fireEvent.click(await screen.findByRole('button', { name: 'hide' }))
  expect(storeActions.setHidden).toHaveBeenCalledWith('protocol', true)
})
