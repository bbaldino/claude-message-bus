import { fireEvent, screen } from '@testing-library/react'
import { beforeEach, expect, test, vi } from 'vitest'
import { renderWithStore } from '../testing/fakeStore'
import { DeleteModal } from './DeleteModal'

const NAME = 'network-debug#2'

beforeEach(() => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(
      JSON.stringify({
        registration: 1,
        memberships: 2,
        cursors: 0,
        host: 'buildbox',
        online: false,
      }),
      { headers: { 'content-type': 'application/json' } },
    ),
  )
})

test('states the real counts from the preview', async () => {
  renderWithStore(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />)
  expect(await screen.findByText('2')).toBeDefined()
  expect(screen.getByText(/room memberships/)).toBeDefined()
  expect(screen.getByText(/messages and files are kept/)).toBeDefined()
})

test('delete stays disabled until the typed name matches exactly', async () => {
  renderWithStore(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />)
  // No jest-dom matcher is installed in this repo (and this task adds no
  // dependencies), so disabled state is read off the DOM property directly
  // rather than via `.toBeDisabled()`.
  const button = (await screen.findByRole('button', { name: 'delete' })) as HTMLButtonElement
  expect(button.disabled).toBe(true)

  const input = screen.getByTestId('confirm-input')
  // The suffix is the part people get wrong when two agents share a base name.
  fireEvent.change(input, { target: { value: 'network-debug' } })
  expect(button.disabled).toBe(true)

  fireEvent.change(input, { target: { value: NAME } })
  expect(button.disabled).toBe(false)
})

test('shows how many characters remain', async () => {
  renderWithStore(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />)
  await screen.findByTestId('confirm-input')
  fireEvent.change(screen.getByTestId('confirm-input'), { target: { value: 'network-' } })
  expect(screen.getByText(`${NAME.length - 8} characters to go`)).toBeDefined()
})

test('Enter does nothing until the name matches', async () => {
  const onDeleted = vi.fn()
  renderWithStore(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={onDeleted} />)
  const input = await screen.findByTestId('confirm-input')
  fireEvent.change(input, { target: { value: 'network-debug' } })
  fireEvent.keyDown(input, { key: 'Enter' })
  expect(onDeleted).not.toHaveBeenCalled()
})

test('Esc closes', async () => {
  const onClose = vi.fn()
  renderWithStore(<DeleteModal name={NAME} onClose={onClose} onDeleted={vi.fn()} />)
  await screen.findByTestId('confirm-input')
  fireEvent.keyDown(window, { key: 'Escape' })
  expect(onClose).toHaveBeenCalled()
})
