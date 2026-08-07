import { useEffect, useMemo, useRef, useState } from 'react'
import { deliveryFor } from '../data/delivery'
import { store, useStore } from '../useStore'
import { MessageRow } from './MessageRow'
import { RoomHeader } from './RoomHeader'
import { BOTTOM_SLACK, classifyArrival, scrollAction, shouldLoadOlder } from './scroll'
import styles from './Transcript.module.css'

const day = (ms: number) => new Date(ms).toLocaleDateString([], { day: 'numeric', month: 'long' })

export function RoomScreen() {
  const { rail, messages, roomEvents, room, hasMoreHistory, dockOpen } = useStore()
  const delivery = useMemo(() => deliveryFor(roomEvents), [roomEvents])
  const railRoom = rail?.rooms.find((r) => r.name === room)

  const scroller = useRef<HTMLDivElement>(null)
  const prevLastId = useRef<number | null>(null)
  const prevRoom = useRef<string | null>(null)
  const [unseen, setUnseen] = useState(0)
  // Guards the load-older-and-restore sequence in `onScroll`: at most one
  // restoration may be in flight at a time. The store's `loadingOlder` flag
  // guards a different thing (the fetch) — a second `onScroll` firing while a
  // load is pending still needs to be a no-op here, or a resolved-but-no-op
  // `store.loadOlder()` call would still schedule its own correction.
  const restoringOlder = useRef(false)
  // The scrollTop the initial pin last set, so a later fonts-ready re-pin can
  // tell "content grew under an untouched reader" apart from "the reader
  // scrolled since" — only the former should be corrected.
  const pinnedScrollTop = useRef<number | null>(null)

  useEffect(() => {
    const el = scroller.current
    const roomChanged = room !== prevRoom.current
    const arrival = classifyArrival({ prevLastId: prevLastId.current, messages, roomChanged })
    if (el) {
      if (arrival.kind === 'initial') {
        // `scrollTop` directly, never `scrollIntoView` — the handoff is explicit,
        // and scrollIntoView also scrolls ancestor containers.
        el.scrollTop = el.scrollHeight
        pinnedScrollTop.current = el.scrollTop
        setUnseen(0)
        // Webfonts finishing after this pin change text metrics and grow every
        // row slightly, leaving the pin short of the true bottom. Re-pin once
        // fonts settle, but only if this pin is still the right thing to do:
        // `document.fonts` is undefined in jsdom, the room may have changed by
        // the time the promise resolves, the component may have unmounted, or
        // the reader may have scrolled away from where we left them.
        const pinnedRoom = room
        document.fonts?.ready.then(() => {
          const current = scroller.current
          if (!current) return
          if (prevRoom.current !== pinnedRoom) return
          if (current.scrollTop !== pinnedScrollTop.current) return
          current.scrollTop = current.scrollHeight
          pinnedScrollTop.current = current.scrollTop
        })
      } else if (arrival.kind === 'append') {
        const action = scrollAction({
          scrollTop: el.scrollTop,
          scrollHeight: el.scrollHeight,
          clientHeight: el.clientHeight,
          grew: true,
        })
        if (action === 'pin') {
          el.scrollTop = el.scrollHeight
          pinnedScrollTop.current = el.scrollTop
        }
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
    // `onScroll` fires repeatedly while a load is in flight. The store's
    // `loadingOlder` flag only stops a duplicate fetch — a second call here
    // still resolves (as a no-op) and must not schedule its own restoration,
    // or an anchor message drifts by a whole prepend height. `restoringOlder`
    // guards the entire load-and-restore sequence instead, so at most one
    // restoration is ever in flight.
    if (hasMoreHistory && shouldLoadOlder(el) && !restoringOlder.current) {
      restoringOlder.current = true
      const before = el.scrollHeight
      try {
        await store.loadOlder()
        // Restore in the same frame the new rows land in, or the viewport jumps
        // by the height of the page just prepended. If `loadOlder` no-oped
        // (nothing more to load), `scrollHeight` is unchanged and this delta is
        // zero — harmless.
        requestAnimationFrame(() => {
          el.scrollTop += el.scrollHeight - before
          restoringOlder.current = false
        })
      } catch {
        // Don't let a rejected fetch wedge paging shut permanently.
        restoringOlder.current = false
      }
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
                narrow={dockOpen}
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
            if (el) {
              el.scrollTop = el.scrollHeight
              pinnedScrollTop.current = el.scrollTop
            }
            setUnseen(0)
          }}
        >
          {unseen} new below
        </button>
      )}
    </div>
  )
}
