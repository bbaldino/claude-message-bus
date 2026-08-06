import type { RailSummary } from '../types/RailSummary'
import type { Meta } from '../types/Meta'
import type { Message } from '../types/Message'
import type { Event } from '../types/Event'

async function getJson<T>(path: string): Promise<T> {
  const res = await fetch(path)
  // Never swallow a failure into an empty result: an empty rail renders as
  // "everything is quiet", which is the opposite of the truth when the API is down.
  if (!res.ok) throw new Error(`${path} returned ${res.status}`)
  return (await res.json()) as T
}

export const fetchRail = () => getJson<RailSummary>('/api/rail')
export const fetchMeta = () => getJson<Meta>('/api/meta')

export const fetchMessages = (room: string, limit = 100) =>
  getJson<Message[]>(`/api/rooms/${encodeURIComponent(room)}/messages?limit=${limit}`)

export const fetchEvents = (opts: { room?: string; kind?: string; limit?: number } = {}) => {
  const p = new URLSearchParams()
  if (opts.room) p.set('room', opts.room)
  if (opts.kind) p.set('kind', opts.kind)
  p.set('limit', String(opts.limit ?? 200))
  return getJson<Event[]>(`/api/events?${p}`)
}
