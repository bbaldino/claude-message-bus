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
  return (
    <div className="shell">
      <TopBar />
      <div className="shell-body">
        <Rail />
        <main className="shell-main">
          <Outlet />
        </main>
      </div>
    </div>
  )
}
