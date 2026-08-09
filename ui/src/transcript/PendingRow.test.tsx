import { fireEvent, screen } from '@testing-library/react'
import { beforeEach, expect, test, vi } from 'vitest'
import { renderWithStore, storeActions } from '../testing/fakeStore'
import type { PendingSend } from '../data/store'
import { PendingRow } from './PendingRow'

beforeEach(() => {
  vi.clearAllMocks()
})

const sending: PendingSend = {
  clientId: 1,
  room: 'protocol',
  text: 'still going',
  done: false,
  status: 'sending',
  error: null,
}

const failed: PendingSend = {
  clientId: 2,
  room: 'protocol',
  text: 'did not land',
  done: false,
  status: 'failed',
  error: 'storage failed',
}

// Review finding (Important): `PendingRow` had no test coverage at all, and
// it is the SOLE enforcement of a deferred defect's mitigation — discarding
// an in-flight send has no cancellation on the wire, so if a `sending` row
// ever grew retry/discard controls, discarding one could still let its ack
// resurrect the message the operator just threw away. That was accepted only
// because `PendingRow` gates those buttons on the `failed` branch; nothing
// short of a test here stops a future refactor from hoisting them out of the
// `else` and silently reopening the hole.
test('a sending row shows no retry or discard controls', () => {
  renderWithStore(<PendingRow send={sending} />)
  expect(screen.getByText('sending…')).toBeDefined()
  expect(screen.queryByRole('button', { name: /retry/i })).toBeNull()
  expect(screen.queryByRole('button', { name: /discard/i })).toBeNull()
})

test('a failed row shows retry and discard, and they call the store', () => {
  renderWithStore(<PendingRow send={failed} />)
  expect(screen.getByText(/could not send — storage failed/)).toBeDefined()
  fireEvent.click(screen.getByRole('button', { name: /retry/i }))
  expect(storeActions.retry).toHaveBeenCalledWith(2)
  fireEvent.click(screen.getByRole('button', { name: /discard/i }))
  expect(storeActions.discard).toHaveBeenCalledWith(2)
})
