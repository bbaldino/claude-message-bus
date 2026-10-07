import { useState } from 'react'
import { useStore } from '../useStore'
import type { RailAgent } from '../types/RailAgent'
import { useTicker } from '../ui/time'
import { AgentRow } from './AgentRow'
import styles from './Rail.module.css'

/// Blocked first, then online, each group by last seen descending, in one
/// continuous list. An earlier design draft had a separate "offline" subheading
/// and it was dropped as noise.
///
/// A blocked agent is addressed to the human, like a "needs you" room — it is
/// asking for an action, not reporting one, so it floats above everything else
/// including other online agents. Only an *online* agent's blocked status
/// counts: an offline agent cannot still be waiting on anyone.
const blockedFirst = (a: RailAgent) => (a.online && a.status?.state === 'blocked_on_human' ? 0 : 1)
function sortAgents(agents: RailAgent[]): RailAgent[] {
  return [...agents].sort(
    (a, b) =>
      blockedFirst(a) - blockedFirst(b) ||
      Number(b.online) - Number(a.online) ||
      b.lastSeen - a.lastSeen,
  )
}

/// Case-insensitive substring on the agent's name. (The inbox applies the same
/// query to conversations, by room name and member.) An empty query matches
/// everything, since `''.includes` is trivially true for every string.
function matches(name: string, query: string): boolean {
  return name.toLowerCase().includes(query.trim().toLowerCase())
}

/// The rail is the agent list: who is connected and what each is doing. Rooms
/// live in the landing view's inbox. Offline agents — mostly long-gone sessions
/// on a busy bus — start collapsed so the live fleet stays above the fold; a
/// search expands them, since a search that finds a match must show it.
export function Rail({ query = '' }: { query?: string }) {
  const { rail } = useStore()
  const now = useTicker(1000)
  const [showOffline, setShowOffline] = useState(false)
  const trimmedQuery = query.trim()
  const agents = sortAgents((rail?.agents ?? []).filter((a) => matches(a.name, query)))
  const online = agents.filter((a) => a.online)
  const offline = agents.filter((a) => !a.online)
  const offlineOpen = showOffline || trimmedQuery !== ''
  const relayers = rail?.relayers ?? []

  if (trimmedQuery !== '' && agents.length === 0) {
    return (
      <nav className={styles.rail}>
        <p className={styles.railEmpty}>nothing matched &quot;{trimmedQuery}&quot;</p>
      </nav>
    )
  }

  return (
    <nav className={styles.rail}>
      <div className={styles.railHeader} data-testid="agents-header">
        <span className={styles.railLabel}>agents</span>
        <span className={styles.railCount}>
          {online.length} of {agents.length} online
        </span>
      </div>
      <div className={styles.railRows}>
        {online.map((a) => (
          <AgentRow key={a.name} agent={a} now={now} />
        ))}
      </div>
      {offline.length > 0 && (
        <>
          <button
            className={styles.offlineToggle}
            data-testid="rail-offline-toggle"
            onClick={() => setShowOffline(!showOffline)}
          >
            {offlineOpen ? '▾' : '▸'} {offline.length} offline
          </button>
          {offlineOpen && (
            <div className={styles.railRows}>
              {offline.map((a) => (
                <AgentRow key={a.name} agent={a} now={now} />
              ))}
            </div>
          )}
        </>
      )}
      {/* The configured set, stated even when empty. A mistyped `--relayer` flag
          marks no agent, which the badges alone cannot tell apart from a correct
          config whose relayer is not connected — this line is what can. */}
      {rail && (
        <p className={styles.relayerNote} data-testid="relayer-note">
          relayers: {relayers.length > 0 ? relayers.join(', ') : '(none)'}
        </p>
      )}
    </nav>
  )
}
