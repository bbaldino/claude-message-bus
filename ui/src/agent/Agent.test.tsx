import { screen } from '@testing-library/react'
import { expect, test, vi, beforeEach } from 'vitest'
import { renderWithStore } from '../testing/fakeStore'
import { AgentScreen } from './AgentScreen'

const detail = {
  name: 'release-artifact-verifier#2@buildbox',
  host: 'buildbox',
  cwd: '/home/b/src/claude-bus',
  sessionId: '0f9c1d2e-3a4b-5c6d-7e8f-9a0b1c2d3e4f',
  version: '0.3.3',
  online: false,
  isHuman: false,
  lastSeen: 1_700_000_000_000,
  buckets: Array(20).fill(0),
  rooms: [],
  events: [],
  eventTotal: 0,
}

beforeEach(() => {
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
    const url = String(input)
    if (url.includes('/api/meta')) {
      return new Response(JSON.stringify({ host: 'hardac', version: '0.3.3' }), {
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify(detail), {
      headers: { 'content-type': 'application/json' },
    })
  })
})

test('the name is rendered in full, not truncated', async () => {
  // 36 characters. You cannot identify an agent from a truncated name, so this
  // wraps rather than ellipsising.
  renderWithStore(<AgentScreen name="release-artifact-verifier#2@buildbox" />)
  const el = await screen.findByTestId('agent-name')
  expect(el.textContent).toBe('release-artifact-verifier#2@buildbox')
  expect(getComputedStyle(el).textOverflow).not.toBe('ellipsis')
})

test('an agent with no activity still shows a volume strip', async () => {
  // A missing chart is indistinguishable from a broken one.
  renderWithStore(<AgentScreen name="release-artifact-verifier#2@buildbox" />)
  expect(await screen.findByLabelText(/no messages in the last 100 min/)).toBeDefined()
})

test('identity lists host, cwd, session, version and last seen', async () => {
  renderWithStore(<AgentScreen name="release-artifact-verifier#2@buildbox" />)
  await screen.findByTestId('agent-name')
  for (const label of ['host', 'cwd', 'session', 'version', 'last seen']) {
    expect(screen.getByText(label)).toBeDefined()
  }
  expect(screen.getByText(detail.sessionId)).toBeDefined()
})

test('a version matching the bus says so; a differing one gets the differs badge', async () => {
  renderWithStore(<AgentScreen name="release-artifact-verifier#2@buildbox" />)
  expect(await screen.findByText(/matches bus/)).toBeDefined()
  expect(screen.queryByText('differs')).toBeNull()
})

test('a version that differs from the bus renders the differs badge, not matches bus', async () => {
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
    const url = String(input)
    if (url.includes('/api/meta')) {
      return new Response(JSON.stringify({ host: 'hardac', version: '0.3.3' }), {
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify({ ...detail, version: '0.2.9' }), {
      headers: { 'content-type': 'application/json' },
    })
  })
  renderWithStore(<AgentScreen name="release-artifact-verifier#2@buildbox" />)
  expect(await screen.findByText('differs')).toBeDefined()
  expect(screen.queryByText(/matches bus/)).toBeNull()
})
