import type { RailSummary } from '../types/RailSummary'
import type { Event } from '../types/Event'
import type { Message } from '../types/Message'
import type { FromBus } from '../types/FromBus'
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
  unwatchRoom(): void
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

  // Every push handler below narrows the frame against `FromBus`, the union
  // ts-rs generates from `src/proto.rs`. The single `as FromBus` sits at the
  // `unknown` boundary where the JSON arrives; every field access after the
  // `type` check is compiler-verified against the server's own definition. The
  // hand-written object literals these replaced were the reason a snake_case /
  // camelCase mismatch could reach the browser as a silent `undefined` three
  // times in one phase: a cast to a literal you wrote yourself agrees with you.
  //
  // The snake_case → camelCase normalisations stay, deliberately. They are not
  // the bug — the wire really is snake_case (`rename_all` on an enum renames
  // variant names, not variant fields) while the REST DTOs really are camelCase.
  // What changed is that the source shape is now checked rather than asserted.
  deps.live.on('event', (p) => {
    const frame = p as FromBus
    if (frame.type !== 'event') return
    const event: Event = {
      id: frame.id,
      kind: frame.kind,
      agent: frame.agent,
      room: frame.room,
      detail: frame.detail,
      createdAt: frame.created_at,
    }
    setState({ events: [event, ...state.events].slice(0, 500) })
  })

  deps.live.on('message', (p) => {
    const frame = p as FromBus
    if (frame.type !== 'message') return
    // Only the open room's traffic belongs in the transcript. The socket
    // accumulates a `Watch` per room the operator has visited and the protocol
    // has no `Unwatch`, so without this filter room A's messages land under room
    // B — in a list `selectRoom` has just cleared, which makes them look current.
    if (frame.room !== state.room) return
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
      id: frame.id,
      room: frame.room,
      from: frame.from,
      body: frame.text,
      done: frame.done,
      human: frame.human,
      createdAt: Date.now(),
    }
    // Appended, never scrolled to: the design requires a "3 new below" affordance
    // rather than yanking a reader who has scrolled up, so the scroll decision
    // belongs to the component owning the region.
    setState({ messages: [...state.messages, message] })
  })

  deps.live.on('presence', (p) => {
    const frame = p as FromBus
    if (frame.type !== 'presence') return
    const { name, online } = frame
    if (!state.rail) return
    setState({
      rail: {
        ...state.rail,
        agents: state.rail.agents.map((a) => (a.name === name ? { ...a, online } : a)),
      },
    })
  })

  let timer: ReturnType<typeof setInterval> | null = null
  // Bumped by every `start()` and `stop()` so an in-flight `start()` can tell,
  // once its `fetchRail` await resolves, whether it has since been superseded.
  // Without this, a `stop()` that lands while `timer` is still null (the fetch
  // hasn't finished, so no interval exists yet to clear) is a no-op, and the
  // `start()` that was already in flight installs its interval anyway — one
  // React StrictMode's double-invoked effect leaves running forever, because
  // the *next* `start()`'s interval overwrites the `timer` variable without
  // ever having cleared the first one.
  let generation = 0

  return {
    getState: () => state,
    setState,
    subscribe(fn: () => void) {
      subs.add(fn)
      return () => subs.delete(fn)
    },
    // `name: null` clears the selection — used when the route driving this
    // (see Shell.tsx) is no longer a room route at all, not just a different
    // room. That case isn't "switch rooms" (which `live.watchRoom` handles by
    // unwatching the old one as a side effect of watching the new one) — it's
    // "watch nothing", which needs its own call so the console never stays
    // subscribed to a room the operator has navigated away from.
    selectRoom(name: string | null) {
      setState({ room: name, messages: [] })
      if (name) {
        deps.live.watchRoom(name)
      } else {
        deps.live.unwatchRoom()
      }
    },
    async start() {
      const myGeneration = ++generation
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
      // A `stop()` (or a newer `start()`) ran while the fetch above was in
      // flight. This run has been superseded; installing an interval here
      // would be the leaked one nothing ever clears.
      if (myGeneration !== generation) return
      timer = setInterval(refresh, 25_000)
    },
    stop() {
      generation++
      if (timer) {
        clearInterval(timer)
        timer = null
      }
      deps.live.stop()
    },
  }
}
