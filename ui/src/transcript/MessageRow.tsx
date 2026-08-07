import type { Message } from '../types/Message'
import type { Delivery } from '../data/delivery'
import { Chip } from '../ui/Chip'
import { MessageBody } from './MessageBody'
import styles from './Transcript.module.css'

const time = (ms: number) =>
  new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false })

export function MessageRow({
  message,
  host,
  delivery,
  narrow,
}: {
  message: Message
  host: string | null
  delivery: Delivery | undefined
  narrow?: boolean
}) {
  return (
    <div className={styles.row} data-testid="message-row">
      <div className={styles.gutter}>{time(message.createdAt)}</div>
      <div className={`${styles.bodyCol} ${message.human ? styles.fromHuman : ''}`}>
        <div className={styles.byline}>
          {/* Sans, not mono: an author is a name being read, not an identifier
              being matched. The handoff is explicit about the inversion. */}
          <span className={styles.author}>{message.from}</span>
          {message.human ? (
            <Chip tone="human">human</Chip>
          ) : (
            <span className={styles.host}>{host}</span>
          )}
        </div>
        <MessageBody body={message.body} narrow={narrow} />
        <div className={styles.meta}>
          <span className={styles.seq}>#{message.id}</span>
          {delivery && delivery.deliveredTo.length > 0 && (
            <span className={styles.delivered}>delivered to {delivery.deliveredTo.join(', ')}</span>
          )}
          {/* Queued is visually distinct from delivered because that distinction
              is the point: a message can be sent and not have arrived. */}
          {delivery && delivery.queuedFor.length > 0 && (
            <span className={styles.queued}>
              <span className={styles.queuedDot} />
              queued for {delivery.queuedFor.join(', ')}
            </span>
          )}
          {message.done && (
            <Chip tone="presence" size="md">
              done
            </Chip>
          )}
        </div>
      </div>
    </div>
  )
}
