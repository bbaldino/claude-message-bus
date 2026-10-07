import { useState } from 'react'
import { useOutletContext, useSearchParams } from 'react-router-dom'
import { NewBus } from '../empty/NewBus'
import { useTicker } from '../ui/time'
import { useStore } from '../useStore'
import { ConversationRow } from './ConversationRow'
import { shapeInbox } from './inbox'
import styles from './Inbox.module.css'

/// The landing view: recent conversations, one row per room, filtered by the
/// `?agent=` URL parameter the rail's agent rows set.
/// `query` is normally the top bar's search, handed down by the Shell through the
/// router outlet; the prop lets a caller (or a test) supply it directly.
export function Inbox({ query: queryProp }: { query?: string } = {}) {
  const { rail } = useStore()
  const now = useTicker(1000)
  const [params, setParams] = useSearchParams()
  const agent = params.get('agent')
  // Outside the Shell (component tests) there is no outlet context.
  const outletQuery = useOutletContext<{ query: string } | undefined>()?.query
  const query = queryProp ?? outletQuery ?? ''
  const [showOlder, setShowOlder] = useState(false)
  const [showHidden, setShowHidden] = useState(false)

  // Before the rail loads we know nothing; the new-bus screen then would tell a
  // populated bus's owner that nothing has joined it.
  if (!rail) return null
  if (rail.agents.length === 0) return <NewBus />

  const { recent, older, hidden } = shapeInbox(rail.rooms, { now, agent, query })
  const trimmed = query.trim()

  return (
    <section className={styles.inbox}>
      <div className={styles.header}>
        <span className={styles.label}>
          conversations
          {agent !== null && (
            <>
              {' '}
              · with {agent}{' '}
              <button
                className={styles.clear}
                data-testid="inbox-filter-clear"
                onClick={() => setParams({})}
              >
                ✕ clear
              </button>
            </>
          )}
        </span>
        <span className={styles.count}>last 24h</span>
      </div>
      {recent.length === 0 && (
        <p className={styles.empty} data-testid="inbox-empty">
          {/* "match" only when the search found nothing at all; older or hidden
              matches still get the window wording, above their own expanders. */}
          {trimmed !== '' && older.length === 0 && hidden.length === 0
            ? `no conversations match "${trimmed}"`
            : agent !== null
              ? `no conversations with ${agent} in the last 24h`
              : 'no conversations in the last 24h'}
        </p>
      )}
      <div className={styles.rows}>
        {recent.map((r) => (
          <ConversationRow key={r.name} room={r} now={now} />
        ))}
      </div>
      {older.length > 0 && (
        <button
          className={styles.toggle}
          data-testid="inbox-older-toggle"
          onClick={() => setShowOlder(!showOlder)}
        >
          {showOlder ? '▾' : '▸'} {older.length} older conversation{older.length === 1 ? '' : 's'}
        </button>
      )}
      {showOlder && (
        <div className={styles.rows}>
          {older.map((r) => (
            <ConversationRow key={r.name} room={r} now={now} />
          ))}
        </div>
      )}
      {hidden.length > 0 && (
        <button
          className={styles.toggle}
          data-testid="inbox-hidden-toggle"
          onClick={() => setShowHidden(!showHidden)}
        >
          {showHidden ? '▾' : '▸'} {hidden.length} hidden
        </button>
      )}
      {showHidden && (
        <div className={styles.rows}>
          {hidden.map((r) => (
            <ConversationRow key={r.name} room={r} now={now} dimmed />
          ))}
        </div>
      )}
    </section>
  )
}
