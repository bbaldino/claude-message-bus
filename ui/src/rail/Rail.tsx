import { useStore } from '../useStore'

export function Rail() {
  const { rail } = useStore()
  return (
    <nav>
      {rail?.rooms.map((r) => (
        <div key={r.name}>{r.name}</div>
      ))}
    </nav>
  )
}
