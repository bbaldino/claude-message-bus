import { fireEvent, render, screen, within } from '@testing-library/react'
import { expect, test, vi } from 'vitest'
import { App } from './App'

test('renders the three shell regions and routes to a room', async () => {
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
          { name: 'protocol', members: ['caas'], lastActivity: 1, buckets: [0, 1], flag: null },
        ],
        agents: [],
      }),
      { headers: { 'content-type': 'application/json' } },
    )
  })

  window.history.pushState({}, '', '/app/rooms/protocol')
  render(<App />)

  // The rail is outside the outlet, so it is present on a room route.
  expect(await screen.findByText('protocol')).toBeDefined()
  // The main pane is a labelled placeholder in this phase.
  expect(await screen.findByTestId('main-placeholder')).toBeDefined()
})

test('typing in the top bar search field filters the rail, and clearing it restores everything', async () => {
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
          { name: 'protocol', members: ['caas'], lastActivity: 2, buckets: [0], flag: null },
          { name: 'ops', members: ['caas'], lastActivity: 1, buckets: [0], flag: null },
        ],
        agents: [
          {
            name: 'caas',
            host: 'h',
            version: '0.3.3',
            online: true,
            isHuman: false,
            lastSeen: 1,
            buckets: [0],
          },
          {
            name: 'dashboard',
            host: 'h',
            version: '0.3.3',
            online: false,
            isHuman: false,
            lastSeen: 1,
            buckets: [0],
          },
        ],
      }),
      { headers: { 'content-type': 'application/json' } },
    )
  })

  window.history.pushState({}, '', '/app')
  render(<App />)

  expect(await screen.findByText('protocol')).toBeDefined()
  expect(screen.getByText('ops')).toBeDefined()
  expect(screen.getByText('caas')).toBeDefined()
  expect(screen.getByText('dashboard')).toBeDefined()
  expect(within(screen.getByTestId('agents-header')).getByText('1 of 2 online')).toBeDefined()

  const search = screen.getByPlaceholderText(/search/)
  fireEvent.change(search, { target: { value: 'proto' } })

  expect(screen.getByText('protocol')).toBeDefined()
  expect(screen.queryByText('ops')).toBeNull()
  // Neither agent name contains "proto" — the agents section empties too, and
  // its count must describe that empty set, not the original two.
  expect(screen.queryByText('caas')).toBeNull()
  expect(screen.queryByText('dashboard')).toBeNull()
  expect(within(screen.getByTestId('agents-header')).getByText('0 of 0 online')).toBeDefined()

  fireEvent.change(search, { target: { value: '' } })

  expect(screen.getByText('protocol')).toBeDefined()
  expect(screen.getByText('ops')).toBeDefined()
  expect(screen.getByText('caas')).toBeDefined()
  expect(screen.getByText('dashboard')).toBeDefined()
  expect(within(screen.getByTestId('agents-header')).getByText('1 of 2 online')).toBeDefined()
})
