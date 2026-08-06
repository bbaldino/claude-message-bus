# A TypeScript frontend for the claude-bus web UI

## Problem

The web UI is Rust. `src/web/mod.rs` builds every page with `format!` — 1071 lines of
string concatenation — and `src/web/html.rs` holds the page shell and a stylesheet stored
as a `&str` constant. There is no JavaScript anywhere; every `<script>` in the repo is
inside an XSS-escaping test.

Three things follow from that, and all three are now costs rather than virtues:

**No live updates.** The bus is inherently real-time: it pushes messages, registrations
and events over a websocket, and `claude-bus tail` consumes exactly that. The web UI
cannot. Every page is a snapshot the operator has to refresh by hand, which is the wrong
shape for a view whose whole purpose is watching a fleet work.

**Awkward to evolve.** Structural changes mean editing Rust string literals. The markup
and the logic that produces it are the same expression, so neither can be changed without
reading the other.

**A ceiling on design.** Any interaction — filtering, sorting, expanding a row, tailing a
feed — is either a round trip or a CSS trick. A serious redesign is being commissioned in
parallel, and it should not be constrained by that.

## Goal

Replace the server-rendered UI with a TypeScript single-page app served by the same Rust
binary, talking JSON over HTTP and consuming the existing websocket for live updates.

## Scope of this spec

**Phase 1 only: the infrastructure.** Repo layout, build, embedding, one API endpoint,
the CI gate, and release integration — proven end to end by a single deliberately
disposable screen.

**Not in this spec:** the redesigned screens. A design pass is running in parallel
(`docs/ui-design-pass/`), and the real UI is built from its output, against a pipeline
this phase has already proven. Porting the existing ten screens to React with their
current design is explicitly *not* a goal — it would be work done twice.

## Decisions

### Framework and stack

React with TypeScript, built by Vite. Prettier configured `{ singleQuote: true, semi:
false, printWidth: 100 }` — the project standard. `ui/` is greenfield, so there is no
reformat commit; it starts in that style.

### Shipping: embedded in the binary

The built bundle is embedded via `rust-embed` over `ui/dist` and served by axum. This
preserves exactly today's deploy story: one image, one version, one process, no volume,
and it works on a LAN with no outbound access.

It also means the frontend and the protocol it speaks can never drift apart — `FromBus`
and `ToBus` are versioned by the binary, and now so is the client that consumes them.
Same-origin removes any CORS question and keeps the existing `Origin`-vs-`Host` CSRF
check working unchanged.

Embedding constrains delivery, not capability. The browser fetches the bundle from the
binary and then behaves like any web app: it opens `/ws`, it fetches JSON, it routes
client-side.

**`ui/dist/.gitkeep` must be tracked from the first commit.** `rust-embed` fails to
compile when its directory is absent, and this repo's `release-plz` runs `cargo package
--verify` on every release, which extracts the crate to a clean tree and builds it there.
Without the placeholder, every release fails at packaging — after the tag decision, not
before. The same placeholder is what lets the Rust CI job compile without building the
frontend first.

### Data: JSON over HTTP, websocket for live

Page data is fetched from `/api/*`. Live updates come from the existing `/ws` observer
connection.

Rejected: routing everything over the websocket. Request/reply over a socket is awkward
for initial page loads, gives up HTTP caching and `curl`, turns a dropped connection into
a blank page rather than a stale one, and would widen an observer surface that was
deliberately narrowed — `handle_observer` rejects everything except `Watch`, `History`
and `ListRooms`, with a comment explaining that a viewer is not a participant.

Also rejected: HTTP with polling, which discards the real-time capability that motivates
the change.

**Note on live scope.** Observers today receive live *room messages* via `Watch`. They do
not receive agent connect/disconnect or bus events. Live presence and a live event feed
therefore require protocol additions — real work, not free, and deliberately out of this
phase.

### API design

A new module `src/web/api.rs` mounted under `/api`. The HTML handlers stay in
`src/web/mod.rs`, untouched until they are deleted.

Response types are defined in `api.rs` rather than serializing store rows. `AgentRow` and
friends are internal shapes that change for storage reasons; serializing them directly
would make every schema tweak a silent API break.

TypeScript types are generated from those Rust structs with `ts-rs`, emitted during
`cargo test` into `ui/src/types/`. Hand-maintained interfaces drift silently, and the
failure mode is a UI rendering `undefined` for a renamed field; generation turns that
into a compile error.

The generated directory goes in `.prettierignore`. `ts-rs` does not emit Prettier-shaped
output, so leaving it in scope would fail `format:check` on files nobody edits — and the
"fix" would be reformatting generated code on every regeneration.

Errors are HTTP status codes with a JSON body — `404` for an unknown agent, `409` for the
refusal to delete an online agent. The HTML pages render prose because a human reads them
directly; the API is consumed by code that needs to branch.

When the delete moves to the API it inherits the `Origin`-vs-`Host` CSRF check. A JSON
endpoint is not automatically safer than a form; cross-origin `fetch` is just as easy.

### Transition

The React app mounts at `/app`, with a catch-all serving `index.html` so client-side
routes deep-link. The existing HTML keeps serving `/` unchanged.

Nothing regresses while the design is in progress, and the eventual swap is a one-line
route change. Deleting `src/web/`'s HTML handlers is a separate, later commit.

## Repo layout

```
ui/
  index.html
  package.json          private: true, version 0.0.0, never released
  tsconfig.json
  vite.config.ts        dev proxy: /api and /ws -> 127.0.0.1:7777
  .prettierrc           singleQuote, no semi, printWidth 100
  .prettierignore       dist, node_modules, package-lock.json, coverage, src/types
  src/
    main.tsx
    types/              generated by ts-rs, committed, prettier-ignored
  dist/
    .gitkeep            tracked; rust-embed and cargo package --verify both need it
src/web/
  api.rs                new: JSON routes under /api
  mod.rs                unchanged this phase
  html.rs               unchanged this phase
```

## Build

The Dockerfile gains a node stage ahead of the Rust one. The Rust build cannot start
until `ui/dist` is populated, so the copy must precede `cargo build`:

```
FROM node:22-slim AS ui       npm ci && npm run build   -> /ui/dist
FROM rust:1-slim  AS build    COPY --from=ui /ui/dist ./ui/dist
                              cargo build --release --bin claude-bus
FROM debian:stable-slim       unchanged
```

Development runs `vite dev` on :5173, proxying `/api` and `/ws` to a bus on :7777. Hot
reload, no Rust rebuild in the loop. With `rust-embed`'s `debug-embed` feature off, debug
builds read `ui/dist` from disk rather than from the binary.

## CI

`test.yml` gains a `test-ui` job. Because `test.yml` is the reusable workflow that both
`publish-image.yml` and `release-plz.yml` call, a broken frontend blocks the image *and*
the release PR with no additional wiring.

```yaml
  test-ui:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: ui
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: npm
          cache-dependency-path: ui/package-lock.json
      - run: npm ci
      - run: npm run typecheck
      - run: npm run format:check
      - run: npm run lint --if-present
      - run: npm test
```

`working-directory` and `cache-dependency-path` must point at `ui/`, not the repo root;
getting that wrong fails with a confusing "no lockfile" error.

`npm test` must actually run tests. A vitest dependency with no `test` script passes this
gate while testing nothing.

## Versioning

One image, one version. `Cargo.toml` remains the authority. `ui/package.json` is
`"private": true` and stays at `0.0.0`.

**The UI must not read its version from `package.json`** — it would report `0.0.0`
permanently. It reads it from the API, which reports `CARGO_PKG_VERSION`, the same value
the overview page shows today.

**Verify once, on the first UI-only `feat:` commit:** that release-plz still opens a
release PR for a change touching only `ui/`. The crate is at the repo root so `ui/` sits
inside the package directory and it should — but this is a known polyglot failure mode
and it is cheap to confirm.

## Testing

**Rust.** `api.rs` gets tests in the existing `tests/web.rs` style: a real bus over a real
socket, asserting response content rather than status codes alone. Plus one test that the
catch-all serves `index.html` for a client-side route path — the piece that otherwise
silently 404s deep links.

**TypeScript.** vitest, with a `test` script that runs something real.

## Phase 1 deliverable

`docker compose up` serves:

- the existing HTML at `/`, unchanged
- a React app at `/app`, from a bundle embedded in the binary, that fetches `/api/agents`
  and lists them

with both language gates green in CI and a clean release through the tag-only path.

**That screen is deliberately disposable.** It proves the pipeline; it is not designed and
should not be. The real UI is built from the design pass output, and anything prettier
built now is work that gets deleted.

## Consequences accepted

- The repo becomes polyglot, with a Node toolchain in CI and the image build.
- Image build time grows by the npm install and Vite build.
- Two UIs coexist until the new one is complete. The old one is frozen — changes to it
  during the transition would be ported twice.
- The binary grows by the bundle size.
- Live presence and a live event feed need protocol additions that this phase does not
  make, so Phase 1's UI is live-capable but not yet live.
