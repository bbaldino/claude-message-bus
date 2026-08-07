import { beforeEach, expect, test, vi } from 'vitest'
import { createStore } from './store'
import type { SendOutcome } from './participant'
import { writeSendAs } from '../composer/identity'
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

// Mirrors `fakeLive` above: `register` defaults to resolving with whatever
// name it was asked for (as the real bus does absent a collision), and
// `send` resolves with `sendResult`, which individual tests mutate before
// calling `store.send`/`store.retry` — the same "shared fake, per-test
// override" shape `live.emit` gives the tests for push frames.
function fakeParticipant() {
  const p = {
    sendResult: { ok: true, msgId: 1, deliveredTo: [], queuedFor: [] } as SendOutcome,
    register: vi.fn(async (name: string) => name),
    send: vi.fn(async () => p.sendResult),
    close: vi.fn(),
  }
  return p
}

let live: ReturnType<typeof fakeLive>
let participant: ReturnType<typeof fakeParticipant>

beforeEach(() => {
  live = fakeLive()
  participant = fakeParticipant()
  // `ensureRegistered` reads the operator's chosen name from localStorage
  // (see `composer/identity.ts`); a send with no name set fails with 'no
  // name set' rather than reaching `participant.register` at all, so the
  // send/retry tests need a name on record the same way a real tab would
  // after the operator set one.
  localStorage.clear()
  writeSendAs('bbaldino')
})

// Shared deps for the send/retry/dedup tests below: the same `live` and
// `participant` fakes `beforeEach` resets per test, plus the same no-op
// rail/messages/events fetches every other test in this file already uses.
// Individual tests override what they need to (a local `live`, a custom
// `fetchMessages`, ...) the same way the pre-existing tests below do by
// passing their own object to `createStore` directly.
function makeStore(overrides: Partial<Parameters<typeof createStore>[0]> = {}) {
  return createStore({
    live,
    fetchRail: async () => emptyRail,
    fetchMessages: noMessages,
    fetchEvents: noEvents,
    participant,
    ...overrides,
  })
}

// Flushes a macrotask, not just a microtask — the repair path chains a
// `Promise.all` through a `.then`-equivalent `await`, which outlasts one or
// two bare `Promise.resolve()` ticks. A `setTimeout(0)` reliably drains every
// pending microtask ahead of it.
const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

test('a pushed event lands in the log', () => {
  const store = createStore({
    live,
    participant,
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
    participant,
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
    participant,
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
    participant,
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
    participant,
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
    participant,
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
    participant,
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
    participant,
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
    participant,
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
    participant,
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

test('selecting null while a room load is in flight leaves state cleared, not repopulated', async () => {
  // A promise resolved manually from the test, rather than fake timers, makes
  // the race explicit: the `protocol` fetch is still pending when `null` is
  // selected, and only resolves afterwards.
  let resolveMessages: (messages: ReturnType<typeof msg>[]) => void = () => {}
  const store = createStore({
    live: fakeLive(),
    participant,
    fetchRail: async () => emptyRail,
    fetchMessages: () =>
      new Promise<ReturnType<typeof msg>[]>((resolve) => {
        resolveMessages = resolve
      }),
    fetchEvents: async () => [],
  })
  const loading = store.selectRoom('protocol')
  await store.selectRoom(null)
  resolveMessages([msg(1)])
  await loading
  expect(store.getState().room).toBeNull()
  expect(store.getState().messages).toEqual([])
  expect(store.getState().roomEvents).toEqual([])
  // The stale 'protocol' load resolving after the `null` deselection must not
  // stamp 'ready' over it — `null` has no room to have loaded, so this stays
  // at the 'loading' the synchronous part of the `null` selection set.
  expect(store.getState().roomLoad).toBe('loading')
})

test('a room load that rejects after a newer selection has landed does not stamp failed over it', async () => {
  // Mirrors the race above, but for the failure path: `protocol`'s fetch is
  // still pending — and will reject — when `other` is selected and finishes
  // first. The stale rejection landing afterwards must not overwrite `other`'s
  // now-settled 'ready' state with 'failed'.
  let rejectProtocol: (err: unknown) => void = () => {}
  const store = createStore({
    live: fakeLive(),
    participant,
    fetchRail: async () => emptyRail,
    fetchMessages: async (room) => {
      if (room === 'protocol') {
        return new Promise<ReturnType<typeof msg>[]>((_resolve, reject) => {
          rejectProtocol = reject
        })
      }
      return [msg(1)]
    },
    fetchEvents: async () => [],
  })
  const failing = store.selectRoom('protocol')
  await store.selectRoom('other')
  rejectProtocol(new Error('boom'))
  await failing
  expect(store.getState().room).toBe('other')
  expect(store.getState().roomLoad).toBe('ready')
  expect(store.getState().messages).toEqual([msg(1)])
})

test('loadOlder in flight when the room changes clears loadingOlder and a later loadOlder for the new room still fetches', async () => {
  let resolveProtocolOlder: (messages: ReturnType<typeof msg>[]) => void = () => {}
  let otherOlderCalls = 0
  const otherMsg = (id: number) => ({
    id,
    room: 'other',
    from: 'caas',
    body: `o${id}`,
    done: false,
    human: false,
    createdAt: id,
  })
  const store = createStore({
    live: fakeLive(),
    participant,
    fetchRail: async () => emptyRail,
    fetchMessages: async (room, _limit, before) => {
      if (room === 'protocol') {
        if (before === undefined) {
          return Array.from({ length: 100 }, (_, i) => msg(i + 100))
        }
        return new Promise<ReturnType<typeof msg>[]>((resolve) => {
          resolveProtocolOlder = resolve
        })
      }
      // room === 'other'
      if (before === undefined) {
        return Array.from({ length: 100 }, (_, i) => otherMsg(i + 1000))
      }
      otherOlderCalls++
      return [otherMsg(1), otherMsg(2)]
    },
    fetchEvents: async () => [],
  })

  await store.selectRoom('protocol')
  const olderLoad = store.loadOlder() // starts the slow protocol fetch; sets loadingOlder true
  await store.selectRoom('other') // switches rooms while the protocol fetch is still pending
  resolveProtocolOlder([msg(1), msg(2)]) // let the stale fetch resolve after the switch
  await olderLoad

  expect(store.getState().loadingOlder).toBe(false)
  expect(store.getState().room).toBe('other')

  await store.loadOlder()
  expect(otherOlderCalls).toBe(1)
})

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
    const store = createStore({
      live,
      fetchRail,
      fetchMessages: noMessages,
      fetchEvents: noEvents,
      participant,
    })

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

test('a reconnect after a failed room load repairs the transcript', async () => {
  // First call fails (the bus is down), second call (the repair) succeeds.
  let call = 0
  const messages = [msg(1)]
  const events = [
    { id: 9, kind: 'message_sent', agent: 'caas', room: 'protocol', detail: {}, createdAt: 1 },
  ]
  const store = createStore({
    live,
    participant,
    fetchRail: async () => emptyRail,
    fetchMessages: async () => {
      call++
      if (call === 1) throw new Error('boom')
      return messages
    },
    fetchEvents: async () => events,
  })
  await store.selectRoom('protocol')
  expect(store.getState().roomLoad).toBe('failed')

  // A reconnect: the connection transitions into 'live'.
  live.emit('connection', 'reconnecting')
  live.emit('connection', 'live')
  // The repair fetch is async; let it settle.
  await flush()

  expect(store.getState().roomLoad).toBe('ready')
  expect(store.getState().messages).toEqual(messages)
  expect(store.getState().roomEvents).toEqual(events)
})

test('a healthy reconnect does not refetch a transcript that already loaded', async () => {
  let call = 0
  const store = createStore({
    live,
    participant,
    fetchRail: async () => emptyRail,
    fetchMessages: async () => {
      call++
      return [msg(1)]
    },
    fetchEvents: async () => [],
  })
  await store.selectRoom('protocol')
  expect(call).toBe(1)
  expect(store.getState().roomLoad).toBe('ready')

  live.emit('connection', 'reconnecting')
  live.emit('connection', 'live')
  await flush()

  // The transcript already loaded successfully — a reconnect must not throw
  // it away and refetch.
  expect(call).toBe(1)
})

test('a reconnect repair is a no-op when no room is selected', async () => {
  let call = 0
  const store = createStore({
    live,
    participant,
    fetchRail: async () => emptyRail,
    fetchMessages: async () => {
      call++
      return []
    },
    fetchEvents: async () => [],
  })
  live.emit('connection', 'reconnecting')
  live.emit('connection', 'live')
  await flush()
  expect(call).toBe(0)
  expect(store.getState().room).toBeNull()
})

test('re-rendering at live (no transition) does not repair anything', async () => {
  let call = 0
  const store = createStore({
    live,
    participant,
    fetchRail: async () => emptyRail,
    fetchMessages: async () => {
      call++
      if (call === 1) throw new Error('boom')
      return [msg(1)]
    },
    fetchEvents: async () => [],
  })
  await store.selectRoom('protocol')
  expect(store.getState().roomLoad).toBe('failed')
  expect(call).toBe(1)

  // Already 'live' from store creation's default state ('reconnecting')...
  // emit 'live' once to actually transition, then emit 'live' again with no
  // change in between — the second emit is a re-render at the same value,
  // not a transition, and must not trigger a second repair on top of the
  // first (which is still in flight when the second 'live' lands, since no
  // await has happened yet).
  live.emit('connection', 'live')
  live.emit('connection', 'live')
  await flush()

  expect(call).toBe(2)
  expect(store.getState().roomLoad).toBe('ready')
})

test('a reconnect repair racing a newer room switch does not overwrite it', async () => {
  // 'protocol' failed to load. A reconnect fires the repair fetch for
  // 'protocol', but before it resolves the operator switches to 'other'.
  // The stale repair landing afterwards must not stamp over 'other'.
  let resolveRepair: (messages: ReturnType<typeof msg>[]) => void = () => {}
  let protocolCalls = 0
  const store = createStore({
    live,
    participant,
    fetchRail: async () => emptyRail,
    fetchMessages: async (room) => {
      if (room === 'protocol') {
        protocolCalls++
        if (protocolCalls === 1) throw new Error('boom')
        return new Promise<ReturnType<typeof msg>[]>((resolve) => {
          resolveRepair = resolve
        })
      }
      return [msg(2)]
    },
    fetchEvents: async () => [],
  })
  await store.selectRoom('protocol')
  expect(store.getState().roomLoad).toBe('failed')

  live.emit('connection', 'reconnecting')
  live.emit('connection', 'live')
  await flush() // let the repair's fetch call happen (still pending)

  await store.selectRoom('other')
  resolveRepair([msg(1)])
  await flush()

  expect(store.getState().room).toBe('other')
  expect(store.getState().messages).toEqual([msg(2)])
  expect(store.getState().roomLoad).toBe('ready')
})

test('a message already in the transcript is not appended twice', () => {
  // Two sockets now receive the same message: the observer because it watches the
  // room, and the participant because sending joined it. Without this the
  // operator's own message renders twice.
  const store = makeStore()
  store.selectRoom('protocol')
  const frame = {
    type: 'message',
    id: 7,
    room: 'protocol',
    from: 'caas',
    text: 'hi',
    done: false,
    human: false,
  }
  live.emit('message', frame)
  live.emit('message', frame)
  expect(store.getState().messages.filter((m) => m.id === 7)).toHaveLength(1)
})

test('a draft survives leaving the room and coming back', () => {
  const store = makeStore()
  store.setDraft('protocol', 'half a thought')
  store.selectRoom('other')
  store.selectRoom('protocol')
  expect(store.getState().drafts.protocol).toBe('half a thought')
})

test('a successful send promotes its pending row into the transcript', async () => {
  const store = makeStore()
  store.selectRoom('protocol')
  participant.sendResult = { ok: true, msgId: 42, deliveredTo: [], queuedFor: [] }
  await store.send('protocol', 'hello', false)
  expect(store.getState().pending).toHaveLength(0)
  expect(store.getState().messages.at(-1)).toMatchObject({ id: 42, body: 'hello' })
})

test('the observer copy of a just-sent message does not duplicate it', async () => {
  const store = makeStore()
  store.selectRoom('protocol')
  participant.sendResult = { ok: true, msgId: 42, deliveredTo: [], queuedFor: [] }
  await store.send('protocol', 'hello', false)
  live.emit('message', {
    type: 'message',
    id: 42,
    room: 'protocol',
    from: 'bbaldino',
    text: 'hello',
    done: false,
    human: true,
  })
  expect(store.getState().messages.filter((m) => m.id === 42)).toHaveLength(1)
})

test('a failed send keeps the text and leaves a failed row, not a phantom message', async () => {
  const store = makeStore()
  store.selectRoom('protocol')
  participant.sendResult = { ok: false, error: 'storage failed' }
  await store.send('protocol', 'hello', false)
  // The message must NOT be in the transcript — it does not exist on the bus.
  expect(store.getState().messages.some((m) => m.body === 'hello')).toBe(false)
  const failed = store.getState().pending.at(-1)
  expect(failed).toMatchObject({ status: 'failed', text: 'hello', error: 'storage failed' })
})

test('discarding a failed send removes it', async () => {
  const store = makeStore()
  store.selectRoom('protocol')
  participant.sendResult = { ok: false, error: 'nope' }
  await store.send('protocol', 'hello', false)
  const id = store.getState().pending[0].clientId
  store.discard(id)
  expect(store.getState().pending).toHaveLength(0)
})

// Correction to the brief: `register()` (see `data/participant.ts`) rejects
// rather than hanging when the socket closes before the bus answers
// `registered`. Without a guard in `settle` around that await, this
// rejection propagates out of `send()` entirely, and the pending row it
// left behind is never updated — stuck 'sending' forever, with the
// operator's text trapped in a row that offers no retry and no discard.
// That is the exact failure `register()`'s rejection was introduced to
// prevent, reintroduced one layer up. This test fails without the guard:
// confirmed by temporarily removing the `try`/`catch` around
// `ensureRegistered()` in `settle` and re-running — the unhandled rejection
// surfaces as a failed `await store.send(...)`, and the row is left behind
// with `status: 'sending'` instead of `'failed'`.
test('a failed registration marks the pending row failed, keeping the text, not stuck sending', async () => {
  const store = makeStore()
  store.selectRoom('protocol')
  participant.register = vi.fn(async () => {
    throw new Error('connection lost')
  })
  await store.send('protocol', 'hello', false)
  expect(store.getState().messages.some((m) => m.body === 'hello')).toBe(false)
  const row = store.getState().pending.at(-1)
  expect(row).toMatchObject({ status: 'failed', text: 'hello', error: 'connection lost' })
})
