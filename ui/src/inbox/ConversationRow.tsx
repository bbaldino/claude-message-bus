import { Link } from 'react-router-dom'
import { VolumeStrip } from '../rail/VolumeStrip'
import type { RailRoom } from '../types/RailRoom'
import { Chip } from '../ui/Chip'
import { age } from '../ui/time'
import { displayName } from './inbox'
import styles from './Inbox.module.css'

/// The flag detail line, moved here from the rail's old room row. Composed
/// client-side so the copy stays design-owned; `delivered` is a literal 0 because
/// `blocked` means every member is offline, so the server does not send a constant.
function flagDetail(room: RailRoom): string | null {
  if (!room.flag) return null
  if (room.flag.kind === 'needsYou') return `hit ${room.flag.exchanges} exchanges · waiting on you`
  return `waiting on ${room.flag.waitingOn.join(', ')} · ${room.flag.queued} queued, 0 delivered`
}

export function ConversationRow({
  room,
  now,
  dimmed = false,
}: {
  room: RailRoom
  now: number
  dimmed?: boolean
}) {
  const detail = flagDetail(room)
  const m = room.lastMessage
  return (
    <Link
      to={`/rooms/${encodeURIComponent(room.name)}`}
      className={`${styles.row} ${room.flag?.kind === 'needsYou' ? styles.flagNeedsYou : ''} ${dimmed ? styles.dimmed : ''}`}
      data-testid="inbox-row"
      data-room={room.name}
    >
      <div className={styles.rowLine}>
        <span className={styles.name}>{displayName(room)}</span>
        {room.flag && (
          <Chip tone={room.flag.kind === 'needsYou' ? 'attention' : 'destructive'}>
            {room.flag.kind === 'needsYou' ? 'needs you' : 'blocked'}
          </Chip>
        )}
        <div className={styles.spacer} />
        <VolumeStrip buckets={room.buckets} variant="rail" />
        {room.lastActivity !== null && (
          <span className={styles.age}>{age(room.lastActivity, now)}</span>
        )}
      </div>
      {/* Plain text, never HTML: an agent's message is untrusted content. */}
      <div className={styles.preview}>
        {m ? (
          <>
            {/* Marked the way the transcript marks a message with human authority. */}
            {m.human && <Chip tone="human">human</Chip>}
            <span>{`${m.from}: ${m.excerpt}`}</span>
          </>
        ) : (
          'no messages yet'
        )}
      </div>
      {detail && <div className={styles.detail}>{detail}</div>}
    </Link>
  )
}
