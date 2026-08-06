import { useState } from 'react'
import { Outlet, useParams } from 'react-router-dom'
import { Rail } from './rail/Rail'
import { TopBar } from './TopBar'
import './Shell.css'

/// The main pane in this phase. Not a screen — a labelled hole that the room
/// screen fills next. It should not be polished.
export function MainPlaceholder() {
  const { name } = useParams()
  return (
    <p className="shell-placeholder" data-testid="main-placeholder">
      {name ? `selected: ${name}` : 'select a room or agent'}
    </p>
  )
}

export function Shell() {
  // Lifted here, not in the store: it's transient UI state private to this
  // pair of siblings, not something any other consumer needs to read.
  const [query, setQuery] = useState('')

  return (
    <div className="shell">
      <TopBar value={query} onChange={setQuery} />
      <div className="shell-body">
        <Rail query={query} />
        <main className="shell-main">
          <Outlet />
        </main>
      </div>
    </div>
  )
}
