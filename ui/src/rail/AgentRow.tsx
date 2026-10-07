import { Link, useMatch, useSearchParams } from 'react-router-dom'
import type { RailAgent } from '../types/RailAgent'
import { Chip } from '../ui/Chip'
import { age } from '../ui/time'
import { statusText } from '../ui/statusText'
import styles from './Rail.module.css'
import { VolumeStrip } from './VolumeStrip'

/// Clicking an agent filters the landing inbox to its conversations
/// (`/?agent=<name>`); clicking it again clears the filter. The agent's own page
/// sits behind the selected row's "details →" link — a sibling of the row link,
/// never nested inside it (an anchor inside an anchor is invalid HTML and makes
/// the click target ambiguous).
export function AgentRow({ agent, now }: { agent: RailAgent; now: number }) {
  const onIndex = useMatch('/') !== null
  const [params] = useSearchParams()
  const selected = onIndex && params.get('agent') === agent.name
  const to = selected ? '/' : `/?agent=${encodeURIComponent(agent.name)}`

  return (
    <div className={styles.agentRowWrap}>
      <Link
        to={to}
        className={`${styles.row} ${styles.agentRow} ${selected ? styles.selected : ''}`}
        data-testid="rail-agent-row"
        data-agent={agent.name}
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
          {agent.isRelayer && <Chip tone="relayer">relayer</Chip>}
          <div className={styles.spacer} />
          <VolumeStrip buckets={agent.buckets} variant="rail" />
          <span className={styles.agentAge} data-testid="agent-age">
            {age(agent.lastSeen, now)}
          </span>
        </div>
        {agent.status && (
          <div
            className={`${styles.agentStatus} ${
              agent.online && agent.status.state === 'blocked_on_human' ? styles.agentBlocked : ''
            }`}
            data-testid={`agent-status-${agent.name}`}
          >
            {statusText(agent.status, agent.online, now)}
          </div>
        )}
      </Link>
      {selected && (
        <Link
          to={`/agents/${encodeURIComponent(agent.name)}`}
          className={styles.details}
          data-testid="rail-agent-details"
        >
          details →
        </Link>
      )}
    </div>
  )
}
