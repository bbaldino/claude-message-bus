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
  live.emit('presence', { name: 'caas', host: 'h', online: true, lastSeen: 2 })
  expect(store.getState().rail?.agents[0].online).toBe(true)
})

test('a dropped socket surfaces as disconnected', () => {
  const store = createStore({ live, fetchRail: async () => emptyRail })
  live.emit('connection', 'disconnected')
  expect(store.getState().connection).toBe('disconnected')
})

test('a pushed message is normalised into the stored message shape', () => {
  const store = createStore({ live, fetchRail: async () => emptyRail })
  live.emit('message', {
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

test('subscribers are notified when state changes', () => {
  const store = createStore({ live, fetchRail: async () => emptyRail })
  const seen = vi.fn()
  store.subscribe(seen)
  live.emit('connection', 'reconnecting')
  expect(seen).toHaveBeenCalled()
})
