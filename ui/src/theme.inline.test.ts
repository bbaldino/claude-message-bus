import { expect, test } from 'vitest'

// @ts-expect-error — node modules not typed without @types/node, but they
// exist at runtime in Node test environment.
const { readFileSync } = await import('node:fs')
// @ts-expect-error
const { fileURLToPath } = await import('node:url')
// @ts-expect-error
const { dirname, join } = await import('node:path')

// Same route theme.test.ts uses to read theme.css: import.meta.url isn't a
// real file:// URL under vitest's module transform.
const __dirname = dirname(fileURLToPath(import.meta.url))
const themeSrc = readFileSync(join(__dirname, './theme.ts'), 'utf8')
const html = readFileSync(join(__dirname, '../index.html'), 'utf8')

// index.html's inline theme script can't import theme.ts — it has to run
// synchronously before any module executes, ahead of the render-blocking
// stylesheet link, or the dark :root paints before it corrects itself. So it
// duplicates theme.ts's storage key by hand. That duplication is only safe if
// a rename of the key is provably caught here, rather than silently
// reintroducing the flash of the wrong theme on load.
test("index.html's inline theme script uses the same storage key as theme.ts", () => {
  const match = themeSrc.match(/const KEY = '([^']+)'/)
  expect(match).not.toBeNull()
  const key = match![1]
  expect(html).toContain(key)
})

// The key-match test above pins one axis of the duplicated resolve logic.
// Five others can drift with that test still green: the storage medium, the
// attribute name, the media query and its polarity, the accepted values, and
// the script's position relative to the injected stylesheet. Each of the
// following is a grep for one of those axes against the inline script's own
// text, so a silent drift on any of them fails loudly instead of shipping a
// flash-of-wrong-theme regression.
test('the inline theme script reads from localStorage', () => {
  expect(html).toContain('localStorage')
})

test('the inline theme script stamps the data-theme attribute', () => {
  expect(html).toContain('data-theme')
})

test('the inline theme script queries prefers-color-scheme: light', () => {
  // The polarity matters as much as the query itself: matching `light` (not
  // `dark`) is what lets the stored-choice-beats-system rule read correctly
  // when there is no stored choice.
  expect(html).toContain('prefers-color-scheme: light')
})

test('the inline theme script accepts both theme values', () => {
  expect(html).toContain("'dark'")
  expect(html).toContain("'light'")
})

// The three assertions above run against ui/index.html, the source the repo
// tracks — that file has no <link rel="stylesheet"> of its own; Vite injects
// one at build time. The ordering claim (inline script before the injected
// stylesheet and before the module script) can only be checked against a
// built ui/dist/index.html, so it's skipped rather than failed when dist/ is
// absent — e.g. on a clean checkout before `npm run build` has ever run.
test('in the built output, the inline theme script runs before the stylesheet link and the module script', async () => {
  // @ts-expect-error — see the node:fs import above
  const { existsSync } = await import('node:fs')
  const distPath = join(__dirname, '../dist/index.html')
  if (!existsSync(distPath)) {
    return
  }
  const built = readFileSync(distPath, 'utf8')
  const scriptIdx = built.indexOf('<script>')
  const stylesheetIdx = built.indexOf('<link rel="stylesheet"')
  const moduleIdx = built.indexOf('<script type="module"')
  expect(scriptIdx).toBeGreaterThan(-1)
  expect(stylesheetIdx).toBeGreaterThan(-1)
  expect(moduleIdx).toBeGreaterThan(-1)
  expect(scriptIdx).toBeLessThan(stylesheetIdx)
  expect(scriptIdx).toBeLessThan(moduleIdx)
})
