import { beforeEach, expect, test, vi } from 'vitest'
import { applyTheme, readTheme, resolveTheme, setTheme, systemTheme } from './theme'

function stubSystem(prefersLight: boolean) {
  vi.stubGlobal('matchMedia', (q: string) => ({
    matches: q.includes('light') ? prefersLight : !prefersLight,
    media: q,
    addEventListener: () => {},
    removeEventListener: () => {},
  }))
}

beforeEach(() => {
  // Unstub first: the previous test may have replaced `localStorage` itself
  // (see the storage-throws test below), and a stub with no `clear` method
  // would make the next line throw before the real one is restored.
  vi.unstubAllGlobals()
  localStorage.clear()
  document.documentElement.removeAttribute('data-theme')
})

test('with no stored choice, the system preference decides', () => {
  stubSystem(true)
  expect(readTheme()).toBeNull()
  expect(resolveTheme()).toBe('light')
})

test('a stored choice beats the system preference', () => {
  stubSystem(true)
  setTheme('dark')
  expect(resolveTheme()).toBe('dark')
})

test('applying a theme stamps the document, and dark is explicit', () => {
  applyTheme('light')
  expect(document.documentElement.getAttribute('data-theme')).toBe('light')
  // Dark must be stamped too, not left as "absence of light" — the CSS keys the
  // light block off the attribute, and a stale `light` attribute would survive.
  applyTheme('dark')
  expect(document.documentElement.getAttribute('data-theme')).toBe('dark')
})

test('an unreadable localStorage falls back to the system rather than throwing', () => {
  // Private browsing and any storage-blocking policy make localStorage throw
  // rather than return null. This runs during module init, so an unguarded
  // throw is a white screen before React or the error boundary can render —
  // the same hazard readDockOpen documents in the store.
  stubSystem(false)
  vi.stubGlobal('localStorage', {
    getItem() {
      throw new Error('blocked')
    },
    setItem() {
      throw new Error('blocked')
    },
  })
  expect(readTheme()).toBeNull()
  expect(resolveTheme()).toBe('dark')
  expect(() => setTheme('light')).not.toThrow()
})

test('systemTheme reports dark when the system asks for dark', () => {
  stubSystem(false)
  expect(systemTheme()).toBe('dark')
})
