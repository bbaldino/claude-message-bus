import { beforeEach, expect, test, vi } from 'vitest'
import { createStore } from './store'
import type { RailSummary } from '../types/RailSummary'

const emptyRail: RailSummary = { rooms: [], agents: [] }
const noMessages = async () => []
const noEvents = async () => []

function fakeLive() {
  const handlers: Record<string, (p: unknown) => void> = {}
  return {
    on(kind: string, fn: (p: unknown) => void) {
      handlers[kind] = fn
    },
    emit(kind: string, payload: unknown) {
      handlers[kind]?.(payload)
    },
    watchRoom: vi.fn(),
    unwatchRoom: vi.fn(),
    start: vi.fn(),
    stop: vi.fn(),
  }
}

let live: ReturnType<typeof fakeLive>

beforeEach(() => {
  live = fakeLive()
})

test('a pushed event lands in the log', () => {
  const store = createStore({
    live,
    fetchRail: async () => emptyRail,
    fetchMessages: noMessages,
    fetchEvents: noEvents,
  })
  live.emit('event', {
    type: 'event',
    id: 1,
    kind: 'room_joined',
    agent: 'caas',
    room: 'protocol',
    detail: {},
    created_at: 1,
  })
  expect(store.getState().events[0].kind).toBe('room_joined')
  expect(store.getState().events[0].createdAt).toBe(1)
})

test('a presence push flips an agent online', () => {
  const store = createStore({
    live,
    fetchRail: async () => emptyRail,
    fetchMessages: noMessages,
    fetchEvents: noEvents,
  })
  store.setState({
    rail: {
      rooms: [],
      agents: [
        {
          name: 'caas',
          host: 'h',
          version: null,
          online: false,
          isHuman: false,
          lastSeen: 1,
          buckets: [],
        },
      ],
    },
  })
  // `last_seen`, not `lastSeen`: FromBus is snake_case on the wire because
  // `rename_all` on an enum does not reach variant fields. This fixture is the
  // only place the presence wire shape is pinned, so it has to be the real one.
  live.emit('presence', { type: 'presence', name: 'caas', host: 'h', online: true, last_seen: 2 })
  expect(store.getState().rail?.agents[0].online).toBe(true)
})

test('a dropped socket surfaces as disconnected', () => {
  const store = createStore({
    live,
    fetchRail: async () => emptyRail,
    fetchMessages: noMessages,
    fetchEvents: noEvents,
  })
  live.emit('connection', 'disconnected')
  expect(store.getState().connection).toBe('disconnected')
})

test('a pushed message is normalised into the stored message shape', () => {
  const store = createStore({
    live,
    fetchRail: async () => emptyRail,
    fetchMessages: noMessages,
    fetchEvents: noEvents,
  })
  store.selectRoom('protocol')
  live.emit('message', {
    type: 'message',
    id: 7,
    room: 'protocol',
    from: 'caas',
    text: 'hello',
    done: false,
    human: false,
  })
  const m = store.getState().messages[0]
  expect(m.body).toBe('hello')
  expect(typeof m.createdAt).toBe('number')
})

test('a pushed message for another room never enters the transcript', () => {
  // The socket keeps a `Watch` for every room the operator has visited and the
  // protocol has no `Unwatch`, so pushes for rooms other than the open one keep
  // arriving. They must not land in a transcript that has just been cleared for
  // a different room — they would read as current traffic in the wrong place.
  const store = createStore({
    live,
    fetchRail: async () => emptyRail,
    fetchMessages: noMessages,
    fetchEvents: noEvents,
  })
  store.selectRoom('protocol')
  live.emit('message', {
    type: 'message',
    id: 9,
    room: 'other-room',
    from: 'caas',
    text: 'not here',
    done: false,
    human: false,
  })
  expect(store.getState().messages).toEqual([])
})

test('selecting null clears the room and unwatches, without watching anything new', () => {
  const store = createStore({
    live,
    fetchRail: async () => emptyRail,
    fetchMessages: noMessages,
    fetchEvents: noEvents,
  })
  store.selectRoom('protocol')
  store.selectRoom(null)
  expect(store.getState().room).toBeNull()
  expect(live.unwatchRoom).toHaveBeenCalledOnce()
  // Only the two selectRoom calls above should have touched watchRoom, and
  // only with the real room — clearing must not also call watchRoom.
  expect(live.watchRoom).toHaveBeenCalledExactlyOnceWith('protocol')
})

test('subscribers are notified when state changes', () => {
  const store = createStore({
    live,
    fetchRail: async () => emptyRail,
    fetchMessages: noMessages,
    fetchEvents: noEvents,
  })
  const seen = vi.fn()
  store.subscribe(seen)
  live.emit('connection', 'reconnecting')
  expect(seen).toHaveBeenCalled()
})

test('selecting a room loads its history and its events', async () => {
  const messages = [
    { id: 1, room: 'protocol', from: 'caas', body: 'hi', done: false, human: false, createdAt: 1 },
  ]
  const events = [
    { id: 9, kind: 'message_sent', agent: 'caas', room: 'protocol', detail: {}, createdAt: 1 },
  ]
  const store = createStore({
    live: fakeLive(),
    fetchRail: async () => emptyRail,
    fetchMessages: async () => messages,
    fetchEvents: async () => events,
  })
  await store.selectRoom('protocol')
  expect(store.getState().messages).toEqual(messages)
  expect(store.getState().roomEvents).toEqual(events)
})

test('a live event for the open room lands in roomEvents, one for another room does not', () => {
  const live = fakeLive()
  const store = createStore({
    live,
    fetchRail: async () => emptyRail,
    fetchMessages: async () => [],
    fetchEvents: async () => [],
  })
  store.setState({ room: 'protocol' })
  live.emit('event', {
    type: 'event',
    id: 1,
    kind: 'joined',
    agent: 'caas',
    room: 'protocol',
    detail: {},
    created_at: 1,
  })
  live.emit('event', {
    type: 'event',
    id: 2,
    kind: 'joined',
    agent: 'hub',
    room: 'other',
    detail: {},
    created_at: 2,
  })
  // The global feed takes both — it is the `whole bus` dock scope.
  expect(store.getState().events).toHaveLength(2)
  // The room-scoped list takes only the open room's.
  expect(store.getState().roomEvents.map((e) => e.id)).toEqual([1])
})

test('loadOlder prepends a page and stops when a short page comes back', async () => {
  let call = 0
  const store = createStore({
    live: fakeLive(),
    fetchRail: async () => emptyRail,
    fetchMessages: async (_room, _limit, before) => {
      call++
      if (before === undefined) {
        return Array.from({ length: 100 }, (_, i) => msg(i + 100))
      }
      return [msg(1), msg(2)] // short page — the beginning
    },
    fetchEvents: async () => [],
  })
  await store.selectRoom('protocol')
  expect(store.getState().hasMoreHistory).toBe(true)
  await store.loadOlder()
  expect(store.getState().messages[0].id).toBe(1)
  expect(store.getState().hasMoreHistory).toBe(false)
  expect(call).toBe(2)
})

function msg(id: number) {
  return {
    id,
    room: 'protocol',
    from: 'caas',
    body: `m${id}`,
    done: false,
    human: false,
    createdAt: id,
  }
}

test('an interleaved start()/stop()/start() during the initial fetch leaves no leaked interval', async () => {
  // React StrictMode double-invokes the mount effect in dev: start(); stop();
  // start() — all three synchronous, before the first fetchRail() promise
  // settles. `start()` is async and only assigns `timer` after awaiting
  // fetchRail, so the `stop()` in the middle used to find `timer` still null
  // and its `clearInterval` was a no-op — leaving the first start()'s interval
  // running forever once the second start() overwrote the `timer` variable.
  vi.useFakeTimers()
  try {
    let calls = 0
    const fetchRail = () =>
      new Promise<RailSummary>((resolve) => {
        calls++
        queueMicrotask(() => resolve(emptyRail))
      })
    const store = createStore({ live, fetchRail, fetchMessages: noMessages, fetchEvents: noEvents })

    const first = store.start()
    store.stop()
    const second = store.start()
    await Promise.all([first, second])

    // Only the surviving (second) start() should have installed an interval.
    expect(vi.getTimerCount()).toBe(1)

    calls = 0
    vi.advanceTimersByTime(25_000)
    expect(calls).toBe(1)

    store.stop()
    expect(vi.getTimerCount()).toBe(0)
  } finally {
    vi.useRealTimers()
  }
})
