import { expect, test } from 'vitest'
import { canSubmit } from './canSubmit'

test('false with no name set, even with text present', () => {
  expect(canSubmit(null, 'hello')).toBe(false)
})

test('false with an empty draft', () => {
  expect(canSubmit('bbaldino', '')).toBe(false)
})

test('false with a whitespace-only draft', () => {
  expect(canSubmit('bbaldino', '   ')).toBe(false)
})

test('true with a name set and non-empty text', () => {
  expect(canSubmit('bbaldino', 'hello')).toBe(true)
})
