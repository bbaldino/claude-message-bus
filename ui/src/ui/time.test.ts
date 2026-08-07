import { expect, test } from 'vitest'
import { age } from './time'

test('age renders each unit at its boundary', () => {
  const now = 1_000_000_000
  expect(age(now, now)).toBe('0s')
  expect(age(now - 59_000, now)).toBe('59s')
  expect(age(now - 60_000, now)).toBe('1m')
  expect(age(now - 3_600_000, now)).toBe('1h')
  expect(age(now - 86_400_000, now)).toBe('1d')
})

test('a lastSeen in the future clamps to zero rather than going negative', () => {
  // Clock skew between the bus host and the browser is realistic.
  const now = 1_000_000_000
  expect(age(now + 5_000, now)).toBe('0s')
})
