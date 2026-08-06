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

// `@fontsource` faces list woff2 first in `src` (`format('woff2'), format('woff')`),
// and every browser this console targets (React 19 requires a woff2-capable engine
// already) picks woff2. The woff fallback is therefore unreachable weight, not a
// safety net — excluding it here keeps it out of the binary without touching Vite's
// own output, which stays self-consistent for anything that inspects `ui/dist`
// directly. The pattern is `*.woff` (no trailing wildcard), which globset matches
// as a literal suffix: a path ending in `.woff2` has an extra trailing `2` and does
// not satisfy it, so `.woff2` files are unaffected. Verified directly by iterating
// `Bundle::iter()` against a real `npm run build` output, not just reasoned about.
#[derive(rust_embed::Embed)]
#[folder = "ui/dist"]
#[exclude = "*.woff"]
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
        // Gated on the shell being absent rather than the bundle being empty:
        // `ui/dist` always contains the tracked `.gitkeep` and rust-embed does
        // not filter dotfiles, so `Bundle::iter()` is never empty and an unbuilt
        // bundle would otherwise answer a bare "not found" — on exactly the
        // fresh-clone path this hint exists for.
        None if Bundle::get("index.html").is_none() => (
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
        // What an unbuilt bundle actually looks like: `ui/dist/.gitkeep` is
        // tracked so the embed folder exists, and rust-embed does not filter
        // dotfiles. An entirely empty bundle is not a state the real system can
        // be in, so testing that instead would prove nothing about the 503 path.
        let get = fake(&[(".gitkeep", "")]);
        assert!(resolve(&get, "/app").is_none());
        assert!(resolve(&get, "/app/").is_none());
    }
}
