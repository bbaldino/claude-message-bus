import type { StatusView } from '../types/StatusView'

// The twin of `src/status_text.rs`. The two must describe a status identically;
// statusText.test.ts pins the shared cases.
export function age(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  if (s < 60) return `${s}s`
  if (s < 3600) return `${Math.floor(s / 60)}m`
  if (s < 86400) {
    const m = Math.floor((s % 3600) / 60)
    return m === 0 ? `${Math.floor(s / 3600)}h` : `${Math.floor(s / 3600)}h${m}m`
  }
  return `${Math.floor(s / 86400)}d`
}

const WORD: Record<StatusView['state'], string> = {
  working: 'working',
  idle: 'idle',
  blocked_on_human: 'blocked on human',
  unknown: 'unknown',
}

export function statusText(s: StatusView, online: boolean, now: number): string {
  const since = age(now - s.since)
  // "Offline" and "quiet" are about how long since the agent was last heard from,
  // which is the later of the state change and the last heartbeat.
  const heard = age(now - Math.max(s.last_heartbeat ?? s.since, s.since))
  let out = !online
    ? `offline, was ${WORD[s.state]} ${heard} ago`
    : s.quiet
      ? `working? (quiet) ${heard}`
      : s.state === 'blocked_on_human' && s.reason
        ? `${WORD[s.state]} (${s.reason}) ${since}`
        : `${WORD[s.state]} ${since}`
  if (s.text) {
    out += `: ${s.text}`
    if (s.text_at !== null && s.since - s.text_at > 10 * 60_000) {
      out += ` (text from ${age(now - s.text_at)} ago)`
    }
  }
  return out
}
