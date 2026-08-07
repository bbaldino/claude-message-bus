import type { Event } from '../types/Event'

export type Delivery = { deliveredTo: string[]; queuedFor: string[] }

/// Delivery metadata lives in `message_sent` event detail, not on the message
/// row — the bus fans a message out and only then records who it reached, so a
/// live-pushed message could not carry it. Correlating the event is the only way
/// to fill it in without a refetch.
///
/// `Event.detail` is `unknown` on the generated type, so it is narrowed here at
/// the boundary rather than asserted.
export function deliveryFor(events: Event[]): Map<number, Delivery> {
  const out = new Map<number, Delivery>()
  for (const e of events) {
    if (e.kind !== 'message_sent') continue
    const d = e.detail as Record<string, unknown> | null
    if (!d || typeof d.msg_id !== 'number') continue
    out.set(d.msg_id, {
      deliveredTo: Array.isArray(d.delivered_to) ? (d.delivered_to as string[]) : [],
      queuedFor: Array.isArray(d.queued_for) ? (d.queued_for as string[]) : [],
    })
  }
  return out
}
