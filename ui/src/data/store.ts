import type { RailSummary } from '../types/RailSummary'
import type { Event } from '../types/Event'
import type { Message } from '../types/Message'
import type { FromBus } from '../types/FromBus'
import type { Connection } from './live'

export type State = {
  rail: RailSummary | null
  events: Event[]
  roomEvents: Event[]
  messages: Message[]
  room: string | null
  connection: Connection
  hasMoreHistory: boolean
  loadingOlder: boolean
  dockOpen: boolean
}

type Live = {
  on(kind: string, fn: (payload: unknown) => void): void
  watchRoom(room: string): void
  unwatchRoom(): void
  start(): void
  stop(): void
}

const DOCK_OPEN_KEY = 'claude-bus.dockOpen'

/// Private browsing and blocked storage both make `localStorage` throw rather
/// than return null — Safari's private mode is the well-known case, but any
/// policy that blocks storage does the same. The read runs at module
/// evaluation (see `dockOpen` below, set while constructing the initial
/// `state`), so an unguarded throw here fails the whole module import: a
/// white screen with no React and no error boundary, before either has had a
/// chance to render. Default to closed on a read failure — the dock simply
/// starts closed, same as a first visit.
function readDockOpen(): boolean {
  try {
    return localStorage.getItem(DOCK_OPEN_KEY) === 'true'
  } catch {
    return false
  }
}

/// One store rather than per-screen hooks: presence and events feed the rail, the
/// dock, the unseen badge and the transcript at once, and separate subscriptions
/// to one stream would let them disagree about what is current.
export function createStore(deps: {
  live: Live
  fetchRail: () => Promise<RailSummary>
  fetchMessages: (room: string, limit?: number, before?: number) => Promise<Message[]>
  fetchEvents: (opts: { room?: string; kind?: string; limit?: number }) => Promise<Event[]>
}) {
  let state: State = {
    rail: null,
    events: [],
    roomEvents: [],
    messages: [],
    room: null,
    connection: 'reconnecting',
    hasMoreHistory: false,
    loadingOlder: false,
    // Defaults to false, per the design: the dock is closed until asked for.
    dockOpen: readDockOpen(),
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
    setState({
      events: [event, ...state.events].slice(0, 500),
      roomEvents:
        event.room && event.room === state.room
          ? [event, ...state.roomEvents].slice(0, 500)
          : state.roomEvents,
    })
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

  // Bumped by every `selectRoom()`, separately from `generation` above: that one
  // guards `start()`/`stop()` races against the rail poll, this one guards room
  // selection races against the history/events fetch. Sharing a counter would
  // make a rail refresh cancel an in-flight room load, and vice versa.
  let roomGeneration = 0

  const PAGE = 100

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
    async selectRoom(name: string | null) {
      setState({
        room: name,
        messages: [],
        roomEvents: [],
        hasMoreHistory: false,
        loadingOlder: false,
      })
      // Bumped on every path, including `null`: the generation marks "a new
      // selection has happened", and deselecting is a selection. Bumping it only
      // on the named path let a room load in flight at the time of a `null`
      // deselection sail through its own now-stale generation check once it
      // resolved, repopulating `messages`/`roomEvents` for a room the console had
      // just navigated away from.
      const mine = ++roomGeneration
      if (!name) {
        deps.live.unwatchRoom()
        return
      }
      deps.live.watchRoom(name)
      try {
        const [messages, roomEvents] = await Promise.all([
          deps.fetchMessages(name, PAGE),
          deps.fetchEvents({ room: name, limit: 500 }),
        ])
        // A second selection may have landed while these were in flight; its own
        // fetches own the state, not ours.
        if (mine !== roomGeneration) return
        setState({ messages, roomEvents, hasMoreHistory: messages.length === PAGE })
      } catch {
        // Leave the empty transcript rather than a stale one. The connection pill
        // already reports trouble.
      }
    },
    async loadOlder() {
      const { room, messages, hasMoreHistory, loadingOlder } = state
      if (!room || !hasMoreHistory || loadingOlder || messages.length === 0) return
      setState({ loadingOlder: true })
      const mine = roomGeneration
      try {
        const older = await deps.fetchMessages(room, PAGE, messages[0].id)
        if (mine !== roomGeneration) return
        setState({
          messages: [...older, ...state.messages],
          hasMoreHistory: older.length === PAGE,
        })
      } finally {
        if (mine === roomGeneration) setState({ loadingOlder: false })
      }
    },
    setDockOpen(open: boolean) {
      try {
        localStorage.setItem(DOCK_OPEN_KEY, String(open))
      } catch {
        // Storage is blocked or full; the in-memory state below still updates,
        // it just won't survive a reload.
      }
      setState({ dockOpen: open })
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
