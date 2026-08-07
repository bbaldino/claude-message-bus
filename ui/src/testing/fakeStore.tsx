import { render } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import type { ReactElement } from 'react'
import { vi } from 'vitest'
import type { State } from '../data/store'

/// One shape for every component test. Three different mocking patterns grew
/// across the suite because `useStore` exports a module-level singleton built
/// from the real `createLive`/`fetchRail`; this is the single seam.
export const emptyState: State = {
  rail: null,
  events: [],
  roomEvents: [],
  messages: [],
  room: null,
  connection: 'live',
  dockOpen: false,
  hasMoreHistory: false,
  loadingOlder: false,
}

export const storeActions = {
  selectRoom: vi.fn(),
  loadOlder: vi.fn(),
  setDockOpen: vi.fn(),
  getState: vi.fn(),
  setState: vi.fn(),
  subscribe: vi.fn(),
  start: vi.fn(),
  stop: vi.fn(),
}

let current: State = emptyState

export function setStoreState(patch: Partial<State>) {
  current = { ...emptyState, ...patch }
  storeActions.getState.mockReturnValue(current)
}

vi.mock('../useStore', () => ({
  useStore: () => current,
  store: storeActions,
}))

export function renderWithStore(ui: ReactElement, patch: Partial<State> = {}) {
  setStoreState(patch)
  return render(<MemoryRouter>{ui}</MemoryRouter>)
}
