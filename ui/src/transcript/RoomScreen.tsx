import { useMemo } from 'react'
import { deliveryFor } from '../data/delivery'
import { useStore } from '../useStore'
import { MessageRow } from './MessageRow'
import { RoomHeader } from './RoomHeader'
import styles from './Transcript.module.css'

const day = (ms: number) => new Date(ms).toLocaleDateString([], { day: 'numeric', month: 'long' })

export function RoomScreen() {
  const { rail, messages, roomEvents, room } = useStore()
  const delivery = useMemo(() => deliveryFor(roomEvents), [roomEvents])
  const railRoom = rail?.rooms.find((r) => r.name === room)

  return (
    <div className={styles.screen}>
      {railRoom && <RoomHeader room={railRoom} agents={rail?.agents ?? []} />}
      <div className={styles.transcript}>
        {messages.map((m, i) => {
          const prev = messages[i - 1]
          const newDay = !prev || day(prev.createdAt) !== day(m.createdAt)
          return (
            <div key={m.id}>
              {newDay && (
                <div className={styles.dateDivider} data-testid="date-divider">
                  <span className={styles.rule} />
                  <span className={styles.dateLabel}>{day(m.createdAt)}</span>
                  <span className={styles.rule} />
                </div>
              )}
              <MessageRow
                message={m}
                host={rail?.agents.find((a) => a.name === m.from)?.host ?? null}
                delivery={delivery.get(m.id)}
              />
            </div>
          )
        })}
      </div>
    </div>
  )
}
