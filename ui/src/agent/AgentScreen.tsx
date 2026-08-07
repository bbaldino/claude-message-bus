import { useEffect, useState } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { fetchAgent, fetchMeta } from '../data/api'
import type { AgentDetail } from '../types/AgentDetail'
import { VolumeStrip } from '../rail/VolumeStrip'
import { age, useTicker } from '../ui/time'
import { AgentEvents } from './AgentEvents'
import { AgentHeader } from './AgentHeader'
import { AgentIdentity } from './AgentIdentity'
import { AgentRooms } from './AgentRooms'
import { DeleteModal } from './DeleteModal'
import styles from './Agent.module.css'

export function AgentScreen({ name: nameProp }: { name?: string }) {
  const params = useParams()
  const navigate = useNavigate()
  const name = nameProp ?? params.name ?? ''
  const [agent, setAgent] = useState<AgentDetail | null>(null)
  const [busVersion, setBusVersion] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [deleting, setDeleting] = useState(false)
  const now = useTicker(1000)

  useEffect(() => {
    let live = true
    setAgent(null)
    setError(null)
    // A second deliberate exception to "components subscribe to the store;
    // nothing fetches on its own" (see useStore.ts and TopBar's `fetchMeta`
    // effect, the first exception). Safe here for the same reason: agent
    // detail has exactly one consumer (this screen) and is fetched once per
    // screen visit — there is no second view for it to disagree with. That
    // stops being true the moment either of two things happens: a second
    // consumer of agent detail appears, or this screen wants to update live
    // while open (e.g. reflect a room join without a manual refresh) — either
    // one means this belongs in the store instead.
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
        <AgentRooms rooms={agent.rooms} now={now} />
        <AgentEvents events={agent.events} total={agent.eventTotal} />
      </div>
      <footer className={styles.deleteFooter}>
        {agent.online ? (
          <>
            <button className={styles.deleteButtonDisabled} disabled>
              delete
            </button>
            {/* Stated inline, always visible — a control you cannot use should
                say why before you try it, not on click or hover. */}
            <span className={styles.deleteReason}>
              online agents cannot be deleted — stop the session first
            </span>
          </>
        ) : (
          <>
            <button className={styles.deleteButton} onClick={() => setDeleting(true)}>
              delete
            </button>
            <span className={styles.deleteReason}>
              offline {age(agent.lastSeen, now)} · safe to remove
            </span>
          </>
        )}
      </footer>
      {deleting && (
        <DeleteModal
          name={name}
          onClose={() => setDeleting(false)}
          onDeleted={() => navigate('/')}
        />
      )}
    </div>
  )
}
