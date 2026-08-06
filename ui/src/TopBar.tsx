import { useEffect, useState } from 'react'
import { fetchMeta } from './data/api'
import type { Meta } from './types/Meta'
import { useStore } from './useStore'
import './TopBar.css'

export function TopBar() {
  const { connection } = useStore()
  // The generated type, not a hand-written equivalent — see Global Constraints.
  const [meta, setMeta] = useState<Meta | null>(null)

  useEffect(() => {
    fetchMeta()
      .then(setMeta)
      .catch(() => setMeta(null))
  }, [])

  return (
    <header className="topbar">
      <span className="wordmark">claude-bus</span>
      {meta && <span className="host-pill">{`${meta.host} · ${meta.version}`}</span>}
      <div className="search">
        <span className="search-icon" />
        <input
          className="search-input"
          // Rooms and agents filter client-side from the rail summary. Message
          // text has no endpoint, so the placeholder must not promise it.
          placeholder="search agents and rooms"
        />
        {/* Decorative only — matches the handoff's "/" key badge. No keyboard
            handler is wired up here; global "/" focus is not in this task's scope. */}
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
