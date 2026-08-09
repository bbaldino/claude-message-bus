import type { AgentDetail } from '../types/AgentDetail'
import { Chip } from '../ui/Chip'
import { age } from '../ui/time'
import styles from './Agent.module.css'

export function AgentHeader({ agent, now }: { agent: AgentDetail; now: number }) {
  const neverActive = agent.rooms.length === 0
  return (
    <header className={styles.header}>
      <div className={styles.titleRow}>
        {/* break-all, not ellipsis: a 36-character name like
            release-artifact-verifier#2@buildbox cannot be identified from a
            truncated form. */}
        {/* Deliberately not `agent-name` — the rail's `AgentRow` already uses that
            testid for every row it renders, and a whole-app test would resolve
            `getByTestId('agent-name')` to whichever rail row happened to render
            first, never this header. */}
        <h1 className={styles.name} data-testid="agent-detail-name">
          {agent.name}
        </h1>
        {agent.isHuman && <Chip tone="human">human</Chip>}
        <span className={agent.online ? styles.pillOnline : styles.pillOffline}>
          {!agent.online && <span className={styles.pillDot} />}
          {agent.online ? 'online' : 'offline'}
        </span>
      </div>
      <p className={styles.subtitle}>
        agent · {agent.online ? 'seen' : 'last seen'} {age(agent.lastSeen, now)} ago
        {neverActive && ' · never active in a room'}
      </p>
    </header>
  )
}
