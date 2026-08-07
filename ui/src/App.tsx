import { useEffect } from 'react'
import { BrowserRouter, Route, Routes, useParams } from 'react-router-dom'
import { AgentScreen } from './agent/AgentScreen'
import { MainPlaceholder, Shell } from './Shell'
import { RoomScreen } from './transcript/RoomScreen'
import { store } from './useStore'

/// A fresh RoomScreen per room. Without the key, React reuses one instance and
/// one scroller DOM node across rooms, which is what forced `prevRoom`,
/// forced `roomChanged` through `classifyArrival`, and let a paging correction
/// in flight apply one room's height delta to another's node.
function KeyedRoomScreen() {
  const { name } = useParams()
  return <RoomScreen key={name} />
}

/// A fresh AgentScreen per agent, mirroring `KeyedRoomScreen` above — this
/// route was missing its own key. Without it, following a rail link from one
/// agent's page to another re-renders the same instance in place rather than
/// mounting a new one, which is what let `AgentScreen`'s open `DeleteModal`
/// silently re-target: the modal's `name` prop would change under it while it
/// stayed mounted, open, and holding whatever the operator had already typed.
function KeyedAgentScreen() {
  const { name } = useParams()
  return <AgentScreen key={name} />
}

export function App() {
  useEffect(() => {
    void store.start()
    return () => store.stop()
  }, [])

  return (
    // basename must match Vite's `base`. The SPA is served at /app while the
    // original UI still holds /, and a mismatch breaks deep links in exactly the
    // way the catch-all route test was written to catch.
    <BrowserRouter basename="/app">
      <Routes>
        <Route path="/" element={<Shell />}>
          <Route index element={<MainPlaceholder />} />
          <Route path="rooms/:name" element={<KeyedRoomScreen />} />
          <Route path="agents/:name" element={<KeyedAgentScreen />} />
        </Route>
      </Routes>
    </BrowserRouter>
  )
}
