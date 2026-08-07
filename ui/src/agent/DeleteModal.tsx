import { useEffect, useState } from 'react'
import { fetchDeletionPreview } from '../data/api'
import type { DeletionPreview } from '../types/DeletionPreview'
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
  const [preview, setPreview] = useState<DeletionPreview | null>(null)
  const [typed, setTyped] = useState('')
  const [failed, setFailed] = useState<string | null>(null)
  const matches = typed === name

  useEffect(() => {
    // Counted at dialog open, not at page load: the screen behind this may be
    // minutes old, and a stale count in a delete confirmation is worse than none.
    setPreview(null)
    setFailed(null)
    fetchDeletionPreview(name)
      .then(setPreview)
      .catch((e: unknown) => setFailed(e instanceof Error ? e.message : String(e)))
  }, [name])

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
          <h2 className={styles.title}>Delete this agent?</h2>
          <p className={styles.subtitle}>
            Irreversible. The bus keeps no record of a deleted agent beyond the{' '}
            <code>agent_deleted</code> event.
          </p>
        </header>
        <div className={styles.body}>
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
            {matches ? 'matches' : `${Math.max(0, name.length - typed.length)} characters to go`}
          </p>
        </div>
        <footer className={styles.footer}>
          <button className={styles.cancel} onClick={onClose}>
            cancel · esc
          </button>
          <button className={styles.delete} disabled={!matches || !preview} onClick={submit}>
            delete
          </button>
        </footer>
      </div>
    </div>
  )
}
