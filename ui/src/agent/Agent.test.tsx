import { screen } from '@testing-library/react'
import { expect, test, vi, beforeEach } from 'vitest'
import { renderWithStore, setStoreState } from '../testing/fakeStore'
import { MemoryRouter } from 'react-router-dom'
import { AgentScreen } from './AgentScreen'

const detail = {
  name: 'release-artifact-verifier#2@buildbox',
  host: 'buildbox',
  cwd: '/home/b/src/claude-bus',
  sessionId: '0f9c1d2e-3a4b-5c6d-7e8f-9a0b1c2d3e4f',
  version: '0.3.3',
  online: false,
  isHuman: false,
  isRelayer: false,
  lastSeen: 1_700_000_000_000,
  buckets: Array(20).fill(0),
  rooms: [],
  events: [],
  eventTotal: 0,
  status: null,
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
  const el = await screen.findByTestId('agent-detail-name')
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
  await screen.findByTestId('agent-detail-name')
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

test('a null agent version is flagged as unknown and differing, not rendered as nothing', async () => {
  // `agent.version === null` is a real signal (a binary predating the version
  // field, per `version_cell` on the old HTML UI) and must say so even though
  // this bus's own version is known — unlike `busVersion === null`, where
  // rendering nothing is correct because there is genuinely nothing to compare.
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
    const url = String(input)
    if (url.includes('/api/meta')) {
      return new Response(JSON.stringify({ host: 'hardac', version: '0.3.3' }), {
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify({ ...detail, version: null }), {
      headers: { 'content-type': 'application/json' },
    })
  })
  renderWithStore(<AgentScreen name="release-artifact-verifier#2@buildbox" />)
  expect(await screen.findByText('unknown')).toBeDefined()
  expect(screen.getByText('differs')).toBeDefined()
  expect(screen.queryByText(/matches bus/)).toBeNull()
})

test('an agent with no rooms gets a stated explanation, not blank space', async () => {
  renderWithStore(<AgentScreen name="release-artifact-verifier#2@buildbox" />)
  expect(await screen.findByText(/Never joined a room/)).toBeDefined()
})

test('the event section header states the true total, not the slice length', async () => {
  // The endpoint caps the list at 50; the header must not report 50 for an
  // agent with 312 events.
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) =>
    String(input).includes('/api/meta')
      ? new Response(JSON.stringify({ host: 'h', version: '0.3.3' }), {
          headers: { 'content-type': 'application/json' },
        })
      : new Response(
          JSON.stringify({
            ...detail,
            events: [{ id: 1, kind: 'agent_registered', detail: {}, createdAt: 1 }],
            eventTotal: 312,
          }),
          { headers: { 'content-type': 'application/json' } },
        ),
  )
  renderWithStore(<AgentScreen name="release-artifact-verifier#2@buildbox" />)
  expect(await screen.findByText('312 total')).toBeDefined()
})

test('a configured relayer is badged in the header', async () => {
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
    const url = String(input)
    const body = url.includes('/api/meta')
      ? { host: 'hardac', version: '0.3.3' }
      : { ...detail, isRelayer: true }
    return new Response(JSON.stringify(body), {
      headers: { 'content-type': 'application/json' },
    })
  })
  renderWithStore(<AgentScreen name="release-artifact-verifier#2@buildbox" />)
  await screen.findByTestId('agent-detail-name')
  expect(screen.getByText('relayer')).toBeDefined()
})

test('an agent that is not a relayer carries no relayer badge', async () => {
  renderWithStore(<AgentScreen name="release-artifact-verifier#2@buildbox" />)
  await screen.findByTestId('agent-detail-name')
  expect(screen.queryByText('relayer')).toBeNull()
})

test('a detail with a status renders the status line', async () => {
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
    const url = String(input)
    if (url.includes('/api/meta')) {
      return new Response(JSON.stringify({ host: 'hardac', version: '0.3.3' }), {
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(
      JSON.stringify({
        ...detail,
        status: {
          state: 'idle',
          since: detail.lastSeen,
          last_heartbeat: null,
          reason: null,
          text: 'wrote anchor.verified.json',
          text_at: null,
          quiet: false,
        },
      }),
      { headers: { 'content-type': 'application/json' } },
    )
  })
  renderWithStore(<AgentScreen name="release-artifact-verifier#2@buildbox" />)
  const el = await screen.findByTestId('agent-detail-status')
  expect(el.textContent).toContain('wrote anchor.verified.json')
})

test('the header status follows the store, without refetching the detail', async () => {
  // The detail is a one-time snapshot; the rail's `status` push is what is live.
  const fetchSpy = vi.spyOn(globalThis, 'fetch')
  const railAgent = (state: 'working' | 'idle', text: string | null) => ({
    name: detail.name,
    host: detail.host,
    version: detail.version,
    online: true,
    isHuman: false,
    isRelayer: false,
    lastSeen: detail.lastSeen,
    buckets: detail.buckets,
    status: {
      state,
      since: detail.lastSeen,
      last_heartbeat: null,
      reason: null,
      text,
      text_at: text === null ? null : detail.lastSeen,
      quiet: false,
    },
  })
  const rail = (a: ReturnType<typeof railAgent>) => ({ rooms: [], agents: [a], relayers: [] })
  const { rerender } = renderWithStore(<AgentScreen name={detail.name} />, {
    rail: rail(railAgent('working', null)),
  })
  const el = await screen.findByTestId('agent-detail-status')
  expect(el.textContent).toMatch(/working/)
  const detailFetches = () =>
    fetchSpy.mock.calls.filter(([u]) => String(u).includes('/api/agents/')).length
  const before = detailFetches()

  setStoreState({ rail: rail(railAgent('idle', 'wrote anchor.verified.json')) })
  rerender(
    <MemoryRouter>
      <AgentScreen name={detail.name} />
    </MemoryRouter>,
  )
  expect(screen.getByTestId('agent-detail-status').textContent).toMatch(
    /idle .*: wrote anchor\.verified\.json/,
  )
  expect(detailFetches()).toBe(before)
})
