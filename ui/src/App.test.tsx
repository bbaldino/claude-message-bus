import { render, screen } from '@testing-library/react'
import { expect, test, vi } from 'vitest'
import { App } from './App'

test('renders rooms and agents from the rail', async () => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(
      JSON.stringify({
        rooms: [
          { name: 'protocol', members: ['caas'], lastActivity: 1, buckets: [0, 1], flag: null },
        ],
        agents: [
          {
            name: 'network-debug#2',
            host: 'hardac',
            version: '0.3.3',
            online: false,
            isHuman: false,
            lastSeen: 1,
            buckets: [0],
          },
        ],
      }),
      { headers: { 'content-type': 'application/json' } },
    ),
  )

  render(<App />)

  expect(await screen.findByText(/protocol/)).toBeDefined()
  expect(await screen.findByText(/network-debug#2/)).toBeDefined()
})

test('shows the error rather than an empty console when the rail fails', async () => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('', { status: 500 }))

  render(<App />)

  expect(await screen.findByText(/500/)).toBeDefined()
})
