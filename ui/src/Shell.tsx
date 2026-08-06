import { useEffect, useState } from 'react'
import { Outlet, useMatch, useParams } from 'react-router-dom'
import { Rail } from './rail/Rail'
import { TopBar } from './TopBar'
import { store } from './useStore'
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

  // The one place the route drives the store. `useMatch` (not `useParams`)
  // because this needs to know whether the *room* route family is active, not
  // just read whatever `:name` happens to be — the rail sits outside the
  // `Outlet`, so `useParams` here would see stale params from whatever route
  // last matched. Its `params.name` is already decoded, so a room like
  // `dm:caas|network-debug#2` reaches `selectRoom` as itself, not as the
  // percent-encoded form the `Link` in RoomRow puts in the URL.
  //
  // Leaving the room route — to an agent route, or back to the index — passes
  // `null` rather than leaving the last room selected. The alternative (keep
  // the last room) would leave the store watching, and filtering messages
  // into, a room the operator can no longer see on screen: exactly the
  // "two views disagreeing about what is current" failure the store exists to
  // prevent.
  const roomMatch = useMatch('/rooms/:name')
  useEffect(() => {
    store.selectRoom(roomMatch?.params.name ?? null)
  }, [roomMatch?.params.name])

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
