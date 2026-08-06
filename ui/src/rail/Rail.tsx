import { useEffect, useState } from 'react'
import { useStore } from '../useStore'
import type { RailAgent } from '../types/RailAgent'
import type { RailRoom } from '../types/RailRoom'
import { AgentRow } from './AgentRow'
import { RoomRow } from './RoomRow'
import './Rail.css'

/// Flagged rooms float to the top, `needs you` above `blocked`, then everything
/// else by last activity. `needs you` outranks `blocked` because it is the state
/// addressed to the operator — it asks for an action rather than reporting one.
function rank(room: RailRoom): number {
  if (room.flag?.kind === 'needsYou') return 0
  if (room.flag?.kind === 'blocked') return 1
  return 2
}

function sortRooms(rooms: RailRoom[]): RailRoom[] {
  return [...rooms].sort(
    (a, b) => rank(a) - rank(b) || (b.lastActivity ?? 0) - (a.lastActivity ?? 0),
  )
}

/// Online first, each group by last seen descending, in one continuous list. An
/// earlier design draft had a separate "offline" subheading and it was dropped as
/// noise.
function sortAgents(agents: RailAgent[]): RailAgent[] {
  return [...agents].sort((a, b) => Number(b.online) - Number(a.online) || b.lastSeen - a.lastSeen)
}

/// One ticker shared by the whole rail, not one per row — the handoff says
/// relative timestamps are "derived per render... re-derive on a timer so '4s
/// ago' stays true". A single interval here re-renders every row at once; eight
/// rows each owning an interval is the version of this that causes problems
/// later. One second matches the handoff's own "4s ago" granularity. Cleaned up
/// on unmount — this codebase has already had a bug where an interval outlived
/// its owner.
function useTicker(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs)
    return () => clearInterval(id)
  }, [intervalMs])
  return now
}

/// Case-insensitive substring on the name only — rooms and agents are all this
/// filters, matching the top bar's placeholder. An empty query matches
/// everything, since `''.includes` is trivially true for every string.
function matches(name: string, query: string): boolean {
  return name.toLowerCase().includes(query.trim().toLowerCase())
}

export function Rail({ query = '' }: { query?: string }) {
  const { rail } = useStore()
  const now = useTicker(1000)
  const trimmedQuery = query.trim()
  const rooms = sortRooms((rail?.rooms ?? []).filter((r) => matches(r.name, query)))
  const agents = sortAgents((rail?.agents ?? []).filter((a) => matches(a.name, query)))
  const online = agents.filter((a) => a.online).length
  // Only for a search that matches nothing at all — a query that matches
  // agents but no rooms (or vice versa) still gets its normal empty section,
  // header and all, since that's a real, legible statement about that half of
  // the fleet. This is the "I was fooled during manual testing" case: both
  // sections empty, with nothing on screen to say why.
  const noMatches = trimmedQuery !== '' && rooms.length === 0 && agents.length === 0

  if (noMatches) {
    return (
      <nav className="rail">
        <p className="rail-empty">nothing matched &quot;{trimmedQuery}&quot;</p>
      </nav>
    )
  }

  return (
    <nav className="rail">
      <div className="rail-header">
        <span className="rail-label">rooms</span>
        <span className="rail-count">last 60 min</span>
      </div>
      <div className="rail-rows">
        {rooms.map((r) => (
          <RoomRow key={r.name} room={r} />
        ))}
      </div>

      <div className="rail-header agents" data-testid="agents-header">
        <span className="rail-label">agents</span>
        <span className="rail-count">
          {online} of {agents.length} online
        </span>
      </div>
      <div className="rail-rows">
        {agents.map((a) => (
          <AgentRow key={a.name} agent={a} now={now} />
        ))}
      </div>
    </nav>
  )
}
