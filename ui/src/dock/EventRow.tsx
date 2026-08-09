import type { Event } from '../types/Event'
import { time } from '../ui/time'
import styles from './Dock.module.css'

/// `detail` is `unknown`; render it as compact JSON rather than guessing at a
/// shape per kind. The dock is the machine record — it should show what the bus
/// wrote.
function summarise(detail: unknown): string {
  if (detail === null || detail === undefined) return ''
  if (typeof detail === 'string') return detail
  try {
    return JSON.stringify(detail)
  } catch {
    return ''
  }
}

export function EventRow({ event }: { event: Event }) {
  return (
    <div className={styles.eventRow} data-testid="event-row">
      <span className={styles.eventTime}>{time(event.createdAt)}</span>
      <span className={styles.eventKind}>{event.kind}</span>
      <span className={styles.eventDetail}>{summarise(event.detail)}</span>
    </div>
  )
}
