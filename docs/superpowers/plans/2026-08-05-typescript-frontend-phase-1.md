# TypeScript Frontend — Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the TypeScript frontend pipeline end to end — a React app under `ui/`, embedded in the Rust binary, fetching a JSON API, gated by CI — proven by one deliberately disposable screen.

**Architecture:** React + Vite builds to `ui/dist`. `rust-embed` compiles that into the binary; axum serves it at `/app` with a catch-all so client-side routes deep-link. Page data comes from JSON under `/api`, with TypeScript types generated from the Rust structs by `ts-rs`. The existing Rust HTML UI stays at `/` untouched.

**Tech Stack:** React, TypeScript, Vite, vitest, Prettier, `rust-embed`, `ts-rs`, axum 0.8, Node 22.

**Spec:** `docs/superpowers/specs/2026-08-05-typescript-frontend-design.md`

## Global Constraints

- **Rust deps are added with `cargo add`, never by hand-editing `Cargo.toml`** — this project always takes the latest version.
- **Node deps are installed with `npm install`, never by hand-writing versions into `package.json`** — same reason.
- Format Rust with **nightly** rustfmt: `cargo +nightly fmt`. CI runs `cargo +nightly fmt --check`.
- Rust lints are blocking: `cargo +stable clippy --all-targets --all-features -- -D warnings`.
- Rust tests: `cargo +stable test --locked`.
- Prettier config is exactly `{ "singleQuote": true, "semi": false, "printWidth": 100 }`. Never adopt Prettier's defaults.
- **Phase 1 deliberately cuts no release.** Every commit here is `chore:` or `refactor:` — never `feat:` or `fix:`, which `release_commits = "^(feat|fix)[(!:]"` would turn into a published version. The screen this phase builds is throwaway; the release comes with the designed UI.
- `ui/package.json` is `"private": true` and stays at version `0.0.0` forever. The Rust `Cargo.toml` is the only version authority.
- Never delete from the `messages` or `events` tables.
- Only the first letter of a multi-letter acronym is capitalised in type names.

---

### Task 1: Scaffold the `ui/` package

Creates the frontend package with its toolchain, config, and the tracked `dist` placeholder that later tasks and every release depend on. No React app yet — just a package that builds, typechecks, formats and tests cleanly.

**Files:**
- Create: `ui/package.json`, `ui/tsconfig.json`, `ui/vite.config.ts`, `ui/.prettierrc`, `ui/.prettierignore`, `ui/index.html`, `ui/src/main.tsx`, `ui/src/smoke.test.ts`, `ui/dist/.gitkeep`
- Modify: `.gitignore`

**Interfaces:**
- Consumes: nothing
- Produces: `ui/` npm package with scripts `dev`, `build`, `typecheck`, `format:check`, `format`, `test`. Build output lands in `ui/dist`.

- [ ] **Step 1: Create the package manifest without dependency versions**

`ui/package.json` — dependencies are added by `npm install` in Step 2, so this file starts with none:

```json
{
  "name": "claude-bus-ui",
  "private": true,
  "version": "0.0.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "typecheck": "tsc --noEmit",
    "format:check": "prettier --check .",
    "format": "prettier --write .",
    "test": "vitest run"
  }
}
```

- [ ] **Step 2: Install dependencies with npm so versions resolve fresh**

Run from `ui/`:

```bash
npm install react react-dom
npm install -D vite @vitejs/plugin-react typescript @types/react @types/react-dom prettier vitest jsdom @testing-library/react @testing-library/jest-dom
```

- [ ] **Step 3: Write the remaining config files**

`ui/.prettierrc`:

```json
{
  "singleQuote": true,
  "semi": false,
  "printWidth": 100
}
```

`ui/.prettierignore`:

```
dist
node_modules
coverage
package-lock.json
src/types
```

`src/types` is ignored because `ts-rs` (Task 3) generates it and does not emit Prettier-shaped output. Without this, `format:check` fails on a file nobody edits.

`ui/tsconfig.json`:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "moduleResolution": "bundler",
    "jsx": "react-jsx",
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noEmit": true,
    "skipLibCheck": true,
    "types": ["vitest/globals"]
  },
  "include": ["src"]
}
```

`ui/vite.config.ts` — `base` matters: the bundle is served under `/app/`, so asset URLs must be built with that prefix or they 404.

```ts
/// <reference types="vitest" />
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  base: '/app/',
  build: { outDir: 'dist', emptyOutDir: true },
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:7777',
      '/ws': { target: 'ws://127.0.0.1:7777', ws: true },
    },
  },
  test: { environment: 'jsdom', globals: true },
})
```

`ui/index.html`:

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>claude-bus</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

`ui/src/main.tsx` — a placeholder root so the package builds before Task 4 writes the real app:

```tsx
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <p>claude-bus</p>
  </StrictMode>,
)
```

`ui/src/smoke.test.ts` — proves `npm test` runs something real, rather than passing vacuously:

```ts
import { expect, test } from 'vitest'

test('the test runner executes assertions', () => {
  expect(1 + 1).toBe(2)
})
```

- [ ] **Step 4: Create the tracked dist placeholder**

```bash
mkdir -p ui/dist
touch ui/dist/.gitkeep
```

This file is load-bearing. `rust-embed` (Task 2) fails to compile when its folder is absent, and `release-plz` runs `cargo package --verify` on every release, which extracts the crate to a clean tree and builds it there. Without a tracked placeholder, releases fail at packaging.

- [ ] **Step 5: Ignore build output but keep the placeholder**

Append to `.gitignore`:

```
ui/node_modules
ui/dist/*
!ui/dist/.gitkeep
```

- [ ] **Step 6: Run the full frontend gate**

Run from `ui/`:

```bash
npm run typecheck && npm run format:check && npm test && npm run build
```

Expected: all four pass, and `ui/dist/index.html` now exists.

If `format:check` fails, run `npm run format` once and re-run — the config files above were written by hand and may not match Prettier exactly.

- [ ] **Step 7: Commit**

```bash
git add ui .gitignore
git commit -F - <<'EOF'
chore: scaffold the ui package

React, Vite and TypeScript under ui/, with the project's Prettier config
rather than Prettier's defaults, and vitest wired to a test that actually
asserts something.

ui/dist/.gitkeep is tracked deliberately: rust-embed will not compile
without the folder, and release-plz runs cargo package --verify from a
clean extract on every release.
EOF
```

---

### Task 2: Serve the bundle from the Rust binary

Embeds `ui/dist` and serves it at `/app`, with a catch-all so client-side routes deep-link. The path-resolution logic is a pure function so it can be tested without a built bundle — CI's Rust job has only `.gitkeep` in `ui/dist`.

**Files:**
- Create: `src/web/assets.rs`
- Modify: `src/web/mod.rs` (add `mod assets;`, register two routes in `routes()` at line ~288)
- Modify: `Cargo.toml` (via `cargo add`)

**Interfaces:**
- Consumes: `ui/dist/.gitkeep` from Task 1
- Produces: routes `GET /app` and `GET /app/{*rest}`; `assets::resolve(get, path) -> Option<(Vec<u8>, &'static str)>`

- [ ] **Step 1: Add the dependency**

```bash
cargo add rust-embed
```

- [ ] **Step 2: Write the failing tests**

Create `src/web/assets.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fake(files: &[(&str, &str)]) -> impl Fn(&str) -> Option<Vec<u8>> {
        let map: HashMap<String, Vec<u8>> = files
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
            .collect();
        move |p: &str| map.get(p).cloned()
    }

    #[test]
    fn serves_an_exact_file_with_its_content_type() {
        let get = fake(&[("assets/app.js", "console.log(1)")]);
        let (body, ct) = resolve(&get, "/app/assets/app.js").expect("asset must resolve");
        assert_eq!(body, b"console.log(1)");
        assert_eq!(ct, "text/javascript");
    }

    #[test]
    fn serves_index_at_the_app_root() {
        let get = fake(&[("index.html", "<!doctype html>")]);
        let (body, ct) = resolve(&get, "/app").expect("root must resolve");
        assert_eq!(body, b"<!doctype html>");
        assert_eq!(ct, "text/html; charset=utf-8");
    }

    #[test]
    fn an_unknown_client_route_falls_back_to_index() {
        // A deep link like /app/agents/caas is a client-side route, not a file.
        let get = fake(&[("index.html", "<!doctype html>")]);
        let (body, _) = resolve(&get, "/app/agents/caas").expect("deep link must resolve");
        assert_eq!(body, b"<!doctype html>");
    }

    #[test]
    fn a_missing_file_with_an_extension_is_not_index() {
        // Falling back to index.html for a missing .js would hand the browser
        // HTML where it expected a script, which fails confusingly at runtime.
        let get = fake(&[("index.html", "<!doctype html>")]);
        assert!(resolve(&get, "/app/assets/missing.js").is_none());
    }

    #[test]
    fn an_unbuilt_bundle_resolves_to_nothing() {
        let get = fake(&[]);
        assert!(resolve(&get, "/app").is_none());
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib assets`
Expected: FAIL to compile — `cannot find function 'resolve' in this scope`

- [ ] **Step 4: Write the implementation**

Add above the test module in `src/web/assets.rs`:

```rust
//! Serving the single-page app bundle out of the binary.
//!
//! The bundle is compiled in by `rust-embed`, so a release image carries the UI
//! with no second artifact and no outbound fetch — this bus commonly runs on a
//! LAN with no internet.
//!
//! `resolve` is a pure function taking the lookup as a parameter rather than
//! calling `Bundle::get` directly, so it is testable without a built bundle.
//! CI's Rust job has only `.gitkeep` in `ui/dist`, and building the frontend
//! just to test path resolution would couple the two jobs for nothing.

use axum::extract::Path as AxumPath;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

#[derive(rust_embed::Embed)]
#[folder = "ui/dist"]
struct Bundle;

/// Content type for a bundle path, by extension. Deliberately small: a Vite
/// build emits html, js, css, and occasionally svg/json/woff2.
fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        Some("woff2") => "font/woff2",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    }
}

/// Resolve a request path within the bundle.
///
/// Returns the bytes and content type, or `None` when the request is for a
/// missing file or the bundle was never built.
fn resolve(
    get: &impl Fn(&str) -> Option<Vec<u8>>,
    request_path: &str,
) -> Option<(Vec<u8>, &'static str)> {
    let rel = request_path
        .strip_prefix("/app")
        .unwrap_or(request_path)
        .trim_start_matches('/');

    if !rel.is_empty()
        && let Some(bytes) = get(rel)
    {
        return Some((bytes, content_type(rel)));
    }

    // A path with an extension that missed is a genuine 404. Only extension-less
    // paths are client-side routes worth answering with the app shell.
    if rel.contains('.') {
        return None;
    }

    get("index.html").map(|bytes| (bytes, content_type("index.html")))
}

fn respond(request_path: &str) -> Response {
    let get = |p: &str| Bundle::get(p).map(|f| f.data.into_owned());
    match resolve(&get, request_path) {
        Some((bytes, ct)) => ([(header::CONTENT_TYPE, ct)], bytes).into_response(),
        None if Bundle::iter().next().is_none() => (
            StatusCode::SERVICE_UNAVAILABLE,
            "the UI bundle was not built into this binary — run `npm run build` in ui/ \
             and rebuild, or use the server-rendered UI at /",
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

pub(crate) async fn app_root() -> Response {
    respond("/app")
}

pub(crate) async fn app_path(AxumPath(rest): AxumPath<String>) -> Response {
    respond(&format!("/app/{rest}"))
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib assets`
Expected: PASS (5 tests)

- [ ] **Step 6: Register the routes**

In `src/web/mod.rs`, add the module declaration beside the existing `pub mod html;`:

```rust
mod assets;
```

And add two routes inside `routes()`:

```rust
        .route("/app", get(assets::app_root))
        .route("/app/{*rest}", get(assets::app_path))
```

- [ ] **Step 7: Verify the whole suite and the lints**

Run: `cargo +nightly fmt && cargo +stable clippy --all-targets --all-features -- -D warnings && cargo +stable test --locked`
Expected: PASS. The new routes return 503 in tests because `ui/dist` holds only `.gitkeep`, which is the intended unbuilt-bundle behaviour.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock src/web/assets.rs src/web/mod.rs
git commit -F - <<'EOF'
chore: serve the SPA bundle from the binary at /app

rust-embed compiles ui/dist into the binary, so a release image carries the
UI with no second artifact and no outbound fetch.

Path resolution is a pure function taking the lookup as a parameter, so it
is testable without a built bundle — CI's Rust job has only .gitkeep in
ui/dist, and building the frontend to test path handling would couple the
two jobs for nothing.

A missing file WITH an extension 404s rather than falling back to the app
shell: handing a browser HTML where it asked for a script fails confusingly
at runtime. Extension-less paths are client-side routes and do get the shell.
EOF
```

---

### Task 3: The `/api/agents` endpoint with generated TypeScript types

Establishes the API pattern: an explicit response type in `api.rs`, camelCase on the wire, and a matching `.ts` emitted by `ts-rs` so the frontend's types come from the Rust definitions.

**Files:**
- Create: `src/web/api.rs`
- Modify: `src/web/mod.rs` (add `mod api;`, register one route)
- Modify: `Cargo.toml` (via `cargo add`)
- Test: `tests/web.rs` (append)
- Generated: `ui/src/types/Agent.ts`

**Interfaces:**
- Consumes: `Store::agents() -> Vec<AgentRow>` where `AgentRow { name, host, cwd, session_id, online, is_human, version, last_seen }`; `Registry::is_online(&str) -> bool`
- Produces: route `GET /api/agents` returning `Vec<Agent>`; `ui/src/types/Agent.ts` exporting `type Agent`

- [ ] **Step 1: Add the dependency**

```bash
cargo add ts-rs
```

- [ ] **Step 2: Write the failing test**

Append to `tests/web.rs`:

```rust
#[tokio::test]
async fn the_agents_api_returns_json_in_camel_case() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).await.unwrap();
        store
            .upsert_agent("network-debug#2", "hardac", "/w/nd", Some("sess-1"), false, Some("0.3.3"))
            .await
            .unwrap();
    }
    let port = start(dir.path()).await;

    let body = get(port, "/api/agents").await;

    assert!(body.contains("application/json"), "must be served as JSON: {body}");
    assert!(body.contains("\"name\":\"network-debug#2\""), "got: {body}");
    assert!(body.contains("\"host\":\"hardac\""), "got: {body}");
    // camelCase on the wire even though the column is last_seen.
    assert!(body.contains("\"lastSeen\":"), "wire format must be camelCase: {body}");
    assert!(body.contains("\"sessionId\":\"sess-1\""), "got: {body}");
    // mark_all_offline runs at startup, so a seeded agent is offline.
    assert!(body.contains("\"online\":false"), "got: {body}");
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --test web the_agents_api`
Expected: FAIL — the route 404s, so none of the JSON assertions hold

- [ ] **Step 4: Write the implementation**

Create `src/web/api.rs`:

```rust
//! JSON routes for the single-page app, under `/api`.
//!
//! Response types are defined here rather than serialising `store` rows
//! directly. Those rows are internal shapes that change for storage reasons,
//! and serialising them would turn every schema tweak into a silent API break.
//! Defining the wire format here also lets it be camelCase while the columns
//! stay snake_case.
//!
//! The TypeScript equivalents are generated from these structs by `ts-rs`
//! during `cargo test`, so the frontend's types cannot drift from the server's.

use axum::Json;
use axum::extract::State;

use crate::bus::App;

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "ui/src/types/")]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub name: String,
    pub host: String,
    pub cwd: String,
    pub session_id: Option<String>,
    pub online: bool,
    pub is_human: bool,
    pub version: Option<String>,
    /// Epoch milliseconds.
    pub last_seen: i64,
}

/// Every agent the bus has ever seen, with liveness from the registry rather
/// than the persisted `online` column — the column is only reconciled at
/// startup, while the registry knows who is routable right now.
pub(crate) async fn agents(State(app): State<App>) -> Json<Vec<Agent>> {
    let rows = app.store.agents().await.unwrap_or_default();
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let online = app.registry.is_online(&r.name).await;
        out.push(Agent {
            name: r.name,
            host: r.host,
            cwd: r.cwd,
            session_id: r.session_id,
            online,
            is_human: r.is_human,
            version: r.version,
            last_seen: r.last_seen,
        });
    }
    Json(out)
}
```

- [ ] **Step 5: Register the route**

In `src/web/mod.rs`, add beside the other module declarations:

```rust
mod api;
```

And inside `routes()`:

```rust
        .route("/api/agents", get(api::agents))
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test --test web the_agents_api`
Expected: PASS

- [ ] **Step 7: Verify the TypeScript type was generated**

Run: `cargo test && ls ui/src/types/`
Expected: `Agent.ts` exists. Inspect it — it should declare `export type Agent = { name: string, ... lastSeen: number, ... }`.

If the file lands somewhere else, `export_to` is relative to the crate root; adjust it until the file appears at `ui/src/types/Agent.ts` before continuing.

- [ ] **Step 8: Run the full gate**

Run: `cargo +nightly fmt && cargo +stable clippy --all-targets --all-features -- -D warnings && cargo +stable test --locked`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml Cargo.lock src/web/api.rs src/web/mod.rs tests/web.rs ui/src/types
git commit -F - <<'EOF'
chore: add GET /api/agents with a generated TypeScript type

Response types live in api.rs rather than serialising store rows, so a
schema change cannot silently alter the wire format, and the wire can be
camelCase while the columns stay snake_case.

ts-rs emits the matching .ts during cargo test, so the frontend's types come
from the Rust definitions instead of being hand-copied — a renamed field
becomes a compile error rather than a UI rendering undefined.

Liveness comes from the registry, not the persisted online column, matching
the rest of the web layer.
EOF
```

---

### Task 4: The disposable screen

A React app that fetches `/api/agents` and lists them. Its only job is to prove the pipeline end to end. It is not designed and should not be — the real screens come from the design pass.

**Files:**
- Create: `ui/src/App.tsx`, `ui/src/App.test.tsx`
- Modify: `ui/src/main.tsx`

**Interfaces:**
- Consumes: `GET /api/agents` from Task 3; `ui/src/types/Agent.ts` exporting `type Agent`
- Produces: nothing downstream

- [ ] **Step 1: Write the failing test**

Create `ui/src/App.test.tsx`:

```tsx
import { render, screen } from '@testing-library/react'
import { afterEach, expect, test, vi } from 'vitest'
import { App } from './App'

afterEach(() => {
  vi.restoreAllMocks()
})

test('renders the agents returned by the api', async () => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(
      JSON.stringify([
        {
          name: 'network-debug#2',
          host: 'hardac',
          cwd: '/w/nd',
          sessionId: null,
          online: false,
          isHuman: false,
          version: '0.3.3',
          lastSeen: 1785000000000,
        },
      ]),
      { headers: { 'content-type': 'application/json' } },
    ),
  )

  render(<App />)

  // The suffixed name is the case this whole UI exists to surface.
  expect(await screen.findByText('network-debug#2')).toBeDefined()
  expect(await screen.findByText('hardac')).toBeDefined()
})

test('shows the error rather than an empty page when the api fails', async () => {
  vi.spyOn(globalThis, 'fetch').mockRejectedValue(new Error('connection refused'))

  render(<App />)

  expect(await screen.findByText(/connection refused/)).toBeDefined()
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run from `ui/`: `npm test`
Expected: FAIL — `Failed to resolve import './App'`

- [ ] **Step 3: Write the implementation**

Create `ui/src/App.tsx`:

```tsx
import { useEffect, useState } from 'react'
import type { Agent } from './types/Agent'

// Deliberately unstyled. This screen exists to prove the pipeline — bundle
// embedded in the binary, served at /app, fed by /api/agents — and is replaced
// wholesale by the design pass output. Anything prettier here is deleted later.
export function App() {
  const [agents, setAgents] = useState<Agent[] | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    fetch('/api/agents')
      .then((r) => {
        if (!r.ok) throw new Error(`/api/agents returned ${r.status}`)
        return r.json()
      })
      .then(setAgents)
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
  }, [])

  if (error) return <p>could not load agents: {error}</p>
  if (!agents) return <p>loading…</p>

  return (
    <table>
      <thead>
        <tr>
          <th>name</th>
          <th>host</th>
          <th>version</th>
          <th>state</th>
        </tr>
      </thead>
      <tbody>
        {agents.map((a) => (
          <tr key={a.name}>
            <td>{a.name}</td>
            <td>{a.host}</td>
            <td>{a.version ?? 'unknown'}</td>
            <td>{a.online ? 'online' : 'offline'}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
```

- [ ] **Step 4: Mount it**

Replace `ui/src/main.tsx`:

```tsx
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
```

- [ ] **Step 5: Run the frontend gate**

Run from `ui/`: `npm test && npm run typecheck && npm run format:check && npm run build`
Expected: all pass. If `format:check` fails, run `npm run format` and re-run.

- [ ] **Step 6: Verify it end to end in a real browser path**

```bash
cd ui && npm run build && cd ..
cargo run -- serve --port 7799 --data /tmp/claude-bus-phase1 &
sleep 3
curl -s http://127.0.0.1:7799/app | head -5
curl -s http://127.0.0.1:7799/api/agents
kill %1
```

Expected: `/app` returns the built `index.html` (not the 503), and `/api/agents` returns a JSON array. Note the binary must be rebuilt after `npm run build` for the new bundle to be embedded.

- [ ] **Step 7: Commit**

```bash
git add ui/src
git commit -F - <<'EOF'
chore: add a throwaway agents screen to prove the pipeline

Fetches /api/agents and lists them. Deliberately unstyled: this exists to
prove bundle-in-binary, /app serving and the JSON API work end to end, and
is replaced wholesale by the design pass output.

Covers the two states worth having: rendered rows, and a visible error
rather than a blank page when the API is unreachable.
EOF
```

---

### Task 5: Docker build and the CI gate

Teaches the image build to compile the frontend, and adds the Node gate to the shared reusable workflow so a broken frontend blocks both the image and the release PR.

**Files:**
- Modify: `Dockerfile`
- Modify: `.github/workflows/test.yml`
- Modify: `docs/DEPLOY.md`

**Interfaces:**
- Consumes: `ui/` package with a `build` script from Task 1
- Produces: nothing downstream

- [ ] **Step 1: Add the node build stage to the Dockerfile**

Replace `Dockerfile` with:

```dockerfile
FROM node:22-slim AS ui
WORKDIR /ui
COPY ui/package.json ui/package-lock.json ./
RUN npm ci
COPY ui/ ./
RUN npm run build

FROM rust:1-slim AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY schema.sql ./
COPY src ./src
# rust-embed compiles ui/dist into the binary, so the bundle must exist before
# cargo build runs — not after.
COPY --from=ui /ui/dist ./ui/dist
RUN cargo build --release --bin claude-bus

FROM debian:stable-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/claude-bus /usr/local/bin/claude-bus
VOLUME ["/data"]
EXPOSE 7777
ENTRYPOINT ["claude-bus", "serve", "--port", "7777", "--data", "/data"]
```

- [ ] **Step 2: Verify the image builds**

Run: `docker build -t claude-bus:phase1-test .`
Expected: succeeds. If the Rust stage fails with a `rust-embed` error about a missing folder, the `COPY --from=ui` line is in the wrong place — it must precede `cargo build`.

- [ ] **Step 3: Add the Node job to the shared test workflow**

Append to `.github/workflows/test.yml`, as a second job under `jobs:`:

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
      - run: npm test
      - run: npm run build
```

`working-directory` and `cache-dependency-path` must point at `ui/`, not the repo root — getting that wrong fails with a confusing "no lockfile" error. `npm run build` is included so a build break fails CI rather than only the image build.

- [ ] **Step 4: Validate the workflow with the strict loader**

```bash
python3 - <<'PY'
import yaml
class Strict(yaml.SafeLoader): pass
def no_dupes(loader, node, deep=False):
    seen = set()
    for k, _ in node.value:
        key = loader.construct_object(k, deep=deep)
        if key in seen:
            raise yaml.YAMLError(f"duplicate key: {key!r} at line {k.start_mark.line+1}")
        seen.add(key)
    return yaml.SafeLoader.construct_mapping(loader, node, deep)
Strict.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, no_dupes)
d = yaml.load(open('.github/workflows/test.yml'), Strict)
print('jobs:', list(d['jobs']))
PY
```

Expected: `jobs: ['test', 'test-ui']`. Plain `yaml.safe_load` silently accepts duplicate keys and takes the last, so it cannot catch a duplicated `jobs:` block — this loader can.

- [ ] **Step 5: Update the deployment docs**

In `docs/DEPLOY.md`, replace the paragraph beginning "The bus serves a web UI on the same port:" so it reads:

```markdown
The bus serves two web UIs on the same port, during the transition to the new one:

- `/` — the original server-rendered pages, described below
- `/app` — the TypeScript single-page app, which replaces them once the redesign lands

Both come out of the same binary; there is no second service and nothing is fetched
from the internet at runtime.
```

- [ ] **Step 6: Run the full local gate**

Run: `cargo +nightly fmt --check && cargo +stable clippy --all-targets --all-features -- -D warnings && cargo +stable test --locked`
Then from `ui/`: `npm run typecheck && npm run format:check && npm test && npm run build`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add Dockerfile .github/workflows/test.yml docs/DEPLOY.md
git commit -F - <<'EOF'
chore: build the frontend in the image and gate it in CI

The Dockerfile gains a node stage; its output is copied in before cargo
build, because rust-embed compiles ui/dist into the binary and cannot run
before it exists.

test.yml gains a test-ui job. Because test.yml is the reusable workflow both
publish-image.yml and release-plz.yml call, a broken frontend now blocks the
image and the release PR with no extra wiring. npm run build is part of the
gate so a build break fails CI rather than only the image build.
EOF
```

---

## Self-Review

**Spec coverage:**

| Spec requirement | Task |
|---|---|
| React + TypeScript + Vite under `ui/` | 1 |
| Prettier `{singleQuote, semi:false, printWidth:100}` | 1 |
| `ui/package.json` private, version `0.0.0` | 1 |
| `ui/dist/.gitkeep` tracked | 1 |
| `src/types` in `.prettierignore` (ts-rs output) | 1 |
| Vite dev proxy for `/api` and `/ws` | 1 |
| `rust-embed` over `ui/dist` | 2 |
| App at `/app` with catch-all for client routes | 2 |
| `src/web/api.rs` under `/api`, HTML untouched | 3 |
| Explicit response types, not store rows | 3 |
| camelCase wire format | 3 |
| `ts-rs` generating into `ui/src/types/` | 3 |
| Liveness from the registry | 3 |
| Disposable screen fetching `/api/agents` | 4 |
| Dockerfile node stage before `cargo build` | 5 |
| `test-ui` job in the shared `test.yml` | 5 |
| `working-directory`/`cache-dependency-path` at `ui/` | 5 |
| `npm test` runs something real | 1 (smoke), 4 (real) |
| No `feat:`/`fix:` — phase cuts no release | Global Constraints; every commit is `chore:` |

Two spec items are deliberately **not** in this plan, and both are correct omissions:

- **Errors as status codes with JSON bodies** — Phase 1's only endpoint has no error path; `/api/agents` returns an empty array for an empty bus. The first endpoint that can 404 introduces this.
- **The `Origin`-vs-`Host` CSRF check on API mutations** — Phase 1 adds no mutating endpoint. It applies when the delete moves to the API.

**Placeholder scan:** no TBD/TODO, no "add error handling", no "similar to Task N". Every code step carries the actual content.

**Type consistency:** `Agent` (Task 3) is consumed in Task 4 as `import type { Agent } from './types/Agent'` with fields `name`, `host`, `version`, `online` — matching the `#[serde(rename_all = "camelCase")]` struct. `resolve(get, path) -> Option<(Vec<u8>, &'static str)>` (Task 2) is used only within its own module. `App` is exported as a named export in Task 4 and imported as one in both `main.tsx` and the test.
