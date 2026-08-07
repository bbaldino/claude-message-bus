/// The participant socket. Separate from the observer socket in `live.ts`
/// because the bus models two different roles: `handle_observer` rejects `Send`
/// outright ("a viewer is not a participant"), while sending auto-joins the
/// sender's room — so one connection would either be unable to send or would
/// make the operator a member of every room the console displays.
///
/// Opened lazily, on the first send, and held for the tab's lifetime. Not
/// reopened per message: that would put an `agent_connected`/`agent_disconnected`
/// pair in the events dock for every message sent.
export type SendOutcome =
  | { ok: true; msgId: number; deliveredTo: string[]; queuedFor: string[] }
  | { ok: false; error: string }

type Pending = (outcome: SendOutcome) => void

export function createParticipant(url: string) {
  let ws: WebSocket | null = null
  let nextReqId = 1
  const pending = new Map<number, Pending>()
  let onRegistered: ((name: string) => void) | null = null

  /// Deliberately NOT `live.ts`'s fire-and-forget send. A subscription frame that
  /// misses is re-sent by `onopen`; a message that misses is lost with no error
  /// and no row in the transcript. Every caller here gets an outcome.
  const sendFrame = (frame: unknown): boolean => {
    if (ws?.readyState !== WebSocket.OPEN) return false
    ws.send(JSON.stringify(frame))
    return true
  }

  const failAll = (error: string) => {
    for (const resolve of pending.values()) resolve({ ok: false, error })
    pending.clear()
  }

  return {
    register(name: string): Promise<string> {
      return new Promise((resolve) => {
        onRegistered = resolve
        ws = new WebSocket(url)
        ws.onopen = () => {
          // `host: 'web'` is what makes `Registry::attach` produce `name@web`
          // when it needs to disambiguate from a CLI session holding the bare
          // name. The browser has no hostname of its own worth sending.
          //
          // `cwd` and `session_id` are NOT optional on the wire: neither carries
          // `#[serde(default)]` in `ToBus::Register`, so omitting them fails
          // deserialization and the bus drops the connection. `session_id` may be
          // null; `cwd` must be a string, and the page's origin is the honest
          // answer for a participant that has no working directory — it is what
          // the agent detail screen will show in the identity list.
          sendFrame({
            type: 'register',
            name,
            host: 'web',
            cwd: location.origin,
            session_id: null,
            human: true,
          })
        }
        ws.onmessage = (ev) => {
          const msg = JSON.parse(ev.data as string) as { type: string } & Record<string, unknown>
          if (msg.type === 'registered') {
            onRegistered?.(msg.name as string)
            onRegistered = null
            return
          }
          if (msg.type === 'reply') {
            const result = msg.result as Record<string, unknown>
            // `ReplyResult` is tagged `kind`, not `type` — see `src/proto.rs`'s
            // `#[serde(tag = "kind", ...)]` on `ReplyResult`, distinct from the
            // outer `FromBus` envelope, which is tagged `type`. Confirmed against
            // the generated `ui/src/types/ReplyResult.ts`.
            if (result?.kind !== 'sent') return
            pending.get(msg.req_id as number)?.({
              ok: true,
              msgId: result.msg_id as number,
              deliveredTo: (result.delivered_to as string[]) ?? [],
              queuedFor: (result.queued_for as string[]) ?? [],
            })
            pending.delete(msg.req_id as number)
            return
          }
          if (msg.type === 'error' && typeof msg.req_id === 'number') {
            pending.get(msg.req_id)?.({
              ok: false,
              error: (msg.message as string) ?? 'send failed',
            })
            pending.delete(msg.req_id)
          }
        }
        // A close with sends in flight must settle them. Leaving them pending
        // would hang the composer in `sending` forever with the operator's text
        // held hostage inside it.
        ws.onclose = () => failAll('connection lost')
      })
    },

    send(room: string, text: string, done: boolean): Promise<SendOutcome> {
      return new Promise((resolve) => {
        const reqId = nextReqId++
        // `Target` is tagged `kind`, not `type`, and its room field is `room`,
        // not `name` — see `src/proto.rs`'s `#[serde(tag = "kind")]`. Verified
        // against that enum rather than inferred from the sibling frames, which
        // all use `type`.
        const frame = {
          type: 'send',
          req_id: reqId,
          target: { kind: 'room', room },
          text,
          done,
        }
        if (!sendFrame(frame)) {
          resolve({ ok: false, error: 'not connected' })
          return
        }
        pending.set(reqId, resolve)
      })
    },

    close() {
      failAll('closed')
      ws?.close()
      ws = null
    },
  }
}
