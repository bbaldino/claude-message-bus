import type { Message } from '../types/Message'
import type { Delivery } from '../data/delivery'
import { Chip } from '../ui/Chip'
import { time } from '../ui/time'
import { MessageBody } from './MessageBody'
import styles from './Transcript.module.css'

/// The byline reads `name@host` for everyone, so a message sent from the web
/// console is distinguishable from the same person typing into `claude-bus chat`.
///
/// `from` may ALREADY carry the qualified form: `Registry::attach` hands out
/// `name@host` whenever it has to disambiguate, and that qualified string is what
/// gets stored on the message. Appending the host again would render
/// `bbaldino@web@web`. A `null` host is an agent whose rail entry is gone — a
/// deleted agent's old messages — and renders bare, as it already did.
export function byline(from: string, host: string | null): string {
  if (!host || from.includes('@')) return from
  return `${from}@${host}`
}

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
          <span className={styles.author}>{byline(message.from, host)}</span>
          {message.human && <Chip tone="human">human</Chip>}
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
