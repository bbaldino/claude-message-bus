import { useEffect, useRef, useState } from 'react'
import { fetchMeta } from './data/api'
import type { Theme } from './theme'
import { resolveTheme, setTheme } from './theme'
import type { Meta } from './types/Meta'
import { isTypingTarget } from './ui/platform'
import { useStore } from './useStore'
import styles from './TopBar.module.css'

type Props = {
  // Optional and uncontrolled by default so a bare `<TopBar />` (as the
  // existing tests render it) still works; `Shell` supplies both to make it a
  // controlled field shared with `Rail`.
  value?: string
  onChange?: (value: string) => void
}

export function TopBar({ value = '', onChange = () => {} }: Props) {
  const { connection } = useStore()
  // The generated type, not a hand-written equivalent — see Global Constraints.
  const [meta, setMeta] = useState<Meta | null>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  // Initialised from resolveTheme rather than a fixed 'dark': main.tsx already
  // applied it to the document before this ever rendered, but the button's own
  // label has to agree with that or the two would disagree on first paint.
  const [theme, setThemeState] = useState<Theme>(() => resolveTheme())

  useEffect(() => {
    // Deliberate exception to "components subscribe to the store; nothing
    // fetches on its own" (see useStore.ts). Safe here specifically because
    // meta is static for the life of a session, has exactly one consumer (this
    // bar), and is fetched once on mount — there is no second view for it to
    // disagree with. That is not true of room history or events in the next
    // phase: anything with more than one consumer, or anything live, belongs
    // in the store, not a component-local fetch like this one.
    fetchMeta()
      .then(setMeta)
      .catch(() => setMeta(null))
  }, [])

  useEffect(() => {
    // "/" focuses search — but only when the user isn't already typing
    // somewhere, or they could never type a literal "/". `preventDefault` is
    // what stops the character landing in the field the moment it gains
    // focus; without it the browser's normal keypress handling still runs
    // after this handler and inserts it.
    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== '/') return
      if (isTypingTarget(e.target)) return
      e.preventDefault()
      searchRef.current?.focus()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [])

  return (
    <header className={styles.topbar}>
      <span className={styles.wordmark}>claude-bus</span>
      {meta && <span className={styles.hostPill}>{`${meta.host} · ${meta.version}`}</span>}
      <div className={styles.search}>
        <span className={styles.searchIcon} />
        <input
          ref={searchRef}
          className={styles.searchInput}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          // Rooms and agents filter client-side from the rail summary. Message
          // text has no endpoint, so the placeholder must not promise it.
          placeholder="search agents and rooms"
        />
        <span className={styles.searchKey}>/</span>
      </div>
      {/* The websocket state, not a decoration — the handoff is emphatic. */}
      <span className={`${styles.livePill} ${styles[connection]}`} data-testid="live-pill">
        <span className={styles.liveDot} />
        {connection}
      </span>
      {/* Labelled with the theme the click switches TO, not the one currently
          active — see the task report for why the handoff doesn't settle this
          and how it was decided. */}
      <button
        className={styles.themeToggle}
        onClick={() => {
          const next: Theme = theme === 'dark' ? 'light' : 'dark'
          setTheme(next)
          setThemeState(next)
        }}
      >
        {theme === 'dark' ? 'light' : 'dark'}
      </button>
    </header>
  )
}
