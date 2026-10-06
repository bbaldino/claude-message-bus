import type { RailRoom } from '../types/RailRoom'

/// Conversations active within this window are "recent"; the rest sit behind
/// the "older" expander. Inclusive at exactly 24h.
export const RECENT_WINDOW_MS = 24 * 60 * 60 * 1000

export type InboxGroups = { recent: RailRoom[]; older: RailRoom[]; hidden: RailRoom[] }

/// The rail's room ranking, kept identical: `needs you` is addressed to the
/// operator, so it outranks `blocked`, which reports a stuck delivery.
function rank(room: RailRoom): number {
  if (room.flag?.kind === 'needsYou') return 0
  if (room.flag?.kind === 'blocked') return 1
  return 2
}

function matchesQuery(room: RailRoom, q: string): boolean {
  if (q === '') return true
  return (
    room.name.toLowerCase().includes(q) || room.members.some((m) => m.toLowerCase().includes(q))
  )
}

export function shapeInbox(
  rooms: RailRoom[],
  { now, agent, query }: { now: number; agent: string | null; query: string },
): InboxGroups {
  const q = query.trim().toLowerCase()
  const sorted = rooms
    .filter((r) => agent === null || r.members.includes(agent))
    .filter((r) => matchesQuery(r, q))
    .sort((a, b) => rank(a) - rank(b) || (b.lastActivity ?? 0) - (a.lastActivity ?? 0))
  const groups: InboxGroups = { recent: [], older: [], hidden: [] }
  for (const r of sorted) {
    if (r.hidden) groups.hidden.push(r)
    else if (r.lastActivity !== null && now - r.lastActivity <= RECENT_WINDOW_MS)
      groups.recent.push(r)
    else groups.older.push(r)
  }
  return groups
}

/// A DM (`dm:a|b`) reads as its members; anything else as a channel. Parsed from
/// the name rather than `members`, because membership changes (a deleted agent)
/// while the room's identity does not. A malformed `dm:` name falls back to the
/// raw name rather than rendering nothing.
export function displayName(room: RailRoom): string {
  if (room.name.startsWith('dm:')) {
    const parts = room.name
      .slice(3)
      .split('|')
      .filter((p) => p !== '')
    return parts.length > 0 ? parts.join(' ⇄ ') : room.name
  }
  return `#${room.name}`
}
