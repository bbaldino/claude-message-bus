import { expect, test } from 'vitest'
import type { StatusView } from '../types/StatusView'
import { statusText } from './statusText'

const sv = (over: Partial<StatusView>): StatusView => ({
  state: 'idle',
  since: 0,
  last_heartbeat: null,
  reason: null,
  text: null,
  text_at: null,
  quiet: false,
  ...over,
})

test('matches the Rust renderer for the common cases', () => {
  expect(statusText(sv({ text: 'wrote anchor.verified.json' }), true, 5 * 60_000)).toBe(
    'idle 5m: wrote anchor.verified.json',
  )
  expect(statusText(sv({ state: 'working' }), false, 40 * 60_000)).toBe(
    'offline, was working 40m ago',
  )
  expect(statusText(sv({ state: 'working', quiet: true }), true, 25 * 60_000)).toBe(
    'working? (quiet) 25m',
  )
  expect(
    statusText(sv({ state: 'blocked_on_human', reason: 'permission_prompt' }), true, 120_000),
  ).toBe('blocked on human (permission_prompt) 2m')
})

test('offline and quiet count from the last signal, like the Rust renderer', () => {
  expect(
    statusText(sv({ state: 'working', last_heartbeat: 30 * 60_000 }), false, 40 * 60_000),
  ).toBe('offline, was working 10m ago')
  expect(
    statusText(
      sv({ state: 'working', quiet: true, last_heartbeat: 10 * 60_000 }),
      true,
      25 * 60_000,
    ),
  ).toBe('working? (quiet) 15m')
  expect(statusText(sv({ last_heartbeat: 4 * 60_000 }), true, 5 * 60_000)).toBe('idle 5m')
})
