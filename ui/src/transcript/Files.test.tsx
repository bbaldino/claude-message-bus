import { fireEvent, screen, within } from '@testing-library/react'
import { beforeEach, expect, test, vi } from 'vitest'
import { renderWithStore } from '../testing/fakeStore'
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
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
    if (String(input).includes('/files')) {
      return new Response(status === 200 ? JSON.stringify(payload) : null, {
        status,
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response('[]', { headers: { 'content-type': 'application/json' } })
  })
}

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
