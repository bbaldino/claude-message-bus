import type { AgentEventItem } from '../types/AgentEventItem'
import { time } from '../ui/time'
import { kindTone } from './eventKind'
import styles from './Agent.module.css'

function summarise(detail: unknown): string {
  if (detail === null || detail === undefined) return ''
  if (typeof detail === 'string') return detail
  try {
    return JSON.stringify(detail)
  } catch {
    return ''
  }
}

export function AgentEvents({ events, total }: { events: AgentEventItem[]; total: number }) {
  return (
    <section>
      <div className={styles.sectionHeader}>
        <span className={styles.sectionLabel}>event history</span>
        <span className={styles.sectionRule} />
        {/* The true total, not events.length — the list is capped at 50. */}
        <span className={styles.sectionCount}>{total} total</span>
      </div>
      {events.length === 0 ? (
        <p className={styles.emptyLine}>Nothing has happened yet.</p>
      ) : (
        events.map((e) => (
          <div key={e.id} className={styles.eventRow}>
            <span className={styles.eventTime}>{time(e.createdAt)}</span>
            <span className={`${styles.eventKind} ${styles[kindTone(e.kind)]}`}>{e.kind}</span>
            <span className={styles.eventDetail}>{summarise(e.detail)}</span>
          </div>
        ))
      )}
    </section>
  )
}
