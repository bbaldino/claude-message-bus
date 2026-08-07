import { useEffect, useState } from 'react'
import { fetchDeletionPreview } from '../data/api'
import type { DeletionPreview } from '../types/DeletionPreview'
import { useStore } from '../useStore'
import styles from './DeleteModal.module.css'

export function DeleteModal({
  name,
  onClose,
  onDeleted,
}: {
  name: string
  onClose: () => void
  onDeleted: () => void
}) {
  const { rail } = useStore()
  const [preview, setPreview] = useState<DeletionPreview | null>(null)
  const [typed, setTyped] = useState('')
  const [failed, setFailed] = useState<string | null>(null)
  // Latched by a 409 from the DELETE itself — the server's word overrides
  // whatever the client believed going in. Cleared only by the presence
  // transition below, never just by re-opening.
  const [refused, setRefused] = useState(false)
  const matches = typed === name

  // Presence, read off the rail the websocket already keeps current — not a
  // poll, not a second socket. This is what makes the live-watch strip's
  // claim true.
  const liveNow = rail?.agents.find((a) => a.name === name)?.online
  const showRefused = refused || (preview?.online ?? false)

  useEffect(() => {
    // Counted at dialog open, not at page load: the screen behind this may be
    // minutes old, and a stale count in a delete confirmation is worse than none.
    setPreview(null)
    setFailed(null)
    setRefused(false)
    fetchDeletionPreview(name)
      .then(setPreview)
      .catch((e: unknown) => setFailed(e instanceof Error ? e.message : String(e)))
  }, [name])

  useEffect(() => {
    // The strip says "this dialog updates itself". Make that true: when presence
    // reports the agent has gone offline, re-read the counts — it may have joined
    // or left rooms on its way out — and drop the server-refused latch.
    if (liveNow === false && showRefused) {
      setRefused(false)
      fetchDeletionPreview(name)
        .then(setPreview)
        .catch(() => {})
    }
  }, [liveNow, showRefused, name])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  const submit = async () => {
    if (!matches) return
    try {
      const res = await fetch(`/api/agents/${encodeURIComponent(name)}`, { method: 'DELETE' })
      if (res.status === 204) onDeleted()
      // The bus is the authority on whether the agent is still connected — a
      // 409 renders the refused state regardless of what the client believed.
      else if (res.status === 409) setRefused(true)
      else setFailed(`the bus refused: ${res.status}`)
    } catch (e: unknown) {
      setFailed(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className={styles.scrim} onClick={onClose}>
      <div
        className={styles.modal}
        role="dialog"
        aria-modal="true"
        onClick={(e) => e.stopPropagation()}
      >
        <header className={styles.header}>
          {showRefused ? (
            <div className={styles.titleRow}>
              <h2 className={styles.title}>Still connected</h2>
              <span className={styles.pillOnline}>online</span>
            </div>
          ) : (
            <h2 className={styles.title}>Delete this agent?</h2>
          )}
          <p className={styles.subtitle}>
            {showRefused ? (
              // State the mechanism, not just the rule: this is *why* the bus
              // refuses, not a restatement that it does.
              <>
                <code>{name}</code> is still connected. Deleting a live agent would strip its
                memberships underneath it and it would re-register on its next heartbeat, so the bus
                refuses.
              </>
            ) : (
              <>
                Irreversible. The bus keeps no record of a deleted agent beyond the{' '}
                <code>agent_deleted</code> event.
              </>
            )}
          </p>
        </header>
        <div className={styles.body}>
          {showRefused ? (
            <>
              <p className={styles.willLabel}>to remove it</p>
              <div className={styles.steps}>
                <span className={styles.stepNum}>1.</span>
                <span className={styles.stepText}>
                  Stop the Claude Code session in <code>~/src/claude-bus</code> on hardac.
                </span>
                <span className={styles.stepNum}>2.</span>
                <span className={styles.stepText}>
                  Wait for the bus to mark it offline — one missed heartbeat, about 30 seconds.
                </span>
                <span className={styles.stepNum}>3.</span>
                <span className={styles.stepText}>
                  Delete becomes available on this page. Nothing to come back to; it will tell you.
                </span>
              </div>
              <div className={styles.watchStrip}>
                <span className={styles.watchDot} />
                <span className={styles.watchText}>
                  watching presence · this dialog updates itself
                </span>
              </div>
            </>
          ) : (
            <>
              <div className={styles.nameEcho}>{name}</div>
              <p className={styles.willLabel}>will be removed</p>
              {/* A failed preview offers no button: an empty blast radius and a
                  failed read must not look the same. */}
              {failed && !preview && (
                <p className={styles.failed}>could not read the blast radius: {failed}</p>
              )}
              {preview && (
                <div className={styles.counts}>
                  <span className={styles.count}>{preview.registration}</span>
                  <span className={styles.label}>
                    agent registration <span className={styles.secondary}>on {preview.host}</span>
                  </span>
                  <span className={styles.count}>{preview.memberships}</span>
                  <span className={styles.label}>room memberships</span>
                  <span className={styles.count}>{preview.cursors}</span>
                  <span className={styles.label}>read cursors</span>
                  <span className={styles.kept}>—</span>
                  <span className={styles.kept}>
                    messages and files are kept; they belong to the room
                  </span>
                </div>
              )}
              <p className={styles.willLabel}>type the name to confirm</p>
              <input
                className={styles.input}
                data-testid="confirm-input"
                value={typed}
                onChange={(e) => setTyped(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && submit()}
                // Puts the cursor where the operator needs it and, just as
                // importantly, moves focus off the page behind the scrim and
                // into the dialog. Not a full focus trap — deliberately: this
                // modal has no established focus-trap convention in this
                // codebase, and one is more machinery than a single-input dialog
                // warrants. A keyboard user can still tab out to the page behind
                // the scrim; recorded as a known gap, not an oversight.
                autoFocus
              />
              <p className={styles.progress}>
                {matches
                  ? 'matches'
                  : `${Math.max(0, name.length - typed.length)} characters to go`}
              </p>
            </>
          )}
        </div>
        <footer className={styles.footer}>
          {showRefused ? (
            <>
              <button className={styles.cancel} onClick={onClose}>
                close · esc
              </button>
              <span className={styles.noAction}>no delete action offered</span>
            </>
          ) : (
            <>
              <button className={styles.cancel} onClick={onClose}>
                cancel · esc
              </button>
              <button className={styles.delete} disabled={!matches || !preview} onClick={submit}>
                delete
              </button>
            </>
          )}
        </footer>
      </div>
    </div>
  )
}
