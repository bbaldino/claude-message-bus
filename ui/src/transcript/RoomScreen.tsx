import { useEffect, useMemo, useRef, useState } from 'react'
import { deliveryFor } from '../data/delivery'
import { day } from '../ui/time'
import { store, useStore } from '../useStore'
import { MessageRow } from './MessageRow'
import { RoomHeader } from './RoomHeader'
import { classifyArrival, isAtBottom, scrollAction, shouldLoadOlder } from './scroll'
import styles from './Transcript.module.css'

// Invariant this file maintains: `scrollTop` on the scroller has exactly four
// writers — the messages effect (initial jump, and pin-on-append), the
// ResizeObserver (re-pin after a late reflow), `onScroll` (prepend
// restoration), and the "N new below" button. Every one of them, after
// writing `scrollTop`, also updates `lastScrollTop` and (where relevant)
// `atBottom` — so the next `onScroll` can tell its own echo apart from a real
// scroll, and the next writer can trust `atBottom` without re-measuring.
//
// "Is the reader at the bottom" has two answers on purpose. `scrollAction` /
// `isAtBottom` (scroll.ts) is a live measurement — correct at the instant it
// runs, but only cheap to call from a `scroll` event, since it reads layout.
// `atBottom.current` is a cache of that measurement, updated only on a
// genuine `scroll` event (see `lastScrollTop` below), and it is what the
// ResizeObserver and the messages effect consult. It has to be a cache: pure
// content growth fires no `scroll` event, so by the time a resize is
// observed the growth has already opened a gap, and a live measurement at
// that point would always read "not at the bottom" — the resize handler
// would never be able to re-pin. The cache remembers what was true a moment
// ago, before the growth happened, which is the question that actually
// needs answering.
//
// One load-bearing subtlety: on an append, the new row commits to the DOM
// before the messages effect runs, so `scrollAction` there measures a
// non-zero distance from the bottom and returns 'notify', even when the
// reader was at the bottom a moment ago. It is the ResizeObserver — which
// fires before paint, off the DOM mutation itself — that actually delivers
// the pin. The effect's `if (atBottom.current) setUnseen(0)` sits outside
// the 'notify' branch specifically so this self-heals: `atBottom.current` is
// still true (nothing has scrolled), so the unseen count clears even though
// `scrollAction` itself never saw a 'pin'.
export function RoomScreen() {
  const { rail, messages, roomEvents, room, hasMoreHistory, dockOpen } = useStore()
  const delivery = useMemo(() => deliveryFor(roomEvents), [roomEvents])
  const railRoom = rail?.rooms.find((r) => r.name === room)

  const scroller = useRef<HTMLDivElement>(null)
  const content = useRef<HTMLDivElement>(null)
  const prevLastId = useRef<number | null>(null)
  const prevRoom = useRef<string | null>(null)
  const [unseen, setUnseen] = useState(0)
  // Guards the load-older-and-restore sequence in `onScroll`: at most one
  // restoration may be in flight at a time. The store's `loadingOlder` flag
  // guards a different thing (the fetch) — a second `onScroll` firing while a
  // load is pending still needs to be a no-op here, or a resolved-but-no-op
  // `store.loadOlder()` call would still schedule its own correction.
  const restoringOlder = useRef(false)
  // Whether the reader was at the bottom the last time we knew for sure.
  // Pure content growth (new rows, a reflow) never fires a `scroll` event by
  // itself, so this only changes on a real scroll — which is exactly what
  // the resize-observer re-pin below needs: by the time a resize is
  // observed, the growth has already opened a gap, so measuring live distance
  // from bottom at that point would always read "not at the bottom" and could
  // never re-pin.
  const atBottom = useRef(true)
  // The scrollTop this component itself last set, so `onScroll` can tell a
  // real scroll apart from the delayed echo of our own assignment. Assigning
  // `scrollTop` dispatches its `scroll` event asynchronously; if content
  // grows in the gap between the assignment and that event arriving (a
  // webfont settling, say), the event lands reporting the *old* scrollTop
  // against the *new*, taller scrollHeight — indistinguishable from the
  // reader having scrolled away if judged by distance alone. Comparing the
  // event's scrollTop against what we last set filters that out: only a
  // scrollTop we did not set ourselves is a genuine scroll.
  const lastScrollTop = useRef<number | null>(null)

  useEffect(() => {
    const el = scroller.current
    const roomChanged = room !== prevRoom.current
    const arrival = classifyArrival({ prevLastId: prevLastId.current, messages, roomChanged })
    if (el) {
      if (arrival.kind === 'initial') {
        // `scrollTop` directly, never `scrollIntoView` — the handoff is explicit,
        // and scrollIntoView also scrolls ancestor containers.
        el.scrollTop = el.scrollHeight
        lastScrollTop.current = el.scrollTop
        atBottom.current = true
        setUnseen(0)
      } else if (arrival.kind === 'append') {
        const action = scrollAction({
          scrollTop: el.scrollTop,
          scrollHeight: el.scrollHeight,
          clientHeight: el.clientHeight,
          grew: true,
        })
        if (action === 'pin') {
          el.scrollTop = el.scrollHeight
          lastScrollTop.current = el.scrollTop
          atBottom.current = true
        }
        if (action === 'notify') setUnseen((n) => n + arrival.count)
      }
      // 'none' covers a prepend (or no change); scroll-position restoration for
      // a prepend is handled in onScroll, not here.
    }
    prevLastId.current = messages.length > 0 ? messages[messages.length - 1].id : null
    prevRoom.current = room
  }, [messages, room])

  // A pin can go stale a moment after it runs, for reasons that have nothing
  // to do with new messages: webfonts finishing, a late stylesheet, anything
  // that changes layout fires no scroll event to hook. Two distinct boxes can
  // move the true bottom, and both need watching: the *content* growing
  // taller (rows reflowing) changes `scrollHeight`, and the *scroller itself*
  // shrinking changes `clientHeight` — which happens when a sibling above it
  // (the room header) grows from the same font-metric settling, squeezing how
  // much vertical space the flex layout leaves the scroller. Content growth
  // alone was the first cut here and missed the second case: `scrollHeight`
  // can stay flat while `clientHeight` alone shrinks, which is exactly what a
  // header reflow looks like, and observing only the content div reports
  // nothing for it. Whenever either resizes, if the reader was at the bottom
  // (per `atBottom`, tracked from real scroll events — see above), follow it
  // down; a reader who has deliberately scrolled away is never yanked back.
  useEffect(() => {
    const el = scroller.current
    const contentEl = content.current
    if (!el || !contentEl || typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(() => {
      if (atBottom.current) {
        el.scrollTop = el.scrollHeight
        lastScrollTop.current = el.scrollTop
      }
    })
    observer.observe(el)
    observer.observe(contentEl)
    return () => observer.disconnect()
  }, [])

  const onScroll = async () => {
    const el = scroller.current
    if (!el) return
    if (el.scrollTop !== lastScrollTop.current) {
      // Not an echo of something we set ourselves — a genuine scroll (the
      // reader's, or the first event this component has ever seen). Judge
      // it from the live position and adopt it as the new known baseline.
      atBottom.current = isAtBottom({
        scrollTop: el.scrollTop,
        scrollHeight: el.scrollHeight,
        clientHeight: el.clientHeight,
      })
      lastScrollTop.current = el.scrollTop
    }
    if (atBottom.current) setUnseen(0)
    // `onScroll` fires repeatedly while a load is in flight. The store's
    // `loadingOlder` flag only stops a duplicate fetch — a second call here
    // still resolves (as a no-op) and must not schedule its own restoration,
    // or an anchor message drifts by a whole prepend height. `restoringOlder`
    // guards the entire load-and-restore sequence instead, so at most one
    // restoration is ever in flight.
    if (hasMoreHistory && shouldLoadOlder(el) && !restoringOlder.current) {
      restoringOlder.current = true
      const before = el.scrollHeight
      // Captured now, not read later: if the room changes while the fetch
      // below is in flight, this closure still names the room the fetch was
      // for.
      const forRoom = room
      try {
        await store.loadOlder()
        // Restore in the same frame the new rows land in, or the viewport jumps
        // by the height of the page just prepended. If `loadOlder` no-oped
        // (nothing more to load), `scrollHeight` is unchanged and this delta is
        // zero — harmless.
        requestAnimationFrame(() => {
          // The route has no `key`, so navigating room -> room reuses this
          // component and this same scroller node. `loadOlder()` is
          // generation-guarded and resolves as a no-op for a superseded
          // room, but this rAF was already queued against `el` before that
          // happened — without this check it would apply `before` (A's
          // height) to B's `scrollHeight`, landing B at an arbitrary offset.
          // The mutex still has to be released here on the bail path, or
          // paging wedges shut for the new room permanently.
          if (store.getState().room !== forRoom) {
            restoringOlder.current = false
            return
          }
          el.scrollTop += el.scrollHeight - before
          lastScrollTop.current = el.scrollTop
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
        <div ref={content}>
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
      </div>
      {unseen > 0 && (
        <button
          className={styles.newBelow}
          onClick={() => {
            const el = scroller.current
            if (el) {
              el.scrollTop = el.scrollHeight
              lastScrollTop.current = el.scrollTop
              atBottom.current = true
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
