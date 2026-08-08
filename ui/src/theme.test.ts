import { expect, test } from 'vitest'

// @ts-expect-error — node modules not typed without @types/node, but they
// exist at runtime in Node test environment.
const { readFileSync } = await import('node:fs')
// @ts-expect-error
const { fileURLToPath } = await import('node:url')
// @ts-expect-error
const { dirname, join } = await import('node:path')

// import.meta.url isn't a file:// URL under vitest's module transform (unlike
// plain Node), so `new URL(...)` from the sketch throws "must be of scheme
// file" — go through fileURLToPath/dirname/join instead, the same route
// DeleteModal.test.tsx uses to read its own stylesheet.
const __dirname = dirname(fileURLToPath(import.meta.url))
const css = readFileSync(join(__dirname, './theme.css'), 'utf8')

/// Every custom property declared in :root, in declaration order.
function tokensIn(block: string): string[] {
  return [...block.matchAll(/^\s*(--[a-z0-9-]+):/gm)].map((m) => m[1])
}

// NOTE: the sketch this was based on located a block with `css.indexOf(selector)`
// alone. theme.css's own header comment contains the literal text
// "[data-theme='light']" (it explains where the light block will live), so a bare
// indexOf matches that comment instead of the real rule and then slices all the
// way to :root's closing brace — silently checking the wrong tokens. Matching on
// `${selector} {` (the exact way both rules are written) finds the real rule.
function block(selector: string): string {
  const marker = `${selector} {`
  const start = css.indexOf(marker)
  if (start === -1) throw new Error(`no ${selector} block`)
  return css.slice(start, css.indexOf('}', start))
}

test('light mode redefines every colour token, and only colour tokens', () => {
  const root = tokensIn(block(':root'))
  const light = tokensIn(block("[data-theme='light']"))

  // Non-colour tokens are theme-independent and must NOT be repeated — a
  // duplicated width is a value that can drift between themes for no reason.
  const structural = root.filter((t) => /^--(font|rail|topbar|dock)-/.test(t))
  expect(structural.length).toBeGreaterThan(0)
  for (const t of structural) expect(light).not.toContain(t)

  // Every colour token must have a light value. A token that only exists in
  // dark renders as its dark value on white, which is the failure this test
  // exists to catch — and it is invisible until someone looks at that screen.
  const colours = root.filter((t) => !structural.includes(t))
  const missing = colours.filter((t) => !light.includes(t))
  expect(missing).toEqual([])
})

test('no token is declared twice within a block', () => {
  for (const sel of [':root', "[data-theme='light']"]) {
    const names = tokensIn(block(sel))
    expect(new Set(names).size).toBe(names.length)
  }
})
