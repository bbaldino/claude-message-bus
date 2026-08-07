import { expect, test } from 'vitest'
import { classifyArrival, scrollAction, shouldLoadOlder } from './scroll'

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

const msg = (id: number) => ({ id })

test('classifies the first population of a room as initial, not an arrival', () => {
  expect(
    classifyArrival({ prevLastId: null, messages: [msg(1), msg(2)], roomChanged: false }),
  ).toEqual({ kind: 'initial' })
})

test('classifies a room switch as initial even if a previous last id existed', () => {
  expect(
    classifyArrival({ prevLastId: 2, messages: [msg(10), msg(11)], roomChanged: true }),
  ).toEqual({ kind: 'initial' })
})

test('classifies new messages appended at the tail as an append, counting only the new ones', () => {
  expect(
    classifyArrival({
      prevLastId: 2,
      messages: [msg(1), msg(2), msg(3), msg(4)],
      roomChanged: false,
    }),
  ).toEqual({ kind: 'append', count: 2 })
})

test('classifies a page of older messages prepended at the head as none, since the last id is unchanged', () => {
  const older = Array.from({ length: 100 }, (_, i) => msg(-100 + i))
  const messages = [...older, msg(1), msg(2)]
  expect(classifyArrival({ prevLastId: 2, messages, roomChanged: false })).toEqual({
    kind: 'none',
  })
})
