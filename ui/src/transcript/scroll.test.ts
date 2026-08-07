import { expect, test } from 'vitest'
import { scrollAction, shouldLoadOlder } from './scroll'

const at = (scrollTop: number) => ({ scrollTop, scrollHeight: 1000, clientHeight: 400 })

test('pins when the reader is already at the bottom and messages arrived', () => {
  expect(scrollAction({ ...at(600), grew: true })).toBe('pin')
})

test('notifies instead of yanking when the reader has scrolled up', () => {
  expect(scrollAction({ ...at(100), grew: true })).toBe('notify')
})

test('does nothing when nothing arrived', () => {
  expect(scrollAction({ ...at(600), grew: false })).toBe('none')
  expect(scrollAction({ ...at(100), grew: false })).toBe('none')
})

test('treats near-bottom as bottom, since fractional scroll heights are normal', () => {
  expect(scrollAction({ scrollTop: 598, scrollHeight: 1000, clientHeight: 400, grew: true })).toBe(
    'pin',
  )
})

test('loads older only when near the top', () => {
  expect(shouldLoadOlder(at(0))).toBe(true)
  expect(shouldLoadOlder(at(50))).toBe(true)
  expect(shouldLoadOlder(at(500))).toBe(false)
})
