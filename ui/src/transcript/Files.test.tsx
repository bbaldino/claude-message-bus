import { fireEvent, screen, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import type { ReactElement } from 'react'
import { beforeEach, expect, test, vi } from 'vitest'
import { renderWithStore, setStoreState } from '../testing/fakeStore'
import { RoomScreen } from './RoomScreen'

const rail = {
  rooms: [{ name: 'protocol', members: ['caas'], lastActivity: 5, buckets: [1], flag: null }],
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

const files = [
  {
    key: 'digest-report.json',
    size: 743,
    contentType: 'application/json',
    updatedBy: 'caas',
    updatedAt: 1_700_000_000_000,
  },
]

function mockFiles(payload: unknown, status = 200) {
  return vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
    if (String(input).includes('/files')) {
      return new Response(status === 200 ? JSON.stringify(payload) : null, {
        status,
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response('[]', { headers: { 'content-type': 'application/json' } })
  })
}

const rerenderScreen = (rerender: (ui: ReactElement) => void) =>
  rerender(
    <MemoryRouter>
      <RoomScreen />
    </MemoryRouter>,
  )

beforeEach(() => vi.restoreAllMocks())

test('the count lives in the tab label without opening the tab', async () => {
  mockFiles(files)
  renderWithStore(<RoomScreen />, { rail, room: 'protocol', messages: [] })
  expect(await screen.findByText('files · 1')).toBeDefined()
  // Still on the transcript — the count did not require opening anything.
  expect(screen.queryByTestId('files-table')).toBeNull()
})

test('an empty room announces zero files from the label', async () => {
  mockFiles([])
  renderWithStore(<RoomScreen />, { rail, room: 'protocol', messages: [] })
  expect(await screen.findByText('files · 0')).toBeDefined()
})

test('the table renders key, size, uploader and content type', async () => {
  mockFiles(files)
  renderWithStore(<RoomScreen />, { rail, room: 'protocol', messages: [] })
  fireEvent.click(await screen.findByText('files · 1'))
  // Scoped to the table: the fixture's uploader ('caas') is the same name as
  // the room's only member, who also renders as a header pill labelled
  // 'caas' — a plain `getByText('caas')` is ambiguous between the two, not a
  // defect in the table itself.
  const table = screen.getByTestId('files-table')
  expect(screen.getByText('digest-report.json')).toBeDefined()
  expect(screen.getByText('application/json')).toBeDefined()
  expect(within(table).getByText('caas')).toBeDefined()
  expect(screen.getByText('743 B')).toBeDefined()
})

test('a failed fetch does not render an empty table', async () => {
  // "No files" and "could not find out" are different facts. An empty table for
  // a failed read is the UI asserting something it does not know.
  mockFiles(null, 500)
  renderWithStore(<RoomScreen />, { rail, room: 'protocol', messages: [] })
  expect(await screen.findByText(/could not read the file list/)).toBeDefined()
  expect(screen.queryByTestId('files-table')).toBeNull()
})

test('a failed fetch shows no count rather than zero', async () => {
  // `files · 0` after a failed read would be a lie.
  mockFiles(null, 500)
  renderWithStore(<RoomScreen />, { rail, room: 'protocol', messages: [] })
  expect(await screen.findByText('files')).toBeDefined()
  expect(screen.queryByText('files · 0')).toBeNull()
})

test('an appended message still counts toward unseen when the transcript measures zero', async () => {
  // `classifyArrival`'s 'append' branch is otherwise never exercised in this
  // suite — every other test sets `messages` once per render, which only ever
  // classifies as 'initial'. It also regression-guards the hidden-transcript
  // fix: jsdom reports 0 for scrollTop/scrollHeight/clientHeight
  // unconditionally (see scroll.ts's own comment on this), so every append
  // here runs the exact `el.clientHeight === 0` branch a real hidden files
  // tab would drive. Before that branch counted `unseen` explicitly, an
  // all-zero measurement always resolved 'pin' and never 'notify', so this
  // assertion fails against the pre-fix logic (confirmed by hand before
  // adding this test) even though nothing here is actually hidden.
  mockFiles([])
  const first = {
    id: 1,
    room: 'protocol',
    from: 'caas',
    body: 'first',
    done: true,
    human: false,
    createdAt: 1,
  }
  const { rerender } = renderWithStore(<RoomScreen />, {
    rail,
    room: 'protocol',
    messages: [first],
  })
  await screen.findByText('files · 0')

  setStoreState({
    rail,
    room: 'protocol',
    messages: [
      first,
      {
        id: 2,
        room: 'protocol',
        from: 'caas',
        body: 'second',
        done: true,
        human: false,
        createdAt: 2,
      },
    ],
  })
  rerender(
    <MemoryRouter>
      <RoomScreen />
    </MemoryRouter>,
  )

  expect(await screen.findByText('1 new below')).toBeDefined()
})

test('a file_stored event for the open room refetches the files list', async () => {
  mockFiles([])
  const { rerender } = renderWithStore(<RoomScreen />, {
    rail,
    room: 'protocol',
    messages: [],
    roomLoad: 'ready',
    roomEvents: [],
  })
  await screen.findByText('files · 0')

  mockFiles(files)
  setStoreState({
    rail,
    room: 'protocol',
    messages: [],
    roomLoad: 'ready',
    roomEvents: [
      {
        id: 42,
        kind: 'file_stored',
        agent: 'caas',
        room: 'protocol',
        detail: { key: 'digest-report.json', size: 743, sha256: 'x' },
        createdAt: 2,
      },
    ],
  })
  rerenderScreen(rerender)

  expect(await screen.findByText('files · 1')).toBeDefined()
})

test('an event of another kind for the open room does not refetch the files list', async () => {
  mockFiles(files)
  const { rerender } = renderWithStore(<RoomScreen />, {
    rail,
    room: 'protocol',
    messages: [],
    roomLoad: 'ready',
    roomEvents: [],
  })
  await screen.findByText('files · 1')

  // If this wrongly refetches, the empty payload would flip the count to 0.
  mockFiles([])
  setStoreState({
    rail,
    room: 'protocol',
    messages: [],
    roomLoad: 'ready',
    roomEvents: [
      {
        id: 43,
        kind: 'message_sent',
        agent: 'caas',
        room: 'protocol',
        detail: {},
        createdAt: 3,
      },
    ],
  })
  rerenderScreen(rerender)

  await new Promise((resolve) => setTimeout(resolve, 0))
  expect(screen.getByText('files · 1')).toBeDefined()
})

test('a reconnect repairs the files list after a failed read', async () => {
  mockFiles(null, 500)
  const { rerender } = renderWithStore(<RoomScreen />, {
    rail,
    room: 'protocol',
    messages: [],
    connection: 'live',
  })
  await screen.findByText(/could not read the file list/)

  mockFiles(files)
  setStoreState({ rail, room: 'protocol', messages: [], connection: 'reconnecting' })
  rerenderScreen(rerender)
  setStoreState({ rail, room: 'protocol', messages: [], connection: 'live' })
  rerenderScreen(rerender)

  expect(await screen.findByText('files · 1')).toBeDefined()
  expect(screen.queryByText(/could not read the file list/)).toBeNull()
})

test('a healthy reconnect does not refetch a files list that already succeeded', async () => {
  mockFiles(files)
  const { rerender } = renderWithStore(<RoomScreen />, {
    rail,
    room: 'protocol',
    messages: [],
    connection: 'live',
  })
  await screen.findByText('files · 1')

  // If this wrongly refetches, the failing response would replace the good
  // count with the failure line.
  mockFiles(null, 500)
  setStoreState({ rail, room: 'protocol', messages: [], connection: 'reconnecting' })
  rerenderScreen(rerender)
  setStoreState({ rail, room: 'protocol', messages: [], connection: 'live' })
  rerenderScreen(rerender)

  await new Promise((resolve) => setTimeout(resolve, 0))
  expect(screen.getByText('files · 1')).toBeDefined()
  expect(screen.queryByText(/could not read the file list/)).toBeNull()
})

test('a repair is a no-op when no room is selected', async () => {
  mockFiles([])
  const { rerender } = renderWithStore(<RoomScreen />, {
    rail,
    room: null,
    messages: [],
    connection: 'live',
  })

  setStoreState({ rail, room: null, messages: [], connection: 'reconnecting' })
  rerenderScreen(rerender)
  setStoreState({ rail, room: null, messages: [], connection: 'live' })
  rerenderScreen(rerender)

  await new Promise((resolve) => setTimeout(resolve, 0))
  expect(screen.queryByText(/could not read the file list/)).toBeNull()
})

test('no files request is made when the room is not yet known', async () => {
  // Child effects run before the parent effect that drives `store.selectRoom`
  // off the route param, so on a cold load at a room URL this component can
  // mount with `room` still `null`. `room ?? ''` used to paper over that with
  // a request to `/api/rooms//files`, a guaranteed 404 the `live` latch just
  // discarded — wasteful, not merely silent.
  const fetchSpy = mockFiles([])
  renderWithStore(<RoomScreen />, { rail, room: null, messages: [] })
  await new Promise((resolve) => setTimeout(resolve, 0))
  expect(fetchSpy).not.toHaveBeenCalled()
})
