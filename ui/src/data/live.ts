export type Connection = 'live' | 'reconnecting' | 'disconnected'

type Handler = (payload: unknown) => void

/// The observer socket. Identifies with Observe, then subscribes to presence and
/// events; the watched room changes as selection does. The participant socket
/// that sends messages is a separate connection and does not exist yet.
export function createLive(url: string) {
  const handlers: Record<string, Handler[]> = {}
  let ws: WebSocket | null = null
  let watching: string | null = null
  let backoff = 500
  let stopped = false

  /// The reconnect ceiling, and also the threshold for calling the connection
  /// dead: reaching it means ~30s of failed attempts have already gone by.
  const MAX_BACKOFF = 15_000

  const emit = (kind: string, payload: unknown) => {
    for (const h of handlers[kind] ?? []) h(payload)
  }

  const send = (msg: unknown) => ws?.readyState === WebSocket.OPEN && ws.send(JSON.stringify(msg))

  function open() {
    if (stopped) return
    ws = new WebSocket(url)

    ws.onopen = () => {
      backoff = 500
      emit('connection', 'live')
      send({ type: 'observe', name: 'console' })
      send({ type: 'watch_presence', req_id: 1 })
      send({ type: 'watch_events', req_id: 2, room: null })
      if (watching) send({ type: 'watch', req_id: 3, room: watching })
    }

    ws.onmessage = (ev) => {
      const msg = JSON.parse(ev.data as string) as { type: string } & Record<string, unknown>
      if (msg.type === 'presence') emit('presence', msg)
      else if (msg.type === 'event') emit('event', msg)
      else if (msg.type === 'message') emit('message', msg)
    }

    ws.onclose = () => {
      if (stopped) return
      // `disconnected` is entered here, on a saturated backoff, and nowhere else.
      // Emitting it from `onerror` could never make the pill rest on red: a
      // browser always fires `onclose` immediately after `onerror`, so the state
      // was overwritten with `reconnecting` within the same tick every time. Amber
      // means "retrying and it might work"; red has to mean "this has been failing
      // for a while", and the backoff hitting its ceiling is that fact.
      emit('connection', backoff >= MAX_BACKOFF ? 'disconnected' : 'reconnecting')
      setTimeout(open, backoff)
      backoff = Math.min(backoff * 2, MAX_BACKOFF)
    }

    // No `onerror` handler: the close that follows it is what this reacts to.
    // See `onclose`.
  }

  return {
    on(kind: string, fn: Handler) {
      ;(handlers[kind] ??= []).push(fn)
    },
    watchRoom(room: string) {
      // Release the previous room. Without this the observer accumulates every
      // room ever selected: `Registry::watch` only inserts, and the client-side
      // room filter in the store hides the symptom while the subscription set
      // keeps growing.
      if (watching && watching !== room) {
        send({ type: 'unwatch', req_id: 4, room: watching })
      }
      watching = room
      send({ type: 'watch', req_id: 3, room })
    },
    // The counterpart `watchRoom` doesn't have: release the current room without
    // watching anything new. Needed when the operator navigates away from every
    // room (to an agent route, or back to the index) rather than from one room
    // to another — `watchRoom` only ever unwatches as a side effect of watching
    // its replacement.
    unwatchRoom() {
      if (watching) {
        send({ type: 'unwatch', req_id: 4, room: watching })
        watching = null
      }
    },
    // The latch is reset here, in the explicit entry point, and nowhere else.
    // `open()` is also what a scheduled reconnect calls once its backoff
    // elapses — if that call could clear the latch, a `stop()` issued mid-backoff
    // would not reliably stop the retry loop. Only a real `start()` may un-latch.
    start() {
      stopped = false
      open()
    },
    stop() {
      stopped = true
      ws?.close()
    },
  }
}
