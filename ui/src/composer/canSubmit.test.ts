import { expect, test } from 'vitest'
import { canSubmit } from './canSubmit'

test('false with no name set, even with text present', () => {
  expect(canSubmit(null, 'hello', false)).toBe(false)
})

test('false with an empty draft', () => {
  expect(canSubmit('bbaldino', '', false)).toBe(false)
})

test('false with a whitespace-only draft', () => {
  expect(canSubmit('bbaldino', '   ', false)).toBe(false)
})

test('true with a name set and non-empty text', () => {
  expect(canSubmit('bbaldino', 'hello', false)).toBe(true)
})

// Review finding (Minor): the spec is "the name being set, the text being
// non-empty, and no send already in flight" — the third precondition was
// missing. Not reachable today (`store.send` clears the draft synchronously,
// so a second keydown sees empty text before `canSubmit` is even asked), but
// `canSubmit` is the named home of every precondition now, and should hold
// this one too rather than relying on that incidental clearing.
test('false while a send is already in flight, even with name and text set', () => {
  expect(canSubmit('bbaldino', 'hello', true)).toBe(false)
})
