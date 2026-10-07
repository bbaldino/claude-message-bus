import { expect, test } from 'vitest'
import type { RailRoom } from '../types/RailRoom'
import { RECENT_WINDOW_MS, displayName, shapeInbox } from './inbox'

const NOW = 1_800_000_000_000
const room = (over: Partial<RailRoom> & { name: string }): RailRoom => ({
  members: [],
  lastActivity: NOW - 60_000,
  buckets: [],
  flag: null,
  hidden: false,
  lastMessage: null,
  ...over,
})
const names = (rs: RailRoom[]) => rs.map((r) => r.name)
const shape = (rooms: RailRoom[], agent: string | null = null, query = '') =>
  shapeInbox(rooms, { now: NOW, agent, query })

test('the 24h window is inclusive at exactly 24h, and a room with no messages is older', () => {
  const g = shape([
    room({ name: 'edge', lastActivity: NOW - RECENT_WINDOW_MS }),
    room({ name: 'past', lastActivity: NOW - RECENT_WINDOW_MS - 1 }),
    room({ name: 'silent', lastActivity: null }),
  ])
  expect(names(g.recent)).toEqual(['edge'])
  expect(names(g.older)).toEqual(['past', 'silent'])
})

test('flagged rooms sort first, needs-you above blocked, then by last activity', () => {
  const g = shape([
    room({ name: 'plain-new', lastActivity: NOW - 1_000 }),
    room({
      name: 'blocked',
      lastActivity: NOW - 9_000,
      flag: { kind: 'blocked', queued: 1, waitingOn: ['x'] },
    }),
    room({ name: 'needs', lastActivity: NOW - 8_000, flag: { kind: 'needsYou', exchanges: 20 } }),
    room({ name: 'plain-old', lastActivity: NOW - 5_000 }),
  ])
  expect(names(g.recent)).toEqual(['needs', 'blocked', 'plain-new', 'plain-old'])
})

test('the agent filter keeps only rooms the agent is a member of, in every group', () => {
  const g = shape(
    [
      room({ name: 'a', members: ['hub', 'x'] }),
      room({ name: 'b', members: ['x'] }),
      room({ name: 'c', members: ['hub'], lastActivity: null }),
      room({ name: 'd', members: ['hub'], hidden: true }),
    ],
    'hub',
  )
  expect(names(g.recent)).toEqual(['a'])
  expect(names(g.older)).toEqual(['c'])
  expect(names(g.hidden)).toEqual(['d'])
})

test('search matches a member name as well as the room name, case-insensitively', () => {
  const g = shape(
    [
      room({ name: 'protocol', members: ['caas'] }),
      room({ name: 'other', members: ['Dashboard'] }),
      room({ name: 'nope', members: ['x'] }),
    ],
    null,
    ' DASH ',
  )
  expect(names(g.recent)).toEqual(['other'])
})

test('a hidden room stays hidden even when flagged', () => {
  const g = shape([room({ name: 'h', hidden: true, flag: { kind: 'needsYou', exchanges: 3 } })])
  expect(names(g.hidden)).toEqual(['h'])
  expect(g.recent).toEqual([])
})

test('display names: DMs by their members, rooms with a #, odd names never break', () => {
  expect(displayName(room({ name: 'dm:hub|proxmox' }))).toBe('hub ⇄ proxmox')
  expect(displayName(room({ name: 'dm:a|b|c' }))).toBe('a ⇄ b ⇄ c')
  expect(displayName(room({ name: 'dm:' }))).toBe('dm:')
  expect(displayName(room({ name: 'protocol' }))).toBe('#protocol')
  expect(displayName(room({ name: 'dm:caas|network-debug#2' }))).toBe('caas ⇄ network-debug#2')
})

test('a flagged room stays in recent however old its last message is', () => {
  // A room paused by the exchange cap gets no new messages, so its last activity
  // freezes at the pause; "needs you" lasts until it is resumed. Burying it behind
  // the "older" expander after 24h would hide the one signal the view exists for.
  const old = NOW - 3 * RECENT_WINDOW_MS
  const g = shape([
    room({ name: 'paused', lastActivity: old, flag: { kind: 'needsYou', exchanges: 20 } }),
    room({
      name: 'stuck',
      lastActivity: old,
      flag: { kind: 'blocked', queued: 1, waitingOn: ['x'] },
    }),
    room({ name: 'stale', lastActivity: old }),
  ])
  expect(names(g.recent)).toEqual(['paused', 'stuck'])
  expect(names(g.older)).toEqual(['stale'])
})
