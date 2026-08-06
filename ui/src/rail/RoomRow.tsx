import { Link, useParams } from 'react-router-dom'
import type { RailRoom } from '../types/RailRoom'
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
  const { name } = useParams()
  const selected = name === room.name
  const flagClass = room.flag?.kind === 'needsYou' ? 'flag-needs-you' : ''
  const silent = room.lastActivity === null

  return (
    <Link
      to={`/rooms/${encodeURIComponent(room.name)}`}
      className={`row ${selected ? 'selected' : ''} ${flagClass}`}
    >
      <div className="row-line">
        <span className={`row-name ${silent ? 'empty' : ''}`} data-testid="room-name">
          {room.name}
        </span>
        {room.flag && (
          <span className={`flag ${room.flag.kind === 'needsYou' ? 'needs-you' : 'blocked'}`}>
            {room.flag.kind === 'needsYou' ? 'needs you' : 'blocked'}
          </span>
        )}
        <div className="spacer" />
        <VolumeStrip buckets={room.buckets} variant="rail" />
      </div>
      <div className="row-subtitle">{subtitle(room)}</div>
    </Link>
  )
}
