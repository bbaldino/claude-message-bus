import { fireEvent, screen } from '@testing-library/react'
import { beforeEach, expect, test, vi } from 'vitest'
import { renderWithStore, storeActions } from '../testing/fakeStore'
import { Composer } from './Composer'

beforeEach(() => {
  localStorage.clear()
  vi.clearAllMocks()
})

test('with no name set, the message box is not usable', () => {
  renderWithStore(<Composer room="protocol" />)
  expect(screen.getByLabelText('send as')).toBeDefined()
  expect(screen.queryByLabelText('message')).toBeNull()
})

test('setting a name reveals the message box and is remembered', () => {
  renderWithStore(<Composer room="protocol" />)
  fireEvent.change(screen.getByLabelText('send as'), { target: { value: 'bbaldino' } })
  fireEvent.submit(screen.getByLabelText('send as'))
  expect(screen.getByLabelText('message')).toBeDefined()
  expect(localStorage.getItem('claude-bus.sendAs')).toBe('bbaldino')
})

test('Enter sends and Shift+Enter does not', () => {
  localStorage.setItem('claude-bus.sendAs', 'bbaldino')
  renderWithStore(<Composer room="protocol" />, { drafts: { protocol: 'hello' } })
  const box = screen.getByLabelText('message')
  fireEvent.keyDown(box, { key: 'Enter', shiftKey: true })
  expect(storeActions.send).not.toHaveBeenCalled()
  fireEvent.keyDown(box, { key: 'Enter' })
  expect(storeActions.send).toHaveBeenCalledWith('protocol', 'hello', false)
})

test('with no name set, there is no message control to reach submit through', () => {
  // Not a test of the `!name` guard in `canSubmit` — that's `canSubmit.test.ts`,
  // which exercises the predicate directly. What this asserts is narrower and
  // literally true: until a name is set, neither the textarea nor the send
  // button is mounted, so there is no control an operator (or a stray keydown)
  // could use to reach `submit` in the first place. Firing `keyDown` at the
  // `send-as` field proves nothing here — it has no keydown handler wired to
  // it, so this would pass whether or not any guard existed, which is exactly
  // the false confidence a previous version of this test gave.
  renderWithStore(<Composer room="protocol" />, { drafts: { protocol: 'hello' } })
  expect(screen.queryByLabelText('message')).toBeNull()
  expect(screen.queryByRole('button', { name: /send/i })).toBeNull()
})

test('Enter does not send an empty or whitespace-only draft', () => {
  localStorage.setItem('claude-bus.sendAs', 'bbaldino')
  renderWithStore(<Composer room="protocol" />, { drafts: { protocol: '   ' } })
  fireEvent.keyDown(screen.getByLabelText('message'), { key: 'Enter' })
  expect(storeActions.send).not.toHaveBeenCalled()
})

test('mark done rides along on the send', () => {
  localStorage.setItem('claude-bus.sendAs', 'bbaldino')
  renderWithStore(<Composer room="protocol" />, { drafts: { protocol: 'settled' } })
  fireEvent.click(screen.getByLabelText('mark done'))
  fireEvent.keyDown(screen.getByLabelText('message'), { key: 'Enter' })
  expect(storeActions.send).toHaveBeenCalledWith('protocol', 'settled', true)
})

test('the delivery preview counts the room members, not every agent', () => {
  localStorage.setItem('claude-bus.sendAs', 'bbaldino')
  renderWithStore(<Composer room="protocol" />, {
    rail: {
      rooms: [
        {
          name: 'protocol',
          members: ['caas', 'ci-runner'],
          lastActivity: null,
          buckets: [],
          flag: null,
          hidden: false,
        },
      ],
      agents: [
        {
          name: 'caas',
          host: 'h',
          version: null,
          online: true,
          isHuman: false,
          lastSeen: 0,
          buckets: [],
        },
        {
          name: 'ci-runner',
          host: 'h',
          version: null,
          online: false,
          isHuman: false,
          lastSeen: 0,
          buckets: [],
        },
        {
          name: 'elsewhere',
          host: 'h',
          version: null,
          online: true,
          isHuman: false,
          lastSeen: 0,
          buckets: [],
        },
      ],
    },
  })
  expect(screen.getByText('delivers to 1, queues for 1')).toBeDefined()
})

// Review finding (Important): `src/bus/commands.rs` auto-joins the sender on
// every send (`commands.rs:202`) and excludes the sender from its own
// fan-out (`commands.rs:225`, `members.iter().filter(|m| m.as_str() != me)`).
// The preview here used to count every rail member, including the operator
// once the 25s rail poll picked up that auto-join — over-stating delivery by
// one. The degenerate case is a room where the operator is the only member:
// the preview read "delivers to 1, queues for 0", promising delivery to
// someone when the bus delivers to nobody. `sendAs` (not the typed `name`)
// is the identity the bus fan-out actually excludes, so that is what must be
// filtered out of the member list before counting.
test('alone in the room, the preview does not count the operator as a recipient', () => {
  localStorage.setItem('claude-bus.sendAs', 'bbaldino')
  renderWithStore(<Composer room="protocol" />, {
    sendAs: 'bbaldino',
    rail: {
      rooms: [
        {
          name: 'protocol',
          members: ['bbaldino'],
          lastActivity: null,
          buckets: [],
          flag: null,
          hidden: false,
        },
      ],
      agents: [
        {
          name: 'bbaldino',
          host: 'web',
          version: null,
          online: true,
          isHuman: true,
          lastSeen: 0,
          buckets: [],
        },
      ],
    },
  })
  expect(screen.getByText('delivers to 0, queues for 0')).toBeDefined()
})
