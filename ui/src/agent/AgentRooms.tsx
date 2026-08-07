import { Link } from 'react-router-dom'
import type { AgentRoomSummary } from '../types/AgentRoomSummary'
import { age } from '../ui/time'
import styles from './Agent.module.css'

export function AgentRooms({ rooms, now }: { rooms: AgentRoomSummary[]; now: number }) {
  return (
    <section>
      <div className={styles.sectionHeader}>
        <span className={styles.sectionLabel}>rooms</span>
        <span className={styles.sectionRule} />
        <span className={styles.sectionCount}>{rooms.length}</span>
      </div>
      {rooms.length === 0 ? (
        /* A stated explanation, not blank space. */
        <p className={styles.emptyBox}>
          Never joined a room. Registered, then went quiet — the usual signature of a session that
          was killed before it did any work.
        </p>
      ) : (
        rooms.map((r) => (
          <Link key={r.name} to={`/rooms/${encodeURIComponent(r.name)}`} className={styles.roomRow}>
            <span className={styles.roomName}>{r.name}</span>
            <span className={styles.roomCount}>{r.messageCount} msgs</span>
            <span className={styles.roomAge}>
              {r.lastActivity ? `${age(r.lastActivity, now)} ago` : 'never'}
            </span>
          </Link>
        ))
      )}
    </section>
  )
}
