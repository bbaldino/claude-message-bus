# Light Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The console renders in light mode, switched by the top bar's toggle, persisted, defaulting to the system preference.

**Architecture:** `ui/src/theme.css` already names every token by the role the handoff's own light-mode table uses, and its header states the intent: *"adding light mode later is one more block of values under `[data-theme='light']` and zero component changes."* This plan makes that true — first by tokenising the two `rgba()` literals that escaped into a component, then by adding the light block, then by wiring the toggle.

**Tech Stack:** React 19, TypeScript, Vite, vitest + jsdom + @testing-library/react, CSS Modules, CSS custom properties.

## Source of truth

There is no separate design spec for this phase. The design is
`docs/ui-design-pass/handoff/README.md` §"Light mode (5a–5h)" (lines 523–600),
which carries a full parallel token set. **Every value in this plan is copied
from those tables verbatim.** Where the handoff has no value, this plan says so
explicitly and states the decision taken — see "Three values the handoff does not
give" below.

## Global Constraints

- Every commit message MUST start with `chore:` — `feat:`/`fix:` triggers an automatic release via release-plz.
- Prettier config is exactly `{ "singleQuote": true, "semi": false, "printWidth": 100 }`. Run `npm run format` before committing.
- **No hex colours or `rgba()` outside `ui/src/theme.css`.** This phase is the one that makes that rule absolute; do not add new exceptions.
- **No component may branch on the theme.** No `data-theme` checks in TSX, no `.light`/`.dark` class variants in CSS Modules. If a component needs to differ between themes, that difference becomes a token whose value changes. This is the property that keeps the theme one file.
- No new npm dependencies. Never edit anything under `ui/src/types/` (ts-rs generates it; CI fails on drift).
- No `dangerouslySetInnerHTML`.
- The UI gate, from `ui/`: `npm test && npm run typecheck && npm run format:check && npm run build`.
- Component tests must cover the failure path, not only the success path.

## Three values the handoff does not give

Two tokens have no row in any light-mode table, and one appears to conflict.
Decisions taken here rather than left to an implementer:

| Token | Dark | Light | Reasoning |
|---|---|---|---|
| `--surface-control` | `#1a1e24` | `#f4f2ef` | Inset controls sitting on `--surface-raised` — the top bar's host pill and search field. The handoff's nearest role is "Recessed (input, code, scope bar)", which is exactly what these are. Same value as `--surface-recessed`. |
| `--border-key` | `#2c313a` | `#d5cfc7` | The `/` and `Ctrl E` key hints. No row; takes the emphasis-border value, the strongest neutral border, since a key cap must read as a distinct object. |
| `--text-placeholder` | `#59626e` | `#726c62` | **Resolves a contradiction 2b recorded.** The handoff's top-bar spec gives `#565e69` and its ramp table's Dim tier gives `#565e69` dark / `#726c62` light, listing "placeholders" among Dim's uses. 2b followed the more specific top-bar value (`#59626e`) for dark. For light the ramp table is the only source, so placeholders take Dim's `#726c62`. |

Record these three in the light block with a comment naming them as decisions,
not transcriptions, so the next reader knows they were not in the handoff.

## Two things the handoff describes that do not exist

Stated so no one builds them:

- **"Volume strips lose the glow (`box-shadow` on the presence dot)."** There is
  no `box-shadow` anywhere in `ui/src` outside the modal — grep confirms it. The
  glow was never built, so there is nothing to remove. Do not add one in order to
  remove it.
- **The volume strip "gains weight"** in light. Its three state colours
  (`--volume-idle`, `--volume-dead`, `--volume-never`) already have light values
  in the handoff's table and are applied by Task 2. Whether that reads heavily
  enough on white is a judgement only the manual pass can make; Task 4 looks at
  it explicitly rather than pre-emptively adding weight here.

## One thing the handoff describes that is already built

**The selected row's 2px accent edge already exists.** `Rail.module.css` puts
`border-left: 2px solid transparent` on every `.row` and
`border-left-color: var(--accent)` on `.row.selected`. That is exactly the shape
this plan would otherwise have prescribed: the border is on all rows, so
selection cannot shift content sideways, and the colour is already token-driven.
It needs no change and no new token — Task 2's light `--row-selected` and
`--accent` values are the whole of the work. Verified before planning rather than
discovered during it; an earlier draft of this plan added a redundant
`--row-selected-edge` token and a task to consume it.

---

## File Structure

| File | Responsibility |
|---|---|
| `ui/src/theme.css` | **Modify.** Gains `--modal-scrim`, `--modal-shadow`, and the whole `[data-theme='light']` block. |
| `ui/src/agent/DeleteModal.module.css` | **Modify.** Its two `rgba()` literals become tokens. |
| `ui/src/theme.ts` | **Create.** Resolve, apply, and persist the theme. Pure enough to test. |
| `ui/src/TopBar.tsx` | **Modify.** The toggle stops being `disabled` and switches the theme. |

---

### Task 1: Tokenise the last two colour literals

**Files:**
- Modify: `ui/src/theme.css`
- Modify: `ui/src/agent/DeleteModal.module.css:9-11,29`
- Test: `ui/src/agent/DeleteModal.test.tsx`

**Interfaces:**
- Produces: `--modal-scrim`, `--modal-shadow` in `theme.css`.

`DeleteModal.module.css` holds the only two colour literals outside `theme.css`. Its own comment explains why — there was no scrim token — and this task creates one. The handoff gives light values for both, so they cannot stay literals.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/agent/DeleteModal.test.tsx`:

```tsx
test('the scrim and shadow come from tokens, not literals', async () => {
  // theme.css is the only file allowed to contain a colour. This test pins that
  // for the modal specifically, because its scrim and shadow were the last two
  // literals in the tree and light mode gives both a different value.
  const css = await import('./DeleteModal.module.css?raw')
  expect(css.default).not.toMatch(/rgba\(/)
  expect(css.default).toMatch(/var\(--modal-scrim\)/)
  expect(css.default).toMatch(/var\(--modal-shadow\)/)
})
```

If `?raw` imports are not configured in this Vite/vitest setup, read the file with `node:fs` from the test instead — the assertion is what matters, not the import mechanism. Say in your report which you used.

- [ ] **Step 2: Run to verify it fails**

Run from `ui/`: `npm test -- --run DeleteModal`
Expected: FAIL — the stylesheet still contains `rgba(`.

- [ ] **Step 3: Add the tokens**

In `ui/src/theme.css`, in the `:root` block beside the other surface tokens:

```css
  /* The modal's scrim and shadow. Tokens rather than literals because light
     mode gives both a different value — and a warm one, not a darker one: the
     handoff's light scrim is rgba(48,42,32,.3), so the page behind stays
     legible as context rather than disappearing. */
  --modal-scrim: rgba(6, 7, 9, 0.72);
  --modal-shadow: 0 18px 50px rgba(0, 0, 0, 0.6);
```

- [ ] **Step 4: Use them**

In `ui/src/agent/DeleteModal.module.css`, replace the literal on line 11 with `background: var(--modal-scrim);` and the one on line 29 with `box-shadow: var(--modal-shadow);`. Rewrite the comment above the scrim — it currently explains that no scrim token exists, which stops being true.

- [ ] **Step 5: Run to verify it passes, then commit**

```bash
cd ui && npm run format && npm test && npm run typecheck && npm run format:check && npm run build
cd .. && git add ui/src
git commit -F - <<'EOF'
chore: tokenise the modal scrim and shadow

The last two colour literals outside theme.css. They were literals because no
scrim token existed; light mode gives both a different value, so the token has to
exist now. The light scrim is warm rather than darker — the page behind a modal
stays legible as context instead of disappearing.
EOF
```

---

### Task 2: The light token block

**Files:**
- Modify: `ui/src/theme.css`

**Interfaces:**
- Consumes: `--modal-scrim`, `--modal-shadow` from Task 1.
- Produces: a complete `[data-theme='light']` block. **No new token names** — every token already exists in `:root`, and this block only gives each one a light value.

Every value below is copied from the handoff's tables. Add the block **after** the `:root` block so it overrides by source order as well as specificity.

- [ ] **Step 1: Write the failing test**

Create `ui/src/theme.test.ts`:

```ts
import { readFileSync } from 'node:fs'
import { expect, test } from 'vitest'

const css = readFileSync(new URL('./theme.css', import.meta.url), 'utf8')

/// Every custom property declared in :root, in declaration order.
function tokensIn(block: string): string[] {
  return [...block.matchAll(/^\s*(--[a-z0-9-]+):/gm)].map((m) => m[1])
}

function block(selector: string): string {
  const start = css.indexOf(selector)
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
```

- [ ] **Step 2: Run to verify it fails**

Run from `ui/`: `npm test -- --run theme`
Expected: FAIL — `no [data-theme='light'] block`.

- [ ] **Step 3: Add the block**

Append to `ui/src/theme.css`, after `:root`:

```css
/* Light mode. NOT an inversion — every value is re-picked, because the dark
   theme carries state as saturated colour against near-black, and flipping
   lightness turns those muddy while dropping the neutrals below readable
   contrast. Values are the handoff's (§"Light mode (5a–5h)"), verbatim, except
   the three marked as decisions below.
   
   The text ramp holds a floor: every tier carrying text is at least 4.5:1 on
   white, timestamps most of all since they are the main scan target.
   Decorative-only tiers sit near 3.75:1. Hold that floor if you re-derive. */
[data-theme='light'] {
  /* Surfaces */
  --surface-page: #ffffff;
  --surface-rail: #f7f6f4;
  --surface-raised: #fdfcfb;
  --surface-recessed: #f4f2ef;
  --surface-modal-footer: #f4f2ef;
  /* DECISION, not in the handoff: no light row exists for the inset controls
     sitting on --surface-raised (the top bar's host pill and search field).
     They are inputs, so they take the recessed value. */
  --surface-control: #f4f2ef;

  /* Borders */
  --border-primary: #e3dfd9;
  --border-control: #ddd8d1;
  --border-emphasis: #d5cfc7;
  --border-hairline: #efece7;
  --border-rule: #eae6e0;
  /* DECISION, not in the handoff: the `/` and `Ctrl E` key caps. Takes the
     emphasis border, the strongest neutral, so a key reads as an object. */
  --border-key: #d5cfc7;

  /* Rows. The handoff gives one light hover for both dark values. */
  --row-hover: #f0eee9;
  --row-hover-dock: #f0eee9;
  /* Selection is a pale wash, not a lift — there is no elevation to work with
     on white. The 2px accent edge the handoff pairs with it is already in
     Rail.module.css and already token-driven, so it needs nothing here. */
  --row-selected: #eaf0fa;

  /* Text ramp */
  --text-primary: #16140f;
  --text-body-strong: #26231d;
  --text-body: #3a362f;
  --text-secondary: #3f3b34;
  --text-tertiary: #6b665d;
  --text-tertiary-dim: #6b665d;
  --text-quaternary: #7c766c;
  --text-quaternary-dim: #857f74;
  --text-label: #8a8378;
  --text-dim: #726c62;
  --text-dimmer: #6f6a60;
  --text-dimmest: #7c766c;
  --text-faintest: #8a8378;
  /* DECISION, not in the handoff: the top-bar spec's placeholder value is
     dark-only. The ramp table lists placeholders under Dim, so light takes
     Dim's value. This resolves the contradiction recorded when dark shipped. */
  --text-placeholder: #726c62;

  /* Semantic. Hue meanings are constant across themes: blue delivery, violet
     lifecycle, amber attention, red destructive, teal files, green presence. */
  --accent: #2f5aa8;
  --code-fg: #1f52ad;
  --code-bg: #f2efea;
  --code-border: #e3dfd9;
  --presence: #2f8a5e;
  --presence-bg: #e8f4ed;
  --presence-border: #bfe0cd;
  --human: #5b3aa8;
  --human-border: #d9cdf0;
  --attention: #9a6b1e;
  --attention-bg: #f9f0dd;
  --attention-border: #e3cfa4;
  /* Darker than a mechanical conversion would give: red on white needs more
     depth than red on black to hold at 11px. */
  --destructive: #b03a24;
  --destructive-bg: #fbeeea;
  --destructive-border: #e6bfb5;
  --files: #126053;
  --offline-dot: #c7c0b5;
  --volume-idle: #b8b0a3;
  --volume-dead: #cec7bb;
  --volume-never: #eae6e0;

  /* Warm and light, so the page behind a modal stays legible as context. */
  --modal-scrim: rgba(48, 42, 32, 0.3);
  --modal-shadow: 0 18px 50px rgba(60, 52, 40, 0.16);
}
```

- [ ] **Step 4: Run to verify it passes**

Run from `ui/`: `npm test -- --run theme`
Expected: PASS, 2 tests. If `missing` is non-empty, the listed tokens are real gaps — add them rather than relaxing the test.

- [ ] **Step 5: Commit**

```bash
cd ui && npm run format && npm test && npm run typecheck && npm run format:check && npm run build
cd .. && git add ui/src
git commit -F - <<'EOF'
chore: add the light token block

Not an inversion. The dark theme carries state as saturated colour against
near-black; flipping lightness turns those muddy and drops the neutrals below
readable contrast, so every value is re-picked from the handoff's own tables.

Three values the handoff does not give are marked in the file as decisions rather
than transcriptions: --surface-control and --border-key have no light row, and
--text-placeholder resolves a contradiction between the top-bar spec and the text
ramp by following the ramp, which is the only source that speaks to light.

A test pins that every colour token has a light value and that no structural
token is duplicated — a token missing from the light block silently renders its
dark value on white, which is invisible until someone opens that screen.
EOF
```

---

### Task 3: The toggle

**Files:**
- Create: `ui/src/theme.ts`, `ui/src/theme.state.test.ts`
- Modify: `ui/src/TopBar.tsx:75-77`, `ui/src/TopBar.module.css`
- Modify: `ui/src/main.tsx`
- Test: `ui/src/TopBar.test.tsx`

**Interfaces:**
- Produces:
  ```ts
  export type Theme = 'dark' | 'light'
  export function systemTheme(): Theme
  export function readTheme(): Theme | null      // the persisted choice, or null
  export function resolveTheme(): Theme          // persisted choice, else system
  export function applyTheme(t: Theme): void     // sets data-theme on <html>
  export function setTheme(t: Theme): void       // applies AND persists
  ```

The handoff: *"Persist the choice; default to the system preference."* Those are two different things, and the distinction matters — an operator who has never chosen must follow their system, and one who has chosen must not have it overridden.

- [ ] **Step 1: Write the failing tests**

Create `ui/src/theme.state.test.ts`:

```ts
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
  localStorage.clear()
  document.documentElement.removeAttribute('data-theme')
  vi.unstubAllGlobals()
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
```

Append to `ui/src/TopBar.test.tsx`. **That file does not use `renderWithStore`** — it mocks the store with `vi.doMock('./useStore', ...)` inside a `mockStore(connection)` helper and then dynamically imports, which is one of the three store-mocking patterns the codebase carries. Follow it:

```tsx
test('the toggle is usable and names the theme it will switch to', async () => {
  document.documentElement.setAttribute('data-theme', 'dark')
  localStorage.clear()
  mockStore('live')
  vi.resetModules()
  const { TopBar: Fresh } = await import('./TopBar')
  render(<Fresh />)
  const button = screen.getByRole('button', { name: /light|dark/i })
  expect(button.hasAttribute('disabled')).toBe(false)
  fireEvent.click(button)
  expect(document.documentElement.getAttribute('data-theme')).toBe('light')
})
```

Check how the existing tests in that file sequence `mockStore` / `vi.resetModules()` / `await import` and mirror it exactly rather than the sketch above — the ordering matters for `vi.doMock`, and the file already gets it right.

- [ ] **Step 2: Run to verify they fail**

Run from `ui/`: `npm test -- --run theme.state TopBar`
Expected: FAIL — `Failed to resolve import './theme'`, and the toggle is `disabled`.

- [ ] **Step 3: Write the module**

Create `ui/src/theme.ts`:

```ts
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
```

- [ ] **Step 4: Apply the theme before React renders**

In `ui/src/main.tsx`, call `applyTheme(resolveTheme())` **before** `createRoot(...).render(...)`. Applying it after the first paint would flash the wrong theme on every load.

- [ ] **Step 5: Wire the toggle**

In `ui/src/TopBar.tsx`, replace the disabled button (lines 75-77):

```tsx
const [theme, setThemeState] = useState<Theme>(() => resolveTheme())
```

and

```tsx
<button
  className={styles.themeToggle}
  onClick={() => {
    const next: Theme = theme === 'dark' ? 'light' : 'dark'
    setTheme(next)
    setThemeState(next)
  }}
>
  {theme === 'dark' ? 'light' : 'dark'}
</button>
```

The label names the theme the click will switch **to**, which is what the existing disabled button already read (`dark` while the console was dark is the one thing it could not have meant — it was a placeholder). If you find the design intends the label to name the *current* theme, follow the handoff and say so in your report.

- [ ] **Step 6: Run the gate and commit**

```bash
cd ui && npm run format && npm test && npm run typecheck && npm run format:check && npm run build
cd .. && git add ui/src
git commit -F - <<'EOF'
chore: wire the theme toggle

Persisting the choice and defaulting to the system preference are two different
things, and the module keeps them apart: someone who has chosen must not be
overridden, and someone who has not must not be guessed at.

The theme is applied before React renders, or every load flashes the wrong one.
Dark is stamped explicitly rather than left as the absence of light, since the
stylesheet keys off the attribute and switching back has to replace it.

localStorage is guarded the way the store's dock state is: this runs during
module init, so an unguarded throw under a storage-blocking policy is a white
screen before React or the error boundary can render.
EOF
```

---

### Task 4: Look at every screen in both themes

**Files:** none — verification only, no commit.

Automated tests can prove a token exists. They cannot see that a border vanished into a background or that a timestamp went unreadable, and jsdom applies no stylesheet at all. This task is the only thing that checks light mode is usable.

- [ ] **Step 1: Run both gates**

```bash
cd ui && npm run typecheck && npm run format:check && npm test && npm run build
cd .. && cargo +nightly fmt
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test --locked
```

- [ ] **Step 2: Build and serve with live traffic**

```bash
cd ui && npm run build && cd ..
cargo build
```

Build order is load-bearing — `rust-embed` compiles the bundle into the binary, and a bus already running keeps its old copy. A bus with live traffic already exists on port 7808 (`/tmp/claude-bus-2f`); restart it against the new binary. `examples/simulate_traffic.rs` (gitignored) generates the traffic; restart it too if it is not running.

- [ ] **Step 3: Look at all eight screens in light, then in dark**

The handoff's own list (5a–5h): room screen, room with the dock open, agent detail, agent tombstone, delete modal, files tab, new-bus empty state, empty room. For each, report what you see — not "renders correctly":

1. **Timestamps.** The ramp pulls them to 4.5:1 deliberately because they are the main scan target. Are they readable on white at 11px, or did they disappear?
2. **Borders against their backgrounds.** `--border-hairline` on `--surface-rail` is the tightest pair in the design. Can you see the row separators?
3. **The volume strips.** The handoff says light strips should still read as busy. Do they, or are they washed out? This is the one place it asks for "weight" without giving a value.
4. **The delete modal.** Warm scrim, softened shadow, and the page behind still legible as context.
5. **State pills and chips** — `human`, `blocked`, `needs you`, `done`, presence dots. These carry meaning by hue; confirm each is still distinguishable and that none went muddy.
6. **The composer**, including the `send` button: light `--accent` on it, with the button's foreground still legible.

- [ ] **Step 4: Check the two transitions**

1. **Toggle back and forth** on a busy screen. Nothing should flash, and no element should keep a dark-mode value.
2. **Reload after switching.** The choice must survive, and there must be no flash of the previous theme before paint.

- [ ] **Step 5: Check the system default honestly**

Clear `localStorage` for the origin and reload with the system in light, then in dark, confirming the console follows each. Chromium's DevTools protocol can force `prefers-color-scheme`; if you cannot drive it, say so plainly rather than reporting the check as passed.

- [ ] **Step 6: Commit nothing; report**

Report each check's result, including anything that looked wrong but you could not attribute.

---

## Self-Review

**Spec coverage** against the handoff's §"Light mode (5a–5h)":

| Handoff requirement | Task |
|---|---|
| Surfaces and borders table | 2 |
| Text ramp table, 4.5:1 floor | 2 (values), 5 (verified) |
| Semantic colours table | 2 |
| Selection is a wash + 2px accent edge | 2 (the wash); the edge is already built |
| Warm modal scrim, softened shadow | 1 (tokens), 2 (light values) |
| Volume strips lose the glow | — no glow exists; stated above |
| Volume strips gain weight | 2 (light values), 4 (judged) |
| Toggle switches the whole set | 3 |
| Persist the choice | 3 |
| Default to the system preference | 3, 4 (verified) |
| All eight screens | 4 |

**Placeholder scan:** no TBD/TODO, no "add error handling", no "similar to Task N".

**Two defects this self-review caught in the plan's own content**, both by reading the code rather than deferring the check — the hedge that only relocates an error:

1. **An entire task was redundant.** A "Task 3" prescribed adding a token-driven 2px accent edge to the selected rail row. `Rail.module.css` already has exactly that — `border-left: 2px solid transparent` on every row, `border-left-color: var(--accent)` when selected — including the transparent-on-all-rows trick that stops selection shifting content sideways. The task and its `--row-selected-edge` token are deleted; Task 2's light `--row-selected` is the whole of the work.
2. **A test used the wrong render helper.** The `TopBar` toggle test called `renderWithStore`, which that file does not use — `TopBar.test.tsx` mocks the store with `vi.doMock` plus `vi.resetModules()` and a dynamic import. Corrected to follow the file's own pattern.

**Type consistency:** `Theme` is defined in Task 3 and used only there. `--modal-scrim`/`--modal-shadow` are created in Task 1, given light values in Task 2, and consumed only by `DeleteModal.module.css`. No token is introduced that nothing consumes.

**One risk restated:** Task 2's test asserts that every `:root` colour token has a light counterpart. That is the single most valuable check in this plan — a token missing from the light block silently renders its dark value on white, and nothing else in the suite would notice, because jsdom applies no stylesheet. If that test is ever hard to satisfy, the answer is to add the token, never to narrow the assertion.
