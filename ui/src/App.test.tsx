import { render, screen } from '@testing-library/react'
import { afterEach, expect, test, vi } from 'vitest'
import { App } from './App'

afterEach(() => {
  vi.restoreAllMocks()
})

test('renders the agents returned by the api', async () => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(
      JSON.stringify([
        {
          name: 'network-debug#2',
          host: 'hardac',
          cwd: '/w/nd',
          sessionId: null,
          online: false,
          isHuman: false,
          version: '0.3.3',
          lastSeen: 1785000000000,
        },
      ]),
      { headers: { 'content-type': 'application/json' } },
    ),
  )

  render(<App />)

  // The suffixed name is the case this whole UI exists to surface.
  expect(await screen.findByText('network-debug#2')).toBeDefined()
  expect(await screen.findByText('hardac')).toBeDefined()
})

test('shows the error rather than an empty page when the api fails', async () => {
  vi.spyOn(globalThis, 'fetch').mockRejectedValue(new Error('connection refused'))

  render(<App />)

  expect(await screen.findByText(/connection refused/)).toBeDefined()
})

test('shows the status when the api returns a non-2xx', async () => {
  // fetch does not reject on a 500 — it resolves with ok: false. Without the
  // explicit check the body would go to JSON.parse and the page would fail
  // somewhere less legible than "the server said 500".
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('', { status: 500 }))

  render(<App />)

  expect(await screen.findByText(/500/)).toBeDefined()
})
