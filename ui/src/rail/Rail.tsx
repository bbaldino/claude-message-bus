import { useState } from 'react'
import { useStore } from '../useStore'
import type { RailAgent } from '../types/RailAgent'
import type { RailRoom } from '../types/RailRoom'
import { useTicker } from '../ui/time'
import { AgentRow } from './AgentRow'
import { RoomRow } from './RoomRow'
import styles from './Rail.module.css'

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
  const [showHidden, setShowHidden] = useState(false)
  const visibleRooms = rooms.filter((r) => !r.hidden)
  const hiddenRooms = rooms.filter((r) => r.hidden)
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
      <nav className={styles.rail}>
        <p className={styles.railEmpty}>nothing matched &quot;{trimmedQuery}&quot;</p>
      </nav>
    )
  }

  return (
    <nav className={styles.rail}>
      <div className={styles.railHeader}>
        <span className={styles.railLabel}>rooms</span>
        <span className={styles.railCount}>last 60 min</span>
      </div>
      <div className={styles.railRows}>
        {visibleRooms.map((r) => (
          <RoomRow key={r.name} room={r} />
        ))}
      </div>

      {hiddenRooms.length > 0 && (
        <>
          <button className={styles.hiddenToggle} onClick={() => setShowHidden(!showHidden)}>
            {hiddenRooms.length} hidden {showHidden ? '▴' : '▾'}
          </button>
          {showHidden && (
            <div className={styles.railRows}>
              {hiddenRooms.map((r) => (
                <RoomRow key={r.name} room={r} dimmed />
              ))}
            </div>
          )}
        </>
      )}

      <div className={`${styles.railHeader} ${styles.agents}`} data-testid="agents-header">
        <span className={styles.railLabel}>agents</span>
        <span className={styles.railCount}>
          {online} of {agents.length} online
        </span>
      </div>
      <div className={styles.railRows}>
        {agents.map((a) => (
          <AgentRow key={a.name} agent={a} now={now} />
        ))}
      </div>
    </nav>
  )
}
