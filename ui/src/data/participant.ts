import type { FromBus } from '../types/FromBus'

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

type RegistrationOutcome = { ok: true; name: string } | { ok: false; error: string }
type RegistrationSettle = (outcome: RegistrationOutcome) => void

export function createParticipant(url: string) {
  let ws: WebSocket | null = null
  let nextReqId = 1
  const pending = new Map<number, Pending>()
  // The settler for a `register()` call still waiting on `registered` (or on
  // the connection failing first). Cleared the moment it's used, exactly like
  // `pending`'s entries — a second `register()` before the first settles must
  // not leave this pointing at an already-resolved promise's resolver.
  let registering: RegistrationSettle | null = null

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

  /// Detach and close whatever socket currently exists, and settle everything
  /// it owed an answer to: sends in flight, and a registration still waiting
  /// on `registered`. Used both by a second `register()` call — the earlier
  /// caller must not be orphaned and the earlier socket must not be leaked —
  /// and by `close()`.
  ///
  /// Handlers are detached before `.close()` so a socket already mid-abandonment
  /// can't turn around and fire a late `onmessage`/`onclose` against state that
  /// has moved on to a newer connection.
  const abandon = (reason: string) => {
    failAll(reason)
    registering?.({ ok: false, error: reason })
    registering = null
    if (ws) {
      ws.onopen = null
      ws.onmessage = null
      ws.onclose = null
      ws.close()
    }
    ws = null
  }

  return {
    register(name: string): Promise<string> {
      // A registration (or a bare socket) already in flight is abandoned
      // rather than silently overwritten by `ws = new WebSocket(url)`: without
      // this, `registering` — a single shared variable — is overwritten too,
      // so the earlier caller's promise never settles, and the earlier `ws` is
      // discarded without being closed.
      abandon('superseded by a new register() call')
      return new Promise((resolve, reject) => {
        registering = (outcome) => {
          if (outcome.ok) resolve(outcome.name)
          else reject(new Error(outcome.error))
        }
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
          // Narrowed against `FromBus`, the union ts-rs generates from
          // `src/proto.rs`, the same way `store.ts` does it: the single `as
          // FromBus` sits at the `unknown` boundary where the JSON arrives, and
          // every field access after the `type`/`kind` check is compiler-verified
          // against the server's own definition. A hand-written literal (this
          // file's previous shape) agrees with whatever you wrote, which is
          // exactly how the `kind`-vs-`type` `ReplyResult` bug shipped and was
          // caught only by manually cross-referencing the Rust — narrowed against
          // the generated type, it would have been a `tsc` failure instead.
          const frame = JSON.parse(ev.data as string) as FromBus
          if (frame.type === 'registered') {
            registering?.({ ok: true, name: frame.name })
            registering = null
            return
          }
          if (frame.type === 'reply') {
            if (frame.result.kind !== 'sent') return
            pending.get(frame.req_id)?.({
              ok: true,
              msgId: frame.result.msg_id,
              deliveredTo: frame.result.delivered_to,
              queuedFor: frame.result.queued_for,
            })
            pending.delete(frame.req_id)
            return
          }
          if (frame.type === 'error' && frame.req_id !== null) {
            pending.get(frame.req_id)?.({ ok: false, error: frame.message })
            pending.delete(frame.req_id)
          }
        }
        // A close must settle everything the closed socket owed an answer to:
        // sends in flight (see `sendFrame`'s comment above) — and, just as much,
        // a registration that hasn't heard back yet. That second half used to be
        // missing: `register()`, then a close before `registered` arrived, hung
        // the returned promise forever. `register()` rejects on failure (with
        // `Promise<string>` kept as the resolved shape, per the module's public
        // interface) so a caller can tell success from failure with a plain
        // `try`/`catch` around `await`, the same way any other failable async
        // call in this codebase reads.
        ws.onclose = () => {
          failAll('connection lost')
          registering?.({ ok: false, error: 'connection lost' })
          registering = null
        }
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
      abandon('closed')
    },
  }
}
