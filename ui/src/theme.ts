export type Theme = 'dark' | 'light'

const KEY = 'claude-bus.theme'

/// What the operating system asks for. Only consulted when the operator has
/// made no choice of their own.
export function systemTheme(): Theme {
  return typeof matchMedia === 'function' && matchMedia('(prefers-color-scheme: light)').matches
    ? 'light'
    : 'dark'
}

/// The operator's stored choice, or null if they have never made one. Guarded
/// like the store's `readDockOpen`: private browsing and storage-blocking
/// policies make `localStorage` throw rather than return null, and this runs
/// during module init — an unguarded throw is a white screen with no React and
/// no error boundary to catch it.
export function readTheme(): Theme | null {
  try {
    const v = localStorage.getItem(KEY)
    return v === 'dark' || v === 'light' ? v : null
  } catch {
    return null
  }
}

/// A stored choice wins; otherwise follow the system. These are deliberately
/// different questions — someone who has chosen must not be overridden, and
/// someone who has not must not be guessed at.
export function resolveTheme(): Theme {
  return readTheme() ?? systemTheme()
}

/// Dark is stamped explicitly rather than left as the absence of `light`: the
/// stylesheet keys its light block off this attribute, so switching back has to
/// replace the value, not merely fail to set it.
export function applyTheme(t: Theme): void {
  document.documentElement.setAttribute('data-theme', t)
}

export function setTheme(t: Theme): void {
  applyTheme(t)
  try {
    localStorage.setItem(KEY, t)
  } catch {
    // A choice that cannot be persisted still applies to this tab; the operator
    // is simply asked again next time. Failing the switch over it would be worse.
  }
}
