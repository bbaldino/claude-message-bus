import { useEffect, useMemo, useRef, useState } from 'react'
import { store, useStore } from '../useStore'
import { isTypingTarget, modKey } from '../ui/platform'
import { EventRow } from './EventRow'
import styles from './Dock.module.css'

export function EventsDock() {
  const { events, roomEvents, dockOpen } = useStore()
  const [scope, setScope] = useState<'room' | 'bus'>('room')
  const [hidden, setHidden] = useState<Set<string>>(new Set())
  const [kindsOpen, setKindsOpen] = useState(false)
  const [unseen, setUnseen] = useState(0)
  const mod = useMemo(modKey, [])
  const setOpen = (open: boolean) => store.setDockOpen(open)

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (isTypingTarget(e.target)) return
      if (!mod.matches(e)) return
      e.preventDefault()
      store.setDockOpen(!store.getState().dockOpen)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [mod])

  const source = scope === 'room' ? roomEvents : events
  const prevLen = useRef(source.length)

  // A scope switch swaps the entire list, so its length change is not arrivals.
  // Rebase rather than counting it, or toggling the segmented control would
  // inflate the badge.
  // Deliberately keyed on `scope` alone: this must fire when the scope changes
  // and not when the list it points at grows.
  useEffect(() => {
    prevLen.current = source.length
    setUnseen(0)
  }, [scope])

  useEffect(() => {
    if (dockOpen) setUnseen(0)
  }, [dockOpen])

  // Counts against the current scope, so the badge agrees with what opening the
  // dock will actually show. Counts the delta rather than incrementing by one,
  // so a burst of events is reported honestly.
  useEffect(() => {
    const delta = source.length - prevLen.current
    prevLen.current = source.length
    if (delta > 0 && !dockOpen) setUnseen((n) => n + delta)
  }, [source.length, dockOpen])

  const kinds = useMemo(() => [...new Set(source.map((e) => e.kind))].sort(), [source])
  const shown = source.filter((e) => !hidden.has(e.kind))

  if (!dockOpen) {
    return (
      <aside className={styles.closed} onClick={() => setOpen(true)} data-testid="dock-closed">
        <span className={styles.liveDot} />
        {unseen > 0 && <span className={styles.unseen}>{unseen}</span>}
        <span className={styles.vertical}>events</span>
        <span className={styles.chord}>{mod.label}</span>
      </aside>
    )
  }

  return (
    <aside className={styles.open} data-testid="dock-open">
      <header className={styles.header}>
        <span className={styles.label}>events</span>
        <span className={styles.liveDotOpen} />
        <button className={styles.chordButton} onClick={() => setOpen(false)}>
          {mod.label}
        </button>
      </header>
      <div className={styles.scope}>
        <button
          className={scope === 'room' ? styles.segmentOn : styles.segment}
          onClick={() => setScope('room')}
        >
          this room
        </button>
        <button
          className={scope === 'bus' ? styles.segmentOn : styles.segment}
          onClick={() => setScope('bus')}
        >
          whole bus
        </button>
        <button
          className={styles.kinds}
          data-testid="kinds-toggle"
          onClick={() => setKindsOpen((k) => !k)}
        >
          kinds ▾
        </button>
      </div>
      {kindsOpen && (
        <div className={styles.kindList}>
          {kinds.map((k) => (
            <label key={k} className={styles.kindItem}>
              <input
                type="checkbox"
                aria-label={k}
                checked={!hidden.has(k)}
                onChange={() =>
                  setHidden((h) => {
                    const next = new Set(h)
                    if (next.has(k)) next.delete(k)
                    else next.add(k)
                    return next
                  })
                }
              />
              {k}
            </label>
          ))}
        </div>
      )}
      <div className={styles.list}>
        {shown.map((e) => (
          <EventRow key={e.id} event={e} />
        ))}
      </div>
    </aside>
  )
}
