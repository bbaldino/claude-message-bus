import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { fetchDeletionPreview } from '../data/api'
import type { DeletionPreview } from '../types/DeletionPreview'
import { store, useStore } from '../useStore'
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
  // Distinct from `failed` above: that one is "the blast-radius read failed",
  // which withholds the button entirely. This one is "the delete itself
  // failed" — the button was live, the operator used it, and the request did
  // not succeed. The two mean different things (retry the read vs. retry the
  // delete, or in the 500 case, go look at the bus) so they get separate
  // state and separate copy rather than being folded into one.
  const [deleteError, setDeleteError] = useState<string | null>(null)
  // Guards against a second click firing a second DELETE while the first is
  // still in flight. The registry lock would serialise them anyway, but the
  // second call would then find the row already gone and — absent the
  // server-side fix alongside this — write a second `agent_deleted` event
  // for a delete that removed nothing.
  const [submitting, setSubmitting] = useState(false)
  // Latched by a 409 from the DELETE itself — the server's word overrides
  // whatever the client believed going in. Cleared only by the presence
  // transition below, never just by re-opening.
  const [refused, setRefused] = useState(false)
  const matches = typed === name

  // Presence, read off the rail the websocket already keeps current — not a
  // poll, not a second socket. This is what makes the live-watch strip's
  // claim true. Also the source for the refused state's real host, below.
  const railAgent = rail?.agents.find((a) => a.name === name)
  const liveNow = railAgent?.online
  const showRefused = refused || (preview?.online ?? false)
  // `preview` is the primary source (it is what the confirmable state's own
  // counts render from), with the rail as a fallback for the one window where
  // `preview` can still be null while `showRefused` is already true: an
  // already-online agent whose dialog just opened, where the liveNow-driven
  // effect below can flip `refused` before the preview fetch has resolved.
  const host = preview?.host ?? railAgent?.host

  useEffect(() => {
    // Counted at dialog open, not at page load: the screen behind this may be
    // minutes old, and a stale count in a delete confirmation is worse than none.
    // No reset of `preview`/`failed`/`refused` here: this component is only ever
    // mounted fresh (see `AgentScreen`'s `{deleting && <DeleteModal .../>}` and,
    // upstream of that, the agent route's `key={name}`, which guarantees `name`
    // itself never changes under a mounted instance) so the initial state from
    // `useState` above is already the reset state.
    fetchDeletionPreview(name)
      .then(setPreview)
      .catch((e: unknown) => setFailed(e instanceof Error ? e.message : String(e)))
  }, [name])

  useEffect(() => {
    // The strip says "this dialog updates itself" — in both directions, not just
    // going offline. Presence reporting offline drops the refused latch and
    // re-reads the counts (the agent may have joined or left rooms on its way
    // out). Presence reporting back online re-latches refused, driven by the
    // live value rather than the (possibly stale) preview snapshot — otherwise
    // an operator could sit looking at a live delete button for an agent that
    // reconnected after the dialog opened. The 409 path is untouched by this:
    // it is still the authority, this just reduces how often it has to be.
    if (liveNow === false && showRefused) {
      setRefused(false)
      fetchDeletionPreview(name)
        .then(setPreview)
        .catch(() => {})
    } else if (liveNow === true && !showRefused) {
      setRefused(true)
    }
  }, [liveNow, showRefused, name])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  // The node this dialog portals into, and — while mounted — the boundary
  // `inert` is applied outside of. A real focus trap was deliberately declined
  // for this modal (see the input's `autoFocus` comment, now removed along
  // with the gap it documented); `inert` gets the same result — nothing behind
  // the scrim is reachable by Tab, by a screen reader, or by a click — for far
  // less machinery, and as a side effect it is also what stops a Shift-Tab
  // journey from ending on a rail link and silently re-targeting this dialog
  // at a different agent.
  const portalNode = useRef<HTMLDivElement | null>(null)
  if (!portalNode.current) {
    portalNode.current = document.createElement('div')
  }

  useEffect(() => {
    const node = portalNode.current!
    document.body.appendChild(node)
    const siblings = Array.from(document.body.children).filter((el) => el !== node)
    for (const el of siblings) el.setAttribute('inert', '')
    return () => {
      for (const el of siblings) el.removeAttribute('inert')
      document.body.removeChild(node)
    }
  }, [])

  const submit = async () => {
    // Both halves of the guard live in the action, not on the control: a
    // control-only guard (the button's `disabled`) does not stop the Enter
    // key in the input from calling this directly, which is exactly how this
    // modal used to let a failed preview read through to a real DELETE.
    if (!matches || !preview || submitting) return
    setSubmitting(true)
    setDeleteError(null)
    try {
      const res = await fetch(`/api/agents/${encodeURIComponent(name)}`, { method: 'DELETE' })
      if (res.status === 204) {
        // Ask the store to refresh the rail rather than fetching it here: the
        // store already owns the rail and re-fetches it on a 25s timer, and
        // duplicating that fetch would be a third component reaching past it.
        // Not awaited — getting the operator to a correct console quickly
        // matters more than getting there atomically, and a spinner after a
        // successful delete would be the wrong trade. Failure is silent, same
        // as the poll's own: it leaves the previous rail in place rather than
        // blank it, and the 25s poll remains the backstop either way.
        void store.refreshRail()
        onDeleted()
        return
      }
      // The bus is the authority on whether the agent is still connected — a
      // 409 renders the refused state regardless of what the client believed.
      if (res.status === 409) {
        setRefused(true)
        return
      }
      // Anything else — a 404 (a concurrent delete already won), a 500, some
      // other status — is a delete that did not happen, and now says so:
      // this used to fall into `failed`, which only ever rendered while
      // `preview` was null, and submitting requires `preview` to be non-null.
      // The dialog looked unchanged after the only irreversible action on the
      // page failed.
      setDeleteError(`the bus returned ${res.status}`)
    } catch (e: unknown) {
      setDeleteError(e instanceof Error ? e.message : String(e))
    } finally {
      setSubmitting(false)
    }
  }

  return createPortal(
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
                  {/* The real host, not a fabricated one: agent names carry `@host`
                      suffixes precisely because agents live on different machines,
                      and this is the one state whose entire job is accurate
                      remediation. `preview.host` (with the rail as a fallback — see
                      `host` above) is what is in scope here; the working directory
                      would need threading a new prop from `AgentScreen` down through
                      to here for a value this component otherwise has no reason to
                      know, which is more machinery than this clause is worth, so it
                      is dropped rather than left fabricated. */}
                  Stop the Claude Code session
                  {host ? (
                    <>
                      {' '}
                      on <code>{host}</code>
                    </>
                  ) : (
                    ''
                  )}
                  .
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
                // Puts the cursor where the operator needs it. A full focus trap
                // is unnecessary here: `inert` on everything outside the portal
                // node (see above) already keeps Tab, Shift-Tab, and assistive
                // tech from ever reaching the page behind the scrim.
                autoFocus
              />
              <p className={styles.progress}>
                {matches
                  ? 'matches'
                  : `${Math.max(0, name.length - typed.length)} characters to go`}
              </p>
              {deleteError && <p className={styles.failed}>the delete failed: {deleteError}</p>}
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
              <button
                className={styles.delete}
                disabled={!matches || !preview || submitting}
                onClick={submit}
              >
                delete
              </button>
            </>
          )}
        </footer>
      </div>
    </div>,
    portalNode.current,
  )
}
