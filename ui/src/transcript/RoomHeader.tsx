import type { RailAgent } from '../types/RailAgent'
import type { RailRoom } from '../types/RailRoom'
import styles from './Transcript.module.css'

/// The `files · N` button the handoff puts here is deliberately absent: there is
/// no JSON endpoint for file counts, and rendering `files · 0` when the real
/// number might be five is the failure this effort keeps avoiding. It arrives
/// with the files screen, alongside its endpoint.
export function RoomHeader({ room, agents }: { room: RailRoom; agents: RailAgent[] }) {
  const online = room.members.filter((m) => agents.find((a) => a.name === m)?.online).length
  return (
    <header className={styles.header}>
      <h1 className={styles.roomName}>{room.name}</h1>
      <span className={styles.members}>
        {room.members.length} member{room.members.length === 1 ? '' : 's'} · {online} online
      </span>
      <div className={styles.memberPills}>
        {room.members.map((m) => {
          const isOnline = !!agents.find((a) => a.name === m)?.online
          return (
            <span key={m} className={`${styles.pill} ${isOnline ? styles.pillOnline : ''}`}>
              <span className={styles.pillDot} />
              {m}
            </span>
          )
        })}
      </div>
    </header>
  )
}
