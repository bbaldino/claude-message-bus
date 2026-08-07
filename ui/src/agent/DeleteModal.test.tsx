import { fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, expect, test, vi } from 'vitest'
import { renderWithStore, setStoreState, storeActions } from '../testing/fakeStore'
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
  storeActions.refreshRail.mockClear()
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

test('Enter does not delete when the blast-radius preview failed to load', async () => {
  // The bug this guards: the delete button carries `disabled={!matches ||
  // !preview}`, but if `submit` only checked `matches`, a failed preview left
  // the button disabled while Enter in the input still fired a real DELETE —
  // against an unknown blast radius, no less. `submit` itself must refuse to
  // run without a `preview`, not just the button.
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_i, init) =>
    (init as RequestInit | undefined)?.method === 'DELETE'
      ? new Response(null, { status: 204 })
      : new Response('boom', { status: 500 }),
  )
  const onDeleted = vi.fn()
  renderWithStore(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={onDeleted} />)
  const input = await screen.findByTestId('confirm-input')
  await screen.findByText(/could not read the blast radius/)
  fireEvent.change(input, { target: { value: NAME } })

  const button = screen.getByRole('button', { name: 'delete' }) as HTMLButtonElement
  expect(button.disabled).toBe(true)

  fireEvent.keyDown(input, { key: 'Enter' })
  // Give any wrongly-issued fetch a turn to run.
  await Promise.resolve()
  expect(onDeleted).not.toHaveBeenCalled()
  expect(globalThis.fetch).not.toHaveBeenCalledWith(
    expect.anything(),
    expect.objectContaining({ method: 'DELETE' }),
  )
})

test('Esc closes', async () => {
  const onClose = vi.fn()
  renderWithStore(<DeleteModal name={NAME} onClose={onClose} onDeleted={vi.fn()} />)
  await screen.findByTestId('confirm-input')
  fireEvent.keyDown(window, { key: 'Escape' })
  expect(onClose).toHaveBeenCalled()
})

test('an online agent opens refused, with the mechanism stated', async () => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(
      JSON.stringify({ registration: 1, memberships: 1, cursors: 0, host: 'h', online: true }),
      { headers: { 'content-type': 'application/json' } },
    ),
  )
  renderWithStore(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />)
  expect(await screen.findByText('Still connected')).toBeDefined()
  // The mechanism, not just the rule.
  expect(screen.getByText(/re-register on its next heartbeat/)).toBeDefined()
  expect(screen.queryByTestId('confirm-input')).toBeNull()
})

test('going offline transitions the dialog to confirmable and re-fetches the counts', async () => {
  // The dialog claims "this dialog updates itself" — it must be true, and the
  // counts must be re-read because an agent can change the world on its way out.
  let calls = 0
  vi.spyOn(globalThis, 'fetch').mockImplementation(async () => {
    calls++
    return new Response(
      JSON.stringify({
        registration: 1,
        memberships: 1,
        cursors: 0,
        host: 'h',
        online: calls === 1,
      }),
      { headers: { 'content-type': 'application/json' } },
    )
  })

  const { rerender } = renderWithStore(
    <DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />,
    {
      rail: {
        rooms: [],
        agents: [
          {
            name: NAME,
            host: 'h',
            version: '1',
            online: true,
            isHuman: false,
            lastSeen: 1,
            buckets: [0],
          },
        ],
      },
    },
  )
  expect(await screen.findByText('Still connected')).toBeDefined()

  // Presence says it went offline.
  setStoreState({
    rail: {
      rooms: [],
      agents: [
        {
          name: NAME,
          host: 'h',
          version: '1',
          online: false,
          isHuman: false,
          lastSeen: 1,
          buckets: [0],
        },
      ],
    },
  })
  rerender(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />)

  await waitFor(() => expect(screen.getByTestId('confirm-input')).toBeDefined())
  expect(calls).toBeGreaterThan(1)
})

test('a server refusal renders refused even when the client believed otherwise', async () => {
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_i, init) =>
    (init as RequestInit | undefined)?.method === 'DELETE'
      ? new Response(null, { status: 409 })
      : new Response(
          JSON.stringify({ registration: 1, memberships: 0, cursors: 0, host: 'h', online: false }),
          { headers: { 'content-type': 'application/json' } },
        ),
  )
  renderWithStore(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />)
  const input = await screen.findByTestId('confirm-input')
  fireEvent.change(input, { target: { value: NAME } })
  fireEvent.click(screen.getByRole('button', { name: 'delete' }))
  await waitFor(() => expect(screen.getByText('Still connected')).toBeDefined())
})

test('reconnecting while confirmable re-latches to refused, driven by presence not the stale preview', async () => {
  // The preview read at open said offline and never changes in this test — if
  // the re-latch used `preview.online` instead of live presence, this would
  // stay confirmable forever. It must be `liveNow` that drives it.
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(
      JSON.stringify({ registration: 1, memberships: 1, cursors: 0, host: 'h', online: false }),
      { headers: { 'content-type': 'application/json' } },
    ),
  )

  const { rerender } = renderWithStore(
    <DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />,
    {
      rail: {
        rooms: [],
        agents: [
          {
            name: NAME,
            host: 'h',
            version: '1',
            online: false,
            isHuman: false,
            lastSeen: 1,
            buckets: [0],
          },
        ],
      },
    },
  )
  await screen.findByTestId('confirm-input')

  // Presence says it came back.
  setStoreState({
    rail: {
      rooms: [],
      agents: [
        {
          name: NAME,
          host: 'h',
          version: '1',
          online: true,
          isHuman: false,
          lastSeen: 1,
          buckets: [0],
        },
      ],
    },
  })
  rerender(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />)

  await waitFor(() => expect(screen.getByText('Still connected')).toBeDefined())
  expect(screen.queryByTestId('confirm-input')).toBeNull()
})

test('Esc closes in the refused state too', async () => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(
      JSON.stringify({ registration: 1, memberships: 1, cursors: 0, host: 'h', online: true }),
      { headers: { 'content-type': 'application/json' } },
    ),
  )
  const onClose = vi.fn()
  renderWithStore(<DeleteModal name={NAME} onClose={onClose} onDeleted={vi.fn()} />)
  await screen.findByText('Still connected')
  fireEvent.keyDown(window, { key: 'Escape' })
  expect(onClose).toHaveBeenCalled()
})

test('a successful delete asks the store to refresh the rail', async () => {
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_i, init) =>
    (init as RequestInit | undefined)?.method === 'DELETE'
      ? new Response(null, { status: 204 })
      : new Response(
          JSON.stringify({ registration: 1, memberships: 2, cursors: 0, host: 'h', online: false }),
          { headers: { 'content-type': 'application/json' } },
        ),
  )
  const onDeleted = vi.fn()
  renderWithStore(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={onDeleted} />)
  const input = await screen.findByTestId('confirm-input')
  fireEvent.change(input, { target: { value: NAME } })
  fireEvent.click(screen.getByRole('button', { name: 'delete' }))
  await waitFor(() => expect(onDeleted).toHaveBeenCalled())
  expect(storeActions.refreshRail).toHaveBeenCalledTimes(1)
})

test('a refused delete does not ask for a rail refresh', async () => {
  // A delete that did not happen must not trigger one — the rail has nothing
  // to reflect.
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_i, init) =>
    (init as RequestInit | undefined)?.method === 'DELETE'
      ? new Response(null, { status: 409 })
      : new Response(
          JSON.stringify({ registration: 1, memberships: 0, cursors: 0, host: 'h', online: false }),
          { headers: { 'content-type': 'application/json' } },
        ),
  )
  renderWithStore(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />)
  const input = await screen.findByTestId('confirm-input')
  fireEvent.change(input, { target: { value: NAME } })
  fireEvent.click(screen.getByRole('button', { name: 'delete' }))
  await waitFor(() => expect(screen.getByText('Still connected')).toBeDefined())
  expect(storeActions.refreshRail).not.toHaveBeenCalled()
})

test('a failed delete does not ask for a rail refresh', async () => {
  let deleteCalls = 0
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_i, init) => {
    if ((init as RequestInit | undefined)?.method === 'DELETE') {
      deleteCalls++
      return new Response(null, { status: 500 })
    }
    return new Response(
      JSON.stringify({ registration: 1, memberships: 0, cursors: 0, host: 'h', online: false }),
      { headers: { 'content-type': 'application/json' } },
    )
  })
  renderWithStore(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />)
  const input = await screen.findByTestId('confirm-input')
  fireEvent.change(input, { target: { value: NAME } })
  fireEvent.click(screen.getByRole('button', { name: 'delete' }))
  await waitFor(() => expect(deleteCalls).toBe(1))
  expect(storeActions.refreshRail).not.toHaveBeenCalled()
})

test('a failed delete renders a distinct message from a failed preview', async () => {
  // Submitting requires `preview` to be non-null, so the pre-existing `failed`
  // paragraph — which only renders while `preview` is null — can never show a
  // submit-time failure. The dialog must say something when the console's
  // only irreversible action fails, and it must not reuse the preview
  // failure's wording, since the operator's next move differs (retry reading
  // the blast radius vs. retry the delete, or go look at the bus for a 500).
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_i, init) =>
    (init as RequestInit | undefined)?.method === 'DELETE'
      ? new Response(null, { status: 500 })
      : new Response(
          JSON.stringify({ registration: 1, memberships: 0, cursors: 0, host: 'h', online: false }),
          { headers: { 'content-type': 'application/json' } },
        ),
  )
  renderWithStore(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />)
  const input = await screen.findByTestId('confirm-input')
  fireEvent.change(input, { target: { value: NAME } })
  fireEvent.click(screen.getByRole('button', { name: 'delete' }))

  const message = await screen.findByText(/the delete failed/)
  expect(message.textContent).toContain('500')
  expect(screen.queryByText(/could not read the blast radius/)).toBeNull()
})

test('the delete button is disabled while a delete is already in flight', async () => {
  // Two clicks must not issue two DELETEs — the registry lock on the server
  // serialises them, and the second would find the row already gone.
  let deleteCalls = 0
  let resolveDelete: (() => void) | undefined
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_i, init) => {
    if ((init as RequestInit | undefined)?.method === 'DELETE') {
      deleteCalls++
      await new Promise<void>((resolve) => {
        resolveDelete = resolve
      })
      return new Response(null, { status: 204 })
    }
    return new Response(
      JSON.stringify({ registration: 1, memberships: 0, cursors: 0, host: 'h', online: false }),
      { headers: { 'content-type': 'application/json' } },
    )
  })
  const onDeleted = vi.fn()
  renderWithStore(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={onDeleted} />)
  const input = await screen.findByTestId('confirm-input')
  fireEvent.change(input, { target: { value: NAME } })
  const button = screen.getByRole('button', { name: 'delete' }) as HTMLButtonElement

  fireEvent.click(button)
  await waitFor(() => expect(deleteCalls).toBe(1))
  expect(button.disabled).toBe(true)
  fireEvent.click(button)
  expect(deleteCalls).toBe(1)

  resolveDelete?.()
  await waitFor(() => expect(onDeleted).toHaveBeenCalled())
})

test('the refused state names the real host, not a fabricated one', async () => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(
      JSON.stringify({
        registration: 1,
        memberships: 1,
        cursors: 0,
        host: 'buildbox',
        online: true,
      }),
      { headers: { 'content-type': 'application/json' } },
    ),
  )
  renderWithStore(<DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />)
  await screen.findByText('Still connected')
  expect(screen.getByText(/buildbox/)).toBeDefined()
  expect(screen.queryByText(/hardac/)).toBeNull()
  expect(screen.queryByText(/claude-bus/)).toBeNull()
})

test('mounting inerts the rest of the page, and unmounting restores it', async () => {
  const behindLink = document.createElement('a')
  behindLink.href = '#'
  behindLink.textContent = 'a rail link behind the scrim'
  document.body.appendChild(behindLink)

  const { unmount } = renderWithStore(
    <DeleteModal name={NAME} onClose={vi.fn()} onDeleted={vi.fn()} />,
  )
  await screen.findByTestId('confirm-input')
  // jsdom does not implement the `inert` IDL property (as of the version this
  // repo pins), so the attribute itself is what gets asserted — it is also
  // what a real browser's focus and accessibility-tree handling keys off.
  expect(behindLink.hasAttribute('inert')).toBe(true)

  unmount()
  expect(behindLink.hasAttribute('inert')).toBe(false)
  behindLink.remove()
})
