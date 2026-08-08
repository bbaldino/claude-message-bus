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
