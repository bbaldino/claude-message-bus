import { Link, useMatch } from 'react-router-dom'
import type { RailAgent } from '../types/RailAgent'
import { Chip } from '../ui/Chip'
import { age } from '../ui/time'
import styles from './Rail.module.css'
import { VolumeStrip } from './VolumeStrip'

export function AgentRow({ agent, now }: { agent: RailAgent; now: number }) {
  // See RoomRow: `useMatch` against the agent route family, so a room and an
  // agent sharing a name are never both shown as selected.
  const match = useMatch('/agents/:name')
  const selected = match?.params.name === agent.name

  return (
    <Link
      to={`/agents/${encodeURIComponent(agent.name)}`}
      className={`${styles.row} ${styles.agentRow} ${selected ? styles.selected : ''}`}
    >
      <div className={styles.rowLine}>
        <span className={`${styles.dot} ${agent.online ? styles.online : ''}`} />
        <span
          className={`${styles.agentName} ${agent.online ? styles.online : styles.offline}`}
          data-testid="agent-name"
        >
          {agent.name}
        </span>
        {agent.isHuman && <Chip tone="human">human</Chip>}
        <div className={styles.spacer} />
        <VolumeStrip buckets={agent.buckets} variant="rail" />
        <span className={styles.agentAge} data-testid="agent-age">
          {age(agent.lastSeen, now)}
        </span>
      </div>
    </Link>
  )
}
