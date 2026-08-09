import { expect, test, vi } from 'vitest'
import { createParticipant } from './participant'

/// A minimal fake WebSocket. jsdom has no server, so every participant test
/// drives the socket by hand: open it, then push frames through `onmessage`.
class FakeSocket {
  static last: FakeSocket
  onopen: (() => void) | null = null
  onmessage: ((e: { data: string }) => void) | null = null
  onclose: (() => void) | null = null
  readyState = 0
  sent: string[] = []
  constructor(public url: string) {
    FakeSocket.last = this
  }
  send(s: string) {
    this.sent.push(s)
  }
  close() {
    this.readyState = 3
    this.onclose?.()
  }
  openIt() {
    this.readyState = 1
    this.onopen?.()
  }
  push(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) })
  }
  /// Every frame this socket sent, parsed.
  frames() {
    return this.sent.map((s) => JSON.parse(s))
  }
}

function withFakeSocket() {
  vi.stubGlobal('WebSocket', FakeSocket as unknown as typeof WebSocket)
  Object.assign(FakeSocket as unknown as Record<string, number>, { OPEN: 1 })
}

test('register resolves to the name the bus assigned, not the one requested', async () => {
  withFakeSocket()
  const p = createParticipant('ws://x/ws')
  const pending = p.register('bbaldino')
  FakeSocket.last.openIt()
  // The registry qualifies on collision; the console must use what came back.
  FakeSocket.last.push({ type: 'registered', name: 'bbaldino@web#2' })
  expect(await pending).toBe('bbaldino@web#2')

  const reg = FakeSocket.last.frames().find((f) => f.type === 'register')
  expect(reg.name).toBe('bbaldino')
  expect(reg.host).toBe('web')
  expect(reg.human).toBe(true)
  // Both required on the wire — neither field carries #[serde(default)], so a
  // frame missing them fails to deserialize and the bus drops the connection.
  expect(typeof reg.cwd).toBe('string')
  expect(reg).toHaveProperty('session_id')
})

test('a send resolves with the delivery facts from its own ack', async () => {
  withFakeSocket()
  const p = createParticipant('ws://x/ws')
  const reg = p.register('bbaldino')
  FakeSocket.last.openIt()
  FakeSocket.last.push({ type: 'registered', name: 'bbaldino' })
  await reg

  const sending = p.send('protocol', 'hello', false)
  const frame = FakeSocket.last.frames().find((f) => f.type === 'send')
  expect(frame.text).toBe('hello')
  expect(frame.done).toBe(false)
  // Tagged `kind`, field `room` — `src/proto.rs`'s Target, not the `type`/`name`
  // shape every sibling frame uses.
  expect(frame.target).toEqual({ kind: 'room', room: 'protocol' })

  FakeSocket.last.push({
    type: 'reply',
    req_id: frame.req_id,
    // `ReplyResult` is tagged `kind`, not `type` — see `src/proto.rs`'s
    // `#[serde(tag = "kind", ...)]` on `ReplyResult`, confirmed against the
    // generated `ui/src/types/ReplyResult.ts`. This differs from the outer
    // `FromBus` envelope (this `reply` object itself), which is tagged `type`.
    result: { kind: 'sent', room: 'protocol', msg_id: 42, delivered_to: ['caas'], queued_for: [] },
  })
  expect(await sending).toEqual({ ok: true, msgId: 42, deliveredTo: ['caas'], queuedFor: [] })
})

test('two sends in flight are correlated by req_id, not by arrival order', async () => {
  withFakeSocket()
  const p = createParticipant('ws://x/ws')
  const reg = p.register('bbaldino')
  FakeSocket.last.openIt()
  FakeSocket.last.push({ type: 'registered', name: 'bbaldino' })
  await reg

  const first = p.send('protocol', 'one', false)
  const second = p.send('protocol', 'two', false)
  const [f1, f2] = FakeSocket.last.frames().filter((f) => f.type === 'send')
  expect(f1.req_id).not.toBe(f2.req_id)

  // Answer the SECOND one first. Order must not decide which promise settles.
  FakeSocket.last.push({
    type: 'reply',
    req_id: f2.req_id,
    result: { kind: 'sent', room: 'protocol', msg_id: 2, delivered_to: [], queued_for: [] },
  })
  FakeSocket.last.push({
    type: 'reply',
    req_id: f1.req_id,
    result: { kind: 'sent', room: 'protocol', msg_id: 1, delivered_to: [], queued_for: [] },
  })
  expect(await first).toMatchObject({ ok: true, msgId: 1 })
  expect(await second).toMatchObject({ ok: true, msgId: 2 })
})

test('an error frame fails only the send it names', async () => {
  withFakeSocket()
  const p = createParticipant('ws://x/ws')
  const reg = p.register('bbaldino')
  FakeSocket.last.openIt()
  FakeSocket.last.push({ type: 'registered', name: 'bbaldino' })
  await reg

  const sending = p.send('protocol', 'hello', false)
  const frame = FakeSocket.last.frames().find((f) => f.type === 'send')
  FakeSocket.last.push({ type: 'error', req_id: frame.req_id, message: 'storage failed' })
  expect(await sending).toEqual({ ok: false, error: 'storage failed' })
})

test('sending on a closed socket fails loudly instead of vanishing', async () => {
  // This is the whole reason this module does not reuse live.ts's `send`, which
  // silently no-ops when the socket is shut. A dropped subscription is re-sent on
  // reopen; a dropped message is just gone.
  withFakeSocket()
  const p = createParticipant('ws://x/ws')
  const reg = p.register('bbaldino')
  FakeSocket.last.openIt()
  FakeSocket.last.push({ type: 'registered', name: 'bbaldino' })
  await reg

  FakeSocket.last.close()
  const outcome = await p.send('protocol', 'hello', false)
  expect(outcome.ok).toBe(false)
})

test('register rejects instead of hanging when the socket closes before registered arrives', async () => {
  // Critical 1: failAll() only ever drained the `pending` map that send() uses.
  // A close before the bus replies to Register left `registering` untouched, so
  // this promise never settled — the same failure class send() was built to
  // avoid, just not extended to register().
  withFakeSocket()
  const p = createParticipant('ws://x/ws')
  const registerPromise = p.register('bbaldino')
  FakeSocket.last.openIt()
  FakeSocket.last.close()
  await expect(registerPromise).rejects.toThrow()
})

test('a second register() call rejects the first and closes its socket, rather than orphaning both', async () => {
  // Critical 2: `onRegistered`/`registering` is a single shared variable. A
  // second register() before the first settles used to overwrite it outright,
  // leaving the first promise pending forever and the first socket open and
  // unreferenced.
  withFakeSocket()
  const p = createParticipant('ws://x/ws')
  const first = p.register('bbaldino')
  const firstSocket = FakeSocket.last
  firstSocket.openIt()

  const second = p.register('bbaldino2')
  const secondSocket = FakeSocket.last
  expect(secondSocket).not.toBe(firstSocket)

  // The first socket must be closed by the module itself, not merely dropped.
  expect(firstSocket.readyState).toBe(3)

  // A late `registered` on the abandoned first socket must not resolve
  // anything — it was superseded, not merely raced.
  firstSocket.push({ type: 'registered', name: 'bbaldino' })
  await expect(first).rejects.toThrow()

  secondSocket.openIt()
  secondSocket.push({ type: 'registered', name: 'bbaldino2' })
  expect(await second).toBe('bbaldino2')
})
