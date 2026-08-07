import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, expect, test, vi } from 'vitest'
import { EventsDock } from './EventsDock'

// `dockOpen` lives in the store, so a mocked `useStore` cannot re-render on
// toggle. Rendering is therefore tested by controlling the mocked value, and the
// chord is tested by asserting it calls the store action — testing the toggle
// through a static mock would only assert that the mock is static.
let dockOpen = false
const setDockOpen = vi.fn((open: boolean) => {
  dockOpen = open
})

const state = () => ({
  rail: null,
  messages: [],
  room: 'protocol',
  connection: 'live',
  dockOpen,
  events: [
    { id: 1, kind: 'joined', agent: 'caas', room: 'protocol', detail: {}, createdAt: 1 },
    { id: 2, kind: 'agent_offline', agent: 'hub', room: 'other', detail: {}, createdAt: 2 },
  ],
  roomEvents: [
    { id: 1, kind: 'joined', agent: 'caas', room: 'protocol', detail: {}, createdAt: 1 },
  ],
  hasMoreHistory: false,
  loadingOlder: false,
})

vi.mock('../useStore', () => ({
  useStore: () => state(),
  store: { setDockOpen: (o: boolean) => setDockOpen(o), getState: () => state() },
}))

beforeEach(() => {
  dockOpen = false
  setDockOpen.mockClear()
})

test('renders closed when dockOpen is false', () => {
  render(<EventsDock />)
  expect(screen.getByTestId('dock-closed')).toBeDefined()
  expect(screen.queryByTestId('dock-open')).toBeNull()
})

test('renders open when dockOpen is true', () => {
  dockOpen = true
  render(<EventsDock />)
  expect(screen.getByTestId('dock-open')).toBeDefined()
})

test('the toggle chord asks the store to open it', () => {
  render(<EventsDock />)
  fireEvent.keyDown(window, { key: 'e', ctrlKey: true, metaKey: false })
  expect(setDockOpen).toHaveBeenCalledWith(true)
})

test('the chord is ignored while typing in an input', () => {
  // Otherwise the composer in the next phase cannot type the letter.
  render(
    <>
      <input data-testid="field" />
      <EventsDock />
    </>,
  )
  const field = screen.getByTestId('field')
  field.focus()
  fireEvent.keyDown(field, { key: 'e', ctrlKey: true, bubbles: true })
  expect(setDockOpen).not.toHaveBeenCalled()
})

test('this room scope shows only the room events, whole bus shows all', () => {
  dockOpen = true
  render(<EventsDock />)
  expect(screen.getAllByTestId('event-row')).toHaveLength(1)
  fireEvent.click(screen.getByText('whole bus'))
  expect(screen.getAllByTestId('event-row')).toHaveLength(2)
})

test('the kinds filter offers the kinds actually present, not a hardcoded list', () => {
  dockOpen = true
  render(<EventsDock />)
  fireEvent.click(screen.getByText('whole bus'))
  fireEvent.click(screen.getByTestId('kinds-toggle'))
  expect(screen.getByLabelText('joined')).toBeDefined()
  expect(screen.getByLabelText('agent_offline')).toBeDefined()
})

test('unchecking a kind hides its rows', () => {
  dockOpen = true
  render(<EventsDock />)
  fireEvent.click(screen.getByText('whole bus'))
  fireEvent.click(screen.getByTestId('kinds-toggle'))
  fireEvent.click(screen.getByLabelText('joined'))
  expect(screen.getAllByTestId('event-row')).toHaveLength(1)
})
