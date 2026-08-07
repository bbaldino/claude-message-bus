import type { AgentDetail } from '../types/AgentDetail'
import { Chip } from '../ui/Chip'
import { age } from '../ui/time'
import styles from './Agent.module.css'

/// A definition list rather than a table row. The secondary clause carries the
/// complementary form — absolute beside relative, matches-or-differs beside the
/// version — so both are present without a second column.
export function AgentIdentity({
  agent,
  busVersion,
  now,
}: {
  agent: AgentDetail
  busVersion: string | null
  now: number
}) {
  const differs = !!agent.version && !!busVersion && agent.version !== busVersion
  const rows: [string, React.ReactNode][] = [
    ['host', agent.host],
    ['cwd', <span className={styles.breakAll}>{agent.cwd}</span>],
    ['session', <span className={styles.breakAll}>{agent.sessionId ?? '—'}</span>],
    [
      'version',
      <>
        {agent.version ?? '—'}
        {agent.version && busVersion && (
          <span className={styles.secondary}>
            {' · '}
            {differs ? <Chip tone="attention">differs</Chip> : 'matches bus'}
          </span>
        )}
      </>,
    ],
    [
      'last seen',
      <>
        {new Date(agent.lastSeen).toLocaleString()}
        <span className={styles.secondary}> · {age(agent.lastSeen, now)} ago</span>
      </>,
    ],
  ]
  return (
    <dl className={styles.identity}>
      {rows.map(([label, value]) => (
        <div key={label} className={styles.identityRow}>
          <dt className={styles.identityLabel}>{label}</dt>
          <dd className={styles.identityValue}>{value}</dd>
        </div>
      ))}
    </dl>
  )
}
