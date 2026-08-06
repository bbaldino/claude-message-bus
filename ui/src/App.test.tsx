import { render, screen } from '@testing-library/react'
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
