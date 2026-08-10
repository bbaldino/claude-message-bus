/// The handoff assigns hue *families* — blue delivery, violet lifecycle, amber
/// attention, red destructive, teal files, green presence — but never maps event
/// kinds onto them. This is that mapping, and it is our invention constrained by
/// the families rather than something transcribed.
///
/// The lists below are the kinds `append_event(` actually writes (grepped
/// across `src/` again for the hide-rooms final review, which found
/// `room_hidden` and `room_unhidden` both landing in the delivery-blue
/// fallback, unmapped), not a guess. A first pass guessed at plausible kind
/// names — things like `agent_online`, `file_put`, `message_delivered`,
/// `room_deleted`, `agent_renamed`, a bare `joined`/`left` — and none of those
/// literal strings are ever emitted. The real, verified set is thirteen
/// kinds; see the report for the reasoning behind each placement.
///
/// Unknown kinds fall back to the delivery blue rather than throwing: the bus
/// gains event kinds over time and a new one must render, not break a screen.
export type KindTone = 'accent' | 'human' | 'attention' | 'destructive' | 'files' | 'presence'

const FAMILIES: [KindTone, string[]][] = [
  // green — presence. `agent_disconnected` is the only presence-shaped kind the
  // bus logs; there is no `agent_online`/`agent_connected` counterpart — coming
  // online is tracked via `notify_presence` and an `upsert_agent` row, not an
  // event-log entry.
  ['presence', ['agent_disconnected']],
  // violet — lifecycle. `room_joined` is an agent joining a room (register.rs
  // calls it that, not `joined`); `agent_registered` is the agent itself coming
  // into being. `room_hidden`/`room_unhidden` join them rather than amber: they
  // are a deliberate, reversible operator action on the room's own visibility
  // state, not a warning the bus is raising (that's what `room_paused` and
  // `rate_limited` are for) — the same "two ends of one story" shape as
  // `room_paused`/`resumed` below, but for a lifecycle fact about the room
  // rather than a flow-control one.
  ['human', ['agent_registered', 'room_joined', 'room_hidden', 'room_unhidden']],
  // amber — attention. `room_paused` and `resumed` are the two ends of the same
  // flow-control state (grouped together deliberately — they're the same story);
  // `rate_limited` is the other warning-shaped event the guard logs.
  ['attention', ['room_paused', 'resumed', 'rate_limited']],
  // red — destructive.
  ['destructive', ['agent_deleted']],
  // teal — files. Real kinds are `file_stored`/`file_fetched`, not the guessed
  // `file_put`/`file_get`.
  ['files', ['file_stored', 'file_fetched']],
  // blue — delivery. `ack` is a delivery confirmation (cursor advance), the same
  // family as `message_sent`. There is no `message_delivered` kind.
  ['accent', ['message_sent', 'ack']],
]

export function kindTone(kind: string): KindTone {
  for (const [tone, kinds] of FAMILIES) {
    if (kinds.includes(kind)) return tone
  }
  return 'accent'
}
