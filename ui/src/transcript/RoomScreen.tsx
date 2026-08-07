import { useEffect, useMemo, useRef, useState } from 'react'
import { deliveryFor } from '../data/delivery'
import { store, useStore } from '../useStore'
import { MessageRow } from './MessageRow'
import { RoomHeader } from './RoomHeader'
import { BOTTOM_SLACK, classifyArrival, scrollAction, shouldLoadOlder } from './scroll'
import styles from './Transcript.module.css'

const day = (ms: number) => new Date(ms).toLocaleDateString([], { day: 'numeric', month: 'long' })

export function RoomScreen() {
  const { rail, messages, roomEvents, room, hasMoreHistory } = useStore()
  const delivery = useMemo(() => deliveryFor(roomEvents), [roomEvents])
  const railRoom = rail?.rooms.find((r) => r.name === room)

  const scroller = useRef<HTMLDivElement>(null)
  const prevLastId = useRef<number | null>(null)
  const prevRoom = useRef<string | null>(null)
  const [unseen, setUnseen] = useState(0)

  useEffect(() => {
    const el = scroller.current
    const roomChanged = room !== prevRoom.current
    const arrival = classifyArrival({ prevLastId: prevLastId.current, messages, roomChanged })
    if (el) {
      if (arrival.kind === 'initial') {
        // `scrollTop` directly, never `scrollIntoView` — the handoff is explicit,
        // and scrollIntoView also scrolls ancestor containers.
        el.scrollTop = el.scrollHeight
        setUnseen(0)
      } else if (arrival.kind === 'append') {
        const action = scrollAction({
          scrollTop: el.scrollTop,
          scrollHeight: el.scrollHeight,
          clientHeight: el.clientHeight,
          grew: true,
        })
        if (action === 'pin') el.scrollTop = el.scrollHeight
        if (action === 'notify') setUnseen((n) => n + arrival.count)
      }
      // 'none' covers a prepend (or no change); scroll-position restoration for
      // a prepend is handled in onScroll, not here.
    }
    prevLastId.current = messages.length > 0 ? messages[messages.length - 1].id : null
    prevRoom.current = room
  }, [messages, room])

  const onScroll = async () => {
    const el = scroller.current
    if (!el) return
    if (el.scrollHeight - el.clientHeight - el.scrollTop <= BOTTOM_SLACK) setUnseen(0)
    if (hasMoreHistory && shouldLoadOlder(el)) {
      const before = el.scrollHeight
      await store.loadOlder()
      // Restore in the same frame the new rows land in, or the viewport jumps by
      // the height of the page just prepended.
      requestAnimationFrame(() => {
        el.scrollTop += el.scrollHeight - before
      })
    }
  }

  return (
    <div className={styles.screen}>
      {railRoom && <RoomHeader room={railRoom} agents={rail?.agents ?? []} />}
      <div className={styles.transcript} ref={scroller} onScroll={onScroll}>
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
      {unseen > 0 && (
        <button
          className={styles.newBelow}
          onClick={() => {
            const el = scroller.current
            if (el) el.scrollTop = el.scrollHeight
            setUnseen(0)
          }}
        >
          {unseen} new below
        </button>
      )}
    </div>
  )
}
