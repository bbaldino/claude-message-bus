import { useSyncExternalStore } from 'react'
import { createLive } from './data/live'
import { createParticipant } from './data/participant'
import { createStore } from './data/store'
import { fetchRail, fetchMessages, fetchEvents, setRoomHidden } from './data/api'

// One store for the whole app. Components subscribe; nothing fetches on its own,
// which is the property that stops two views disagreeing about what is current.
const wsUrl = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`
export const store = createStore({
  live: createLive(wsUrl),
  fetchRail,
  fetchMessages,
  fetchEvents,
  participant: createParticipant(wsUrl),
  setRoomHidden,
})

export function useStore() {
  return useSyncExternalStore(store.subscribe, store.getState)
}
