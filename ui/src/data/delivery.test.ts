import { expect, test } from 'vitest'
import { deliveryFor } from './delivery'

const ev = (id: number, kind: string, detail: unknown) => ({
  id,
  kind,
  agent: 'caas',
  room: 'protocol',
  detail,
  createdAt: id,
})

test('correlates message_sent detail by msg_id', () => {
  const m = deliveryFor([
    ev(1, 'message_sent', { msg_id: 42, delivered_to: ['caas'], queued_for: ['hub'] }),
  ])
  expect(m.get(42)).toEqual({ deliveredTo: ['caas'], queuedFor: ['hub'] })
})

test('ignores other event kinds', () => {
  expect(deliveryFor([ev(1, 'joined', { msg_id: 42 })]).size).toBe(0)
})

test('a malformed detail is skipped rather than throwing', () => {
  // `detail` is `unknown` on the wire; a shape change upstream must not take the
  // transcript down with it.
  expect(deliveryFor([ev(1, 'message_sent', null)]).size).toBe(0)
  expect(deliveryFor([ev(2, 'message_sent', { msg_id: 'x' })]).size).toBe(0)
  const m = deliveryFor([ev(3, 'message_sent', { msg_id: 7 })])
  expect(m.get(7)).toEqual({ deliveredTo: [], queuedFor: [] })
})
