import { useSyncExternalStore } from 'react'
import { createLive } from './data/live'
import { createStore } from './data/store'
import { fetchRail } from './data/api'

// One store for the whole app. Components subscribe; nothing fetches on its own,
// which is the property that stops two views disagreeing about what is current.
const wsUrl = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`
export const store = createStore({ live: createLive(wsUrl), fetchRail })

export function useStore() {
  return useSyncExternalStore(store.subscribe, store.getState)
}
