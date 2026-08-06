import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { createLive } from './live'
import type { Connection } from './live'

/// Enough of the browser WebSocket for `live.ts`: it constructs one, assigns
/// handlers, checks `readyState === WebSocket.OPEN` before sending, and closes.
/// Nothing here ever opens on its own — a socket that fails to connect is
/// exactly the case these tests are about.
class FakeSocket {
  static instances: FakeSocket[] = []
  static readonly OPEN = 1
  readyState = 0
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onmessage: ((ev: { data: string }) => void) | null = null
  sent: string[] = []
  constructor(public url: string) {
    FakeSocket.instances.push(this)
  }
  send(data: string) {
    this.sent.push(data)
  }
  close() {}
}

let states: Connection[]

beforeEach(() => {
  vi.useFakeTimers()
  FakeSocket.instances = []
  states = []
  vi.stubGlobal('WebSocket', FakeSocket)
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

function latest() {
  return FakeSocket.instances[FakeSocket.instances.length - 1]
}

/// Drop the current socket and let the scheduled reconnect fire, so the next
/// close sees the grown backoff.
function dropAndRetry() {
  latest().onclose?.()
  vi.advanceTimersByTime(20_000)
}

test('an open socket is live and subscribes to presence and events', () => {
  const live = createLive('ws://x/ws')
  live.on('connection', (p) => states.push(p as Connection))
  live.start()
  latest().readyState = FakeSocket.OPEN
  latest().onopen?.()

  expect(states).toEqual(['live'])
  const types = latest().sent.map((s) => JSON.parse(s).type)
  expect(types).toEqual(['observe', 'watch_presence', 'watch_events'])
})

test('the pill reaches disconnected once the reconnect backoff saturates', () => {
  // The regression: `disconnected` used to be emitted from `onerror`, and a
  // browser always fires `onclose` straight after — which emitted
  // `reconnecting` in the same tick, so red was never observable and the
  // handoff's third connection state did not really exist. Amber must mean
  // "retrying"; red must mean "this has been failing for a while".
  const live = createLive('ws://x/ws')
  live.on('connection', (p) => states.push(p as Connection))
  live.start()

  // 500 → 1000 → 2000 → 4000 → 8000 → 15000: five closes to saturate.
  for (let i = 0; i < 5; i++) dropAndRetry()
  expect(states).toEqual([
    'reconnecting',
    'reconnecting',
    'reconnecting',
    'reconnecting',
    'reconnecting',
  ])

  latest().onclose?.()
  expect(states[states.length - 1]).toBe('disconnected')
})

test('a start after a stop reconnects: the latch does not stay latched forever', () => {
  // React StrictMode double-invokes the mount effect in dev — start(); stop();
  // start() — with nothing to reset the `stopped` flag in between, the second
  // start() was a silent no-op and the socket never opened again.
  const live = createLive('ws://x/ws')
  live.start()
  expect(FakeSocket.instances.length).toBe(1)

  live.stop()
  live.start()

  expect(FakeSocket.instances.length).toBe(2)
})

test('stop kills the retry loop: no socket appears when the old reconnect timer would have fired', () => {
  // The complementary property: fixing the latch reset must not resurrect the
  // reconnect that a `stop()` mid-backoff is supposed to have cancelled.
  const live = createLive('ws://x/ws')
  live.start()
  const before = FakeSocket.instances.length

  // Drop the socket so a reconnect gets scheduled for `backoff` ms out.
  latest().onclose?.()

  live.stop()

  // Advance well past the point the scheduled reconnect would have fired.
  vi.advanceTimersByTime(20_000)

  expect(FakeSocket.instances.length).toBe(before)
})

test('a reconnect that succeeds goes back to live and resets the backoff', () => {
  const live = createLive('ws://x/ws')
  live.on('connection', (p) => states.push(p as Connection))
  live.start()

  for (let i = 0; i < 6; i++) dropAndRetry()
  expect(states).toContain('disconnected')

  latest().readyState = FakeSocket.OPEN
  latest().onopen?.()
  expect(states[states.length - 1]).toBe('live')

  // Backoff reset, so the next failure is amber again rather than staying red.
  latest().onclose?.()
  expect(states[states.length - 1]).toBe('reconnecting')
})

test('switching rooms unwatches the previous one before watching the next', () => {
  const live = createLive('ws://x/ws')
  live.start()
  const sock = latest()
  sock.readyState = FakeSocket.OPEN
  sock.onopen?.()

  live.watchRoom('a')
  live.watchRoom('b')

  // `onopen` already sent observe / watch_presence / watch_events, so filter to
  // the room subscriptions rather than asserting on the whole frame log.
  const frames = sock.sent
    .map((s) => JSON.parse(s) as { type: string; req_id?: number; room?: string })
    .filter((f) => f.type === 'watch' || f.type === 'unwatch')
  expect(frames).toEqual([
    { type: 'watch', req_id: 3, room: 'a' },
    { type: 'unwatch', req_id: 4, room: 'a' },
    { type: 'watch', req_id: 3, room: 'b' },
  ])
})

test('re-selecting the same room does not unwatch it', () => {
  // Re-selecting happens on any re-render that re-drives selection; unwatching
  // and immediately re-watching would drop pushes in the gap.
  const live = createLive('ws://x/ws')
  live.start()
  const sock = latest()
  sock.readyState = FakeSocket.OPEN
  sock.onopen?.()

  live.watchRoom('a')
  live.watchRoom('a')

  const kinds = sock.sent.map((s) => (JSON.parse(s) as { type: string }).type)
  expect(kinds).not.toContain('unwatch')
})
