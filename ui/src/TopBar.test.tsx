import { render, screen } from '@testing-library/react'
import { expect, test, vi } from 'vitest'
import { TopBar } from './TopBar'

function mockStore(connection: string) {
  vi.doMock('./useStore', () => ({
    useStore: () => ({ rail: null, events: [], messages: [], room: null, connection }),
  }))
}

test('the live pill reflects each connection state', async () => {
  for (const [state, label] of [
    ['live', 'live'],
    ['reconnecting', 'reconnecting'],
    ['disconnected', 'disconnected'],
  ] as const) {
    vi.resetModules()
    mockStore(state)
    const { TopBar: Fresh } = await import('./TopBar')
    const { unmount } = render(<Fresh />)
    expect(screen.getByTestId('live-pill').textContent).toBe(label)
    unmount()
  }
})

test('the host pill shows host and version once meta resolves', async () => {
  vi.resetModules()
  mockStore('live')
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(JSON.stringify({ host: 'hardac', version: '0.3.3' }), {
      headers: { 'content-type': 'application/json' },
    }),
  )
  const { TopBar: Fresh } = await import('./TopBar')
  render(<Fresh />)
  expect(await screen.findByText('hardac · 0.3.3')).toBeDefined()
})

test('the search placeholder does not promise message search', () => {
  render(<TopBar />)
  const input = screen.getByPlaceholderText(/search/)
  // There is no search endpoint; rooms and agents filter client-side and message
  // text has no data path. The placeholder must not claim otherwise.
  expect(input.getAttribute('placeholder')).not.toMatch(/message/i)
})
