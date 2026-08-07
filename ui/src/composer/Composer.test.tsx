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

test('Enter cannot send with no name set, even though the box is hidden', () => {
  // THE GUARD LIVES IN THE ACTION, NOT ON THE CONTROL. This is the exact shape of
  // the delete modal's Critical: the button was correctly disabled while Enter
  // called submit() directly, and submit() carried only half the guard.
  renderWithStore(<Composer room="protocol" />, { drafts: { protocol: 'hello' } })
  fireEvent.keyDown(screen.getByLabelText('send as'), { key: 'Enter' })
  expect(storeActions.send).not.toHaveBeenCalled()
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
