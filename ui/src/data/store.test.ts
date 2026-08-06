import { beforeEach, expect, test, vi } from 'vitest'
import { createStore } from './store'
import type { RailSummary } from '../types/RailSummary'

const emptyRail: RailSummary = { rooms: [], agents: [] }

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
    start: vi.fn(),
    stop: vi.fn(),
  }
}

let live: ReturnType<typeof fakeLive>

beforeEach(() => {
  live = fakeLive()
})

test('a pushed event lands in the log', () => {
  const store = createStore({ live, fetchRail: async () => emptyRail })
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
  const store = createStore({ live, fetchRail: async () => emptyRail })
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
  const store = createStore({ live, fetchRail: async () => emptyRail })
  live.emit('connection', 'disconnected')
  expect(store.getState().connection).toBe('disconnected')
})

test('a pushed message is normalised into the stored message shape', () => {
  const store = createStore({ live, fetchRail: async () => emptyRail })
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
  const store = createStore({ live, fetchRail: async () => emptyRail })
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

test('subscribers are notified when state changes', () => {
  const store = createStore({ live, fetchRail: async () => emptyRail })
  const seen = vi.fn()
  store.subscribe(seen)
  live.emit('connection', 'reconnecting')
  expect(seen).toHaveBeenCalled()
})
