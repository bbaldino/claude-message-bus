import { expect, test } from 'vitest'
import { age, day, time } from './time'

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

test('time renders a zero-padded 24-hour clock with no am/pm marker', () => {
  // Locale output isn't pinned exactly (the test environment's locale isn't
  // guaranteed), but `hour12: false` rules out an am/pm marker everywhere,
  // and the transcript gutter needs a fixed-width HH:MM shape to stay
  // scannable.
  const ms = new Date(2026, 0, 5, 9, 5).getTime()
  const rendered = time(ms)
  expect(rendered).toMatch(/^\d{2}:\d{2}$/)
  expect(rendered).not.toMatch(/[ap]m/i)
})

test('day renders the day and month without the year', () => {
  const ms = new Date(2026, 0, 5).getTime()
  const rendered = day(ms)
  expect(rendered).toMatch(/5/)
  expect(rendered).not.toMatch(/2026/)
})
