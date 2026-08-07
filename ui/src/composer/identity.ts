const NAME_KEY = 'claude-bus.sendAs'

/// Guarded exactly like `readDockOpen` in the store, and for the same reason:
/// private browsing and any storage-blocking policy make `localStorage` throw
/// rather than return null, and an unguarded throw at module scope takes the
/// whole app down before React or the error boundary can render.
export function readSendAs(): string | null {
  try {
    return localStorage.getItem(NAME_KEY)
  } catch {
    return null
  }
}

export function writeSendAs(name: string): void {
  try {
    localStorage.setItem(NAME_KEY, name)
  } catch {
    // A name that cannot be persisted still works for this tab; the operator is
    // simply asked again next time. Failing the send over it would be worse.
  }
}
