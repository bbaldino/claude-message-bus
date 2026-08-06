import { Link, useParams } from 'react-router-dom'
import type { RailAgent } from '../types/RailAgent'
import { VolumeStrip } from './VolumeStrip'

/// Relative age, deliberately coarse: this is a scan target in a 26px column, not
/// a timestamp. Re-derived on render rather than stored, so it stays true.
function age(lastSeen: number, now: number): string {
  const s = Math.max(0, Math.floor((now - lastSeen) / 1000))
  if (s < 60) return `${s}s`
  if (s < 3600) return `${Math.floor(s / 60)}m`
  if (s < 86_400) return `${Math.floor(s / 3600)}h`
  return `${Math.floor(s / 86_400)}d`
}

export function AgentRow({ agent, now }: { agent: RailAgent; now: number }) {
  const { name } = useParams()
  const selected = name === agent.name

  return (
    <Link
      to={`/agents/${encodeURIComponent(agent.name)}`}
      className={`row agent-row ${selected ? 'selected' : ''}`}
    >
      <div className="row-line">
        <span className={`dot ${agent.online ? 'online' : ''}`} />
        <span
          className={`agent-name ${agent.online ? 'online' : 'offline'}`}
          data-testid="agent-name"
        >
          {agent.name}
        </span>
        {agent.isHuman && <span className="badge-human">human</span>}
        <div className="spacer" />
        <VolumeStrip buckets={agent.buckets} variant="rail" />
        <span className="agent-age" data-testid="agent-age">
          {age(agent.lastSeen, now)}
        </span>
      </div>
    </Link>
  )
}
