import { useEffect, useState } from 'react'
import type { Agent } from './types/Agent'

// Deliberately unstyled. This screen exists to prove the pipeline — bundle
// embedded in the binary, served at /app, fed by /api/agents — and is replaced
// wholesale by the design pass output. Anything prettier here is deleted later.
export function App() {
  const [agents, setAgents] = useState<Agent[] | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    fetch('/api/agents')
      .then((r) => {
        if (!r.ok) throw new Error(`/api/agents returned ${r.status}`)
        return r.json()
      })
      .then(setAgents)
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
  }, [])

  if (error) return <p>could not load agents: {error}</p>
  if (!agents) return <p>loading…</p>

  return (
    <table>
      <thead>
        <tr>
          <th>name</th>
          <th>host</th>
          <th>version</th>
          <th>state</th>
        </tr>
      </thead>
      <tbody>
        {agents.map((a) => (
          <tr key={a.name}>
            <td>{a.name}</td>
            <td>{a.host}</td>
            <td>{a.version ?? 'unknown'}</td>
            <td>{a.online ? 'online' : 'offline'}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
