import type { PendingSend } from '../data/store'
import { store } from '../useStore'
import styles from './Transcript.module.css'

/// A send that has not landed. Deliberately not a `MessageRow`: this is not a
/// message, and must not look like one that exists on the bus.
export function PendingRow({ send }: { send: PendingSend }) {
  return (
    <div className={styles.pendingRow} data-testid="pending-row">
      <div className={styles.pendingBody}>{send.text}</div>
      {send.status === 'sending' ? (
        <span className={styles.pendingNote}>sending…</span>
      ) : (
        <span className={styles.failedNote}>
          could not send{send.error ? ` — ${send.error}` : ''}
          <button onClick={() => void store.retry(send.clientId)}>retry</button>
          <button onClick={() => store.discard(send.clientId)}>discard</button>
        </span>
      )}
    </div>
  )
}
