import { expect, test } from 'vitest'

// @ts-expect-error — node modules not typed without @types/node, but they
// exist at runtime in Node test environment.
const { readFileSync, readdirSync, statSync } = await import('node:fs')
// @ts-expect-error
const { fileURLToPath } = await import('node:url')
// @ts-expect-error
const { dirname, join, relative } = await import('node:path')

// Same route theme.test.ts uses to read files relative to this test: import.meta.url
// isn't a real file:// URL under vitest's module transform.
const __dirname = dirname(fileURLToPath(import.meta.url))
const srcRoot = join(__dirname, '.')

// "theme.css is the only file allowed to contain a colour" was, until now, a
// rule enforced by a single file-scoped test (DeleteModal.test.tsx, for the
// modal's scrim/shadow) plus manual grep everywhere else — which means a new
// literal in any other component stylesheet ships silently. This walks every
// *.module.css under src/ and asserts none of them contain a hex colour or an
// rgb()/rgba() literal, making the rule structural instead of a promise kept
// by memory.
function moduleCssFiles(dir: string): string[] {
  const out: string[] = []
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    const st = statSync(full)
    if (st.isDirectory()) {
      out.push(...moduleCssFiles(full))
    } else if (entry.endsWith('.module.css')) {
      out.push(full)
    }
  }
  return out
}

// Several stylesheets legitimately quote a hex value in a comment explaining
// where a token's value came from (DeleteModal.module.css's handoff-mismatch
// note, Rail.module.css's selection-colour note) — those aren't literals in
// the CSS sense and must not fail this test. Strip comments before matching.
function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, '')
}

const HEX_LITERAL = /#[0-9a-fA-F]{3,8}\b/
const RGB_LITERAL = /rgba?\(/i

test('no *.module.css file outside theme.css contains a colour literal', () => {
  const files = moduleCssFiles(srcRoot)
  expect(files.length).toBeGreaterThan(0)

  const violations: string[] = []
  for (const file of files) {
    const code = stripComments(readFileSync(file, 'utf8'))
    if (HEX_LITERAL.test(code) || RGB_LITERAL.test(code)) {
      violations.push(relative(srcRoot, file))
    }
  }
  expect(violations).toEqual([])
})
