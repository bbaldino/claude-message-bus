import { useEffect, useState } from 'react'

/// One interval for a whole subtree, not one per row. `intervalMs` is a literal
/// at every call site, so the effect's dependency never changes and no interval
/// accumulates across re-renders.
export function useTicker(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs)
    return () => clearInterval(id)
  }, [intervalMs])
  return now
}

/// Relative age, deliberately coarse: a scan target in a narrow column, not a
/// timestamp. Clamps at zero so clock skew between the bus host and the browser
/// cannot render a negative age.
export function age(lastSeen: number, now: number): string {
  const s = Math.max(0, Math.floor((now - lastSeen) / 1000))
  if (s < 60) return `${s}s`
  if (s < 3600) return `${Math.floor(s / 60)}m`
  if (s < 86_400) return `${Math.floor(s / 3600)}h`
  return `${Math.floor(s / 86_400)}d`
}
