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
  // Two different unknowns, rendered two different ways. `busVersion === null`
  // means "cannot compare" — this bus's own `/api/meta` hasn't answered yet —
  // and rendering nothing is correct: there is genuinely no clause to add.
  // `agent.version === null` means something specific: a binary predating the
  // version field, which the old HTML UI's `version_cell` already flags with
  // "unknown · differs" and documents why. That signal must survive here too,
  // independent of whether `busVersion` happens to be known yet.
  const versionClause = () => {
    if (agent.version === null) {
      return (
        <span className={styles.secondary}>
          {' · '}
          <Chip tone="attention">differs</Chip>
        </span>
      )
    }
    if (!busVersion) return null
    return (
      <span className={styles.secondary}>
        {' · '}
        {agent.version !== busVersion ? <Chip tone="attention">differs</Chip> : 'matches bus'}
      </span>
    )
  }
  const rows: [string, React.ReactNode][] = [
    ['host', agent.host],
    ['cwd', <span className={styles.breakAll}>{agent.cwd}</span>],
    ['session', <span className={styles.breakAll}>{agent.sessionId ?? '—'}</span>],
    [
      'version',
      <>
        {agent.version ?? 'unknown'}
        {versionClause()}
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
