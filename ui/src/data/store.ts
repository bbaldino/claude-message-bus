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
  // Distinguishes "no messages" from "haven't found out yet" and "couldn't
  // find out" — `messages.length === 0` alone is true in all three cases
  // (still loading, the fetch failed, or the room is genuinely empty), and a
  // transcript rendering "Nothing said here yet." from that alone would state
  // a fact the console does not actually know, on every room open and every
  // failed fetch.
  roomLoad: 'loading' | 'ready' | 'failed'
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
    roomLoad: 'loading',
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

  // Repairs the open room's transcript on the transition *into* `'live'` —
  // not on the value, so a re-render that is already `'live'` (the second of
  // two back-to-back pushes, say) must not repair anything a second time.
  // `repairRoom` (defined below, alongside `loadRoomInto`) is looked up at
  // call time, once the connection genuinely flips, by which point the
  // `const` further down in this function body has long since run — the
  // forward reference here is only to where it's *written*, not to when it's
  // evaluated.
  deps.live.on('connection', (p) => {
    const next = p as Connection
    const wasLive = state.connection === 'live'
    setState({ connection: next })
    if (!wasLive && next === 'live') repairRoom()
  })

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

  // The actual fetch-and-apply behind both a fresh room selection and a
  // reconnect repair (see `repairRoom`). `mine` is the `roomGeneration` this
  // load belongs to — captured by the caller, not read fresh here, so a
  // `selectRoom` snapshots the value it bumped to while a `repairRoom` (which
  // never bumps the counter — it isn't a new selection) snapshots whatever is
  // still current. Either way, the same check below discards a result that a
  // newer selection has since superseded.
  const loadRoomInto = async (name: string, mine: number) => {
    try {
      const [messages, roomEvents] = await Promise.all([
        deps.fetchMessages(name, PAGE),
        deps.fetchEvents({ room: name, limit: 500 }),
      ])
      // A second selection may have landed while these were in flight; its own
      // fetches own the state, not ours.
      if (mine !== roomGeneration) return
      setState({
        messages,
        roomEvents,
        hasMoreHistory: messages.length === PAGE,
        roomLoad: 'ready',
      })
    } catch {
      // Leave the empty transcript rather than a stale one. The connection pill
      // already reports trouble. Guarded the same way the success path is: a
      // superseded load must not overwrite the state a newer selection owns.
      if (mine === roomGeneration) setState({ roomLoad: 'failed' })
    }
  }

  // Retries the open room's load on a reconnect. Gated on `roomLoad ===
  // 'failed'` rather than firing on every reconnect: a healthy reconnect
  // finds a transcript that already loaded, and throwing a good one away to
  // re-fetch it is exactly the yank the "N new below" affordance elsewhere in
  // this app exists to avoid causing. A failed load, by contrast, has nothing
  // to lose — the transcript is already showing a failure, so replacing it
  // (on success) or leaving it (on another failure) are the only two
  // outcomes, both fine.
  //
  // No `roomGeneration` bump here, deliberately: this isn't a new selection,
  // it's a retry of the current one, so it reuses whatever generation is
  // already current — same pattern `loadOlder` uses, for the same reason.
  // That's what lets a room switch racing this repair win: `selectRoom` bumps
  // the generation on its own, synchronous path, so by the time this repair's
  // `loadRoomInto` checks it the switch has already invalidated it.
  const repairRoom = () => {
    const { room, roomLoad } = state
    if (!room || roomLoad !== 'failed') return
    void loadRoomInto(room, roomGeneration)
  }

  // Shared by the poll timer and by any explicit caller (the delete modal, for
  // one) that wants the rail to reflect a change it just made rather than wait
  // out the rest of the 25s interval. Failure is intentionally silent here too:
  // an explicit refresh that fails leaves the previous rail in place, same as a
  // missed poll tick, and the poll interval remains the backstop.
  const refreshRail = async () => {
    try {
      setState({ rail: await deps.fetchRail() })
    } catch {
      // Leave the previous rail in place; the connection pill already reports
      // trouble, and blanking the rail would read as an empty fleet.
    }
  }

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
        // Left 'loading' on the `!name` early return below — there is no room
        // to have loaded, so neither 'ready' nor 'failed' would be true.
        roomLoad: 'loading',
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
      await loadRoomInto(name, mine)
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
      await refreshRail()
      // A `stop()` (or a newer `start()`) ran while the fetch above was in
      // flight. This run has been superseded; installing an interval here
      // would be the leaked one nothing ever clears.
      if (myGeneration !== generation) return
      timer = setInterval(refreshRail, 25_000)
    },
    stop() {
      generation++
      if (timer) {
        clearInterval(timer)
        timer = null
      }
      deps.live.stop()
    },
    // Explicit ask, not a duplicate fetch: reuses the same refreshRail the
    // poll timer already calls, so there is exactly one code path that knows
    // how to fetch and apply the rail. Callers that just made a change the
    // rail should reflect (a completed delete) call this instead of waiting
    // out the rest of the poll interval; it fails soft the same way the timer
    // does, and the poll remains the backstop if it fails.
    refreshRail,
  }
}
