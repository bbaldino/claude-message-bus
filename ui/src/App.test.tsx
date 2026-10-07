import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, expect, test, vi } from 'vitest'
import { App } from './App'
import { store } from './useStore'

function mockEmptyRail() {
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
    const url = String(input)
    if (url.includes('/api/meta')) {
      return new Response(JSON.stringify({ host: 'hardac', version: '0.3.3' }), {
        headers: { 'content-type': 'application/json' },
      })
    }
    // The room screen fetches messages and events on selection; these must stay
    // arrays even in an otherwise-empty mock, or `deliveryFor` chokes on the
    // rail's `{ rooms, agents }` shape landing in `roomEvents`.
    if (url.includes('/messages') || url.includes('/api/events')) {
      return new Response(JSON.stringify([]), {
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify({ rooms: [], agents: [] }), {
      headers: { 'content-type': 'application/json' },
    })
  })
}

afterEach(() => {
  // store is the app-wide singleton (see useStore.ts) — a stopped socket and
  // interval from one test must not keep running into the next.
  store.stop()
  vi.restoreAllMocks()
})

test('renders the three shell regions and routes to a room', async () => {
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
    const url = String(input)
    if (url.includes('/api/meta')) {
      return new Response(JSON.stringify({ host: 'hardac', version: '0.3.3' }), {
        headers: { 'content-type': 'application/json' },
      })
    }
    if (url.includes('/messages') || url.includes('/api/events')) {
      return new Response(JSON.stringify([]), {
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(
      JSON.stringify({
        rooms: [
          { name: 'protocol', members: ['caas'], lastActivity: 1, buckets: [0, 1], flag: null },
        ],
        agents: [],
      }),
      { headers: { 'content-type': 'application/json' } },
    )
  })

  window.history.pushState({}, '', '/rooms/protocol')
  render(<App />)

  // The rail (now the agent list) is outside the outlet, so it is present on a
  // room route too.
  expect(await screen.findByTestId('agents-header')).toBeDefined()
  // The main pane now renders the real room screen, not the placeholder.
  expect(await screen.findByRole('heading', { name: 'protocol' })).toBeDefined()
})

test('typing in the top bar search field filters both the agents and the inbox', async () => {
  const now = Date.now()
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
    const url = String(input)
    if (url.includes('/api/meta')) {
      return new Response(JSON.stringify({ host: 'hardac', version: '0.3.3' }), {
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(
      JSON.stringify({
        rooms: [
          {
            name: 'protocol',
            members: ['caas'],
            lastActivity: now - 2_000,
            buckets: [0],
            flag: null,
            hidden: false,
            lastMessage: null,
          },
          {
            name: 'ops',
            members: ['dashboard'],
            lastActivity: now - 1_000,
            buckets: [0],
            flag: null,
            hidden: false,
            lastMessage: null,
          },
        ],
        agents: [
          {
            name: 'caas',
            host: 'h',
            version: '0.3.3',
            online: true,
            isHuman: false,
            isRelayer: false,
            lastSeen: 1,
            buckets: [0],
            status: null,
          },
          {
            name: 'dashboard',
            host: 'h',
            version: '0.3.3',
            online: false,
            isHuman: false,
            isRelayer: false,
            lastSeen: 1,
            buckets: [0],
            status: null,
          },
        ],
        relayers: [],
      }),
      { headers: { 'content-type': 'application/json' } },
    )
  })

  window.history.pushState({}, '', '/')
  render(<App />)

  // Inbox (center) and agents (rail), unfiltered. The offline agent sits in the
  // collapsed group; the header still counts it.
  expect(await screen.findByText('#protocol')).toBeDefined()
  expect(screen.getByText('#ops')).toBeDefined()
  expect(screen.getByText('caas')).toBeDefined()
  expect(within(screen.getByTestId('agents-header')).getByText('1 of 2 online')).toBeDefined()

  const search = screen.getByPlaceholderText(/search/)

  // A member name narrows the inbox to that agent's rooms and the rail to it.
  fireEvent.change(search, { target: { value: 'dash' } })
  expect(screen.getByText('#ops')).toBeDefined()
  expect(screen.queryByText('#protocol')).toBeNull()
  expect(screen.getByText('dashboard')).toBeDefined() // search expands the offline group
  expect(screen.queryByText('caas')).toBeNull()

  // A room name matches no agent: the rail says so, the inbox keeps the room.
  fireEvent.change(search, { target: { value: 'proto' } })
  expect(screen.getByText('#protocol')).toBeDefined()
  expect(screen.queryByText('#ops')).toBeNull()
  expect(screen.getByText('nothing matched "proto"')).toBeDefined()

  fireEvent.change(search, { target: { value: '' } })
  expect(screen.getByText('#protocol')).toBeDefined()
  expect(screen.getByText('#ops')).toBeDefined()
  expect(screen.getByText('caas')).toBeDefined()
  expect(within(screen.getByTestId('agents-header')).getByText('1 of 2 online')).toBeDefined()
})

test('rendering at a room route tells the store to select that room', async () => {
  // This is the wiring the whole phase rests on: nothing else drives
  // store.selectRoom in the running app, so without it the store's `room`
  // stays null forever and the console never actually watches anything.
  mockEmptyRail()
  const selectRoom = vi.spyOn(store, 'selectRoom')

  window.history.pushState({}, '', '/rooms/protocol')
  render(<App />)

  // The room screen replaced the placeholder, so wait on the wiring itself
  // rather than on a testid that no longer exists for a room route.
  await waitFor(() => expect(selectRoom).toHaveBeenCalledWith('protocol'))
})

test('a room name that needs URL encoding reaches the store decoded', async () => {
  mockEmptyRail()
  const selectRoom = vi.spyOn(store, 'selectRoom')

  const roomName = 'dm:caas|network-debug#2'
  window.history.pushState({}, '', `/rooms/${encodeURIComponent(roomName)}`)
  render(<App />)

  // The route param comes through useMatch already decoded — selectRoom must
  // see the real room name, not the percent-encoded form that was in the URL.
  await waitFor(() => expect(selectRoom).toHaveBeenCalledWith(roomName))
})
