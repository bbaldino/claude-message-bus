export type Measurements = { scrollTop: number; scrollHeight: number; clientHeight: number }

/// jsdom has no layout — `scrollHeight` and `clientHeight` are always zero there
/// — so a test that drives the DOM would pass vacuously. Keeping the decision
/// pure is what makes it testable at all; the wiring around it stays thin and is
/// covered by the manual pass.
export const BOTTOM_SLACK = 4
const TOP_SLACK = 80

/// The one place "at the bottom" is decided. `onScroll` in `RoomScreen` needs
/// the same test on every genuine scroll event, not just on growth — reuse
/// this rather than re-deriving the expression.
export function isAtBottom(m: Measurements): boolean {
  const distanceFromBottom = m.scrollHeight - m.clientHeight - m.scrollTop
  return distanceFromBottom <= BOTTOM_SLACK
}

export function scrollAction(m: Measurements & { grew: boolean }): 'pin' | 'notify' | 'none' {
  if (!m.grew) return 'none'
  return isAtBottom(m) ? 'pin' : 'notify'
}

export function shouldLoadOlder(m: Measurements): boolean {
  return m.scrollTop <= TOP_SLACK
}

export type Arrival = { kind: 'initial' } | { kind: 'append'; count: number } | { kind: 'none' }

/// `messages.length` cannot tell an append from a prepend — both grow the
/// array. Classifying from the identity of the last message (its id) instead
/// tells apart a genuine arrival at the tail from a page of older history
/// landing at the head, or the room simply having just been opened.
export function classifyArrival(args: {
  prevLastId: number | null
  messages: { id: number }[]
  roomChanged: boolean
}): Arrival {
  const { prevLastId, messages, roomChanged } = args
  if (roomChanged || prevLastId === null) return { kind: 'initial' }
  const lastId = messages.length > 0 ? messages[messages.length - 1].id : null
  if (lastId === prevLastId) return { kind: 'none' }
  return { kind: 'append', count: messages.filter((m) => m.id > prevLastId).length }
}
