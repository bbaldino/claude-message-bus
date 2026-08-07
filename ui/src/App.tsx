import { useEffect } from 'react'
import { BrowserRouter, Route, Routes } from 'react-router-dom'
import { MainPlaceholder, Shell } from './Shell'
import { RoomScreen } from './transcript/RoomScreen'
import { store } from './useStore'

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
          <Route path="rooms/:name" element={<RoomScreen />} />
          <Route path="agents/:name" element={<MainPlaceholder />} />
        </Route>
      </Routes>
    </BrowserRouter>
  )
}
