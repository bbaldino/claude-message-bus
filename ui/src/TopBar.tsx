import { useEffect, useRef, useState } from 'react'
import { fetchMeta } from './data/api'
import type { Meta } from './types/Meta'
import { useStore } from './useStore'
import './TopBar.css'

type Props = {
  // Optional and uncontrolled by default so a bare `<TopBar />` (as the
  // existing tests render it) still works; `Shell` supplies both to make it a
  // controlled field shared with `Rail`.
  value?: string
  onChange?: (value: string) => void
}

export function TopBar({ value = '', onChange = () => {} }: Props) {
  const { connection } = useStore()
  // The generated type, not a hand-written equivalent — see Global Constraints.
  const [meta, setMeta] = useState<Meta | null>(null)
  const searchRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    fetchMeta()
      .then(setMeta)
      .catch(() => setMeta(null))
  }, [])

  useEffect(() => {
    // "/" focuses search — but only when the user isn't already typing
    // somewhere, or they could never type a literal "/". `preventDefault` is
    // what stops the character landing in the field the moment it gains
    // focus; without it the browser's normal keypress handling still runs
    // after this handler and inserts it.
    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== '/') return
      const active = document.activeElement
      const tag = active?.tagName
      if (
        tag === 'INPUT' ||
        tag === 'TEXTAREA' ||
        (active as HTMLElement | null)?.isContentEditable
      ) {
        return
      }
      e.preventDefault()
      searchRef.current?.focus()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [])

  return (
    <header className="topbar">
      <span className="wordmark">claude-bus</span>
      {meta && <span className="host-pill">{`${meta.host} · ${meta.version}`}</span>}
      <div className="search">
        <span className="search-icon" />
        <input
          ref={searchRef}
          className="search-input"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          // Rooms and agents filter client-side from the rail summary. Message
          // text has no endpoint, so the placeholder must not promise it.
          placeholder="search agents and rooms"
        />
        <span className="search-key">/</span>
      </div>
      {/* The websocket state, not a decoration — the handoff is emphatic. */}
      <span className={`live-pill ${connection}`} data-testid="live-pill">
        <span className="live-dot" />
        {connection}
      </span>
      {/* Inert until light mode lands. It occupies space in the bar's specified
          geometry, so omitting it would change the layout. */}
      <button className="theme-toggle" disabled>
        dark
      </button>
    </header>
  )
}
