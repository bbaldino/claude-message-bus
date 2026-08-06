import { useEffect, useState } from 'react'
import { fetchRail } from './data/api'
import type { RailSummary } from './types/RailSummary'

// Deliberately unstyled and deliberately temporary. This proves the rail
// aggregate reaches the browser; 2b replaces it with the designed console.
export function App() {
  const [rail, setRail] = useState<RailSummary | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    fetchRail()
      .then(setRail)
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
  }, [])

  if (error) return <p>could not load the rail: {error}</p>
  if (!rail) return <p>loading…</p>

  return <pre>{JSON.stringify(rail, null, 2)}</pre>
}
