import { Link, useMatch } from 'react-router-dom'
import type { RailRoom } from '../types/RailRoom'
import { Chip } from '../ui/Chip'
import styles from './Rail.module.css'
import { VolumeStrip } from './VolumeStrip'

/// Subtitles are composed here rather than on the server. `/api/rail` ships data
/// — `{ kind: 'blocked', queued: 2, waitingOn: ['caas'] }` — precisely so the copy
/// stays design-owned. `delivered` renders as a literal 0 because `blocked` means
/// every member is offline, so it is necessarily zero and the server does not send
/// a constant.
function subtitle(room: RailRoom): string {
  if (!room.flag) {
    return `${room.members.length} member${room.members.length === 1 ? '' : 's'}`
  }
  if (room.flag.kind === 'needsYou') {
    return `hit ${room.flag.exchanges} exchanges · waiting on you`
  }
  return `waiting on ${room.flag.waitingOn.join(', ')} · ${room.flag.queued} queued, 0 delivered`
}

export function RoomRow({ room }: { room: RailRoom }) {
  // `useMatch` against the room route family, not a bare `useParams` name
  // comparison: a room and an agent can share a name, and `useParams` alone
  // can't tell which route is active, so both would render selected on either
  // route.
  const match = useMatch('/rooms/:name')
  const selected = match?.params.name === room.name
  const flagClass = room.flag?.kind === 'needsYou' ? styles.flagNeedsYou : ''
  const silent = room.lastActivity === null

  return (
    <Link
      to={`/rooms/${encodeURIComponent(room.name)}`}
      className={`${styles.row} ${selected ? styles.selected : ''} ${flagClass}`}
    >
      <div className={styles.rowLine}>
        <span className={`${styles.rowName} ${silent ? styles.empty : ''}`} data-testid="room-name">
          {room.name}
        </span>
        {room.flag && (
          <Chip tone={room.flag.kind === 'needsYou' ? 'attention' : 'destructive'}>
            {room.flag.kind === 'needsYou' ? 'needs you' : 'blocked'}
          </Chip>
        )}
        <div className={styles.spacer} />
        <VolumeStrip buckets={room.buckets} variant="rail" />
      </div>
      <div className={styles.rowSubtitle}>{subtitle(room)}</div>
    </Link>
  )
}
