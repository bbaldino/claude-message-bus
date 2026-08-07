import { useEffect, useState } from 'react'
import { useParams } from 'react-router-dom'
import { fetchAgent, fetchMeta } from '../data/api'
import type { AgentDetail } from '../types/AgentDetail'
import { VolumeStrip } from '../rail/VolumeStrip'
import { useTicker } from '../ui/time'
import { AgentHeader } from './AgentHeader'
import { AgentIdentity } from './AgentIdentity'
import styles from './Agent.module.css'

export function AgentScreen({ name: nameProp }: { name?: string }) {
  const params = useParams()
  const name = nameProp ?? params.name ?? ''
  const [agent, setAgent] = useState<AgentDetail | null>(null)
  const [busVersion, setBusVersion] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const now = useTicker(1000)

  useEffect(() => {
    let live = true
    setAgent(null)
    setError(null)
    fetchAgent(name)
      .then((a) => live && setAgent(a))
      .catch((e: unknown) => live && setError(e instanceof Error ? e.message : String(e)))
    // The bus version is what makes the `differs` badge computable — the rail
    // never had it, so this badge could not exist before this screen.
    fetchMeta()
      .then((m) => live && setBusVersion(m.version))
      .catch(() => {})
    return () => {
      live = false
    }
  }, [name])

  if (error)
    return (
      <p className={styles.error}>
        could not load {name}: {error}
      </p>
    )
  if (!agent) return <p className={styles.loading}>loading…</p>

  const quiet = agent.buckets.every((b) => b === 0)
  return (
    <div className={styles.screen}>
      <AgentHeader agent={agent} now={now} />
      {/* min-height: 0 on this pane is load-bearing — see Agent.module.css. */}
      <div className={styles.content}>
        <VolumeStrip buckets={agent.buckets} variant="detail" />
        <p className={styles.caption}>
          {quiet ? 'no messages in the last 100 min' : 'messages per 5 min · last 100 min'}
        </p>
        <AgentIdentity agent={agent} busVersion={busVersion} now={now} />
      </div>
    </div>
  )
}
