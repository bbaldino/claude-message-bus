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
      emit('connection', 'reconnecting')
      setTimeout(open, backoff)
      backoff = Math.min(backoff * 2, 15_000)
    }

    ws.onerror = () => emit('connection', 'disconnected')
  }

  return {
    on(kind: string, fn: Handler) {
      ;(handlers[kind] ??= []).push(fn)
    },
    watchRoom(room: string) {
      watching = room
      send({ type: 'watch', req_id: 3, room })
    },
    start: open,
    stop() {
      stopped = true
      ws?.close()
    },
  }
}
