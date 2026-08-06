import type { RailSummary } from '../types/RailSummary'
import type { Event } from '../types/Event'
import type { Message } from '../types/Message'
import type { Connection } from './live'

export type State = {
  rail: RailSummary | null
  events: Event[]
  messages: Message[]
  room: string | null
  connection: Connection
}

type Live = {
  on(kind: string, fn: (payload: unknown) => void): void
  watchRoom(room: string): void
  start(): void
  stop(): void
}

/// One store rather than per-screen hooks: presence and events feed the rail, the
/// dock, the unseen badge and the transcript at once, and separate subscriptions
/// to one stream would let them disagree about what is current.
export function createStore(deps: { live: Live; fetchRail: () => Promise<RailSummary> }) {
  let state: State = {
    rail: null,
    events: [],
    messages: [],
    room: null,
    connection: 'reconnecting',
  }
  const subs = new Set<() => void>()
  const notify = () => subs.forEach((f) => f())

  const setState = (patch: Partial<State>) => {
    state = { ...state, ...patch }
    notify()
  }

  deps.live.on('connection', (p) => setState({ connection: p as Connection }))

  deps.live.on('event', (p) => {
    const push = p as {
      id: number
      kind: string
      agent: string | null
      room: string | null
      detail: unknown
      created_at: number
    }
    // FromBus carries only `rename_all = "snake_case"` on the enum, which renames
    // variant names but not variant fields, so FromBus::Event puts `created_at` on
    // the wire. The REST DTO (`Event.ts`) comes from a separate struct with
    // `rename_all = "camelCase"`, so it declares `createdAt`. Normalise here so
    // `state.events` is homogeneous rather than silently `undefined` for every
    // live-pushed event.
    const event: Event = {
      id: push.id,
      kind: push.kind,
      agent: push.agent,
      room: push.room,
      detail: push.detail,
      createdAt: push.created_at,
    }
    setState({ events: [event, ...state.events].slice(0, 500) })
  })

  deps.live.on('message', (p) => {
    const push = p as {
      id: number
      room: string
      from: string
      text: string
      done: boolean
      human: boolean
    }
    // FromBus::Message and the REST Message DTO are deliberately different
    // shapes: the push is the wire event (`text`), the DTO is the stored row
    // (`body`, `createdAt`). Normalise here so `state.messages` is homogeneous
    // and a transcript can render fetched and live messages identically.
    //
    // The push carries no timestamp, so this is the client's receipt time, not
    // the server's stored `created_at`. They differ by the network delay, and a
    // refetch of the room replaces the approximation with the authoritative
    // value. Do not present it as anything more precise than that.
    const message: Message = {
      id: push.id,
      room: push.room,
      from: push.from,
      body: push.text,
      done: push.done,
      human: push.human,
      createdAt: Date.now(),
    }
    // Appended, never scrolled to: the design requires a "3 new below" affordance
    // rather than yanking a reader who has scrolled up, so the scroll decision
    // belongs to the component owning the region.
    setState({ messages: [...state.messages, message] })
  })

  deps.live.on('presence', (p) => {
    // Destructured against the real wire shape (including the fields unused here)
    // so the snake_case mapping is visible at the point of use, not latent — see
    // the note on the event handler above for why FromBus is snake_case.
    const { name, online } = p as { name: string; host: string; online: boolean; last_seen: number }
    if (!state.rail) return
    setState({
      rail: {
        ...state.rail,
        agents: state.rail.agents.map((a) => (a.name === name ? { ...a, online } : a)),
      },
    })
  })

  let timer: ReturnType<typeof setInterval> | null = null

  return {
    getState: () => state,
    setState,
    subscribe(fn: () => void) {
      subs.add(fn)
      return () => subs.delete(fn)
    },
    selectRoom(name: string) {
      setState({ room: name, messages: [] })
      deps.live.watchRoom(name)
    },
    async start() {
      deps.live.start()
      const refresh = async () => {
        try {
          setState({ rail: await deps.fetchRail() })
        } catch {
          // Leave the previous rail in place; the connection pill already reports
          // trouble, and blanking the rail would read as an empty fleet.
        }
      }
      await refresh()
      timer = setInterval(refresh, 25_000)
    },
    stop() {
      if (timer) clearInterval(timer)
      deps.live.stop()
    },
  }
}
