import './VolumeStrip.css'

/// Bars scale against the strip's own tallest bucket, with an 8% floor so an
/// empty bucket is a visible tick rather than a gap — a flat strip must read as
/// "quiet", not as broken layout.
///
/// Deliberately not animated on update. The handoff is explicit that the strip is
/// scanned rather than watched, and motion in a 14px sparkline across eight rows
/// reads as noise.
export function VolumeStrip({
  buckets,
  variant,
}: {
  buckets: number[]
  variant: 'rail' | 'detail'
}) {
  const peak = Math.max(...buckets, 0)
  const minutes = buckets.length * 5
  const label =
    peak === 0
      ? `no messages in the last ${minutes} min`
      : `messages per 5 min · last ${minutes} min`

  return (
    <div className={`volume-strip ${variant}`} role="img" aria-label={label}>
      {buckets.map((n, i) => (
        <div
          key={i}
          className={`volume-bar ${n === 0 ? 'never' : 'active'}`}
          style={{ height: peak === 0 ? '8%' : `${Math.max(8, (n / peak) * 100)}%` }}
        />
      ))}
    </div>
  )
}
