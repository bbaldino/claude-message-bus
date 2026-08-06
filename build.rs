//! Tells cargo that the embedded frontend bundle is an input to this crate.
//!
//! `rust-embed` reads `ui/dist` at compile time, but cargo has no way to know
//! that: nothing under `ui/` is a Rust source file, so a frontend-only change
//! leaves the crate fingerprint identical and cargo reuses the existing binary.
//! `make install` rebuilds the bundle and then `cargo install --path .` — the
//! install documented in the README — quietly ships the *old* `/app`. The
//! symptom is a fix that "isn't working", with nothing anywhere to explain it.
//!
//! Emitting a `rerun-if-changed` for a directory makes cargo watch the mtime of
//! the directory and of every file in it, which is exactly the granularity
//! wanted here: rebuild when the bundle changes, never otherwise.
//!
//! Note that this deliberately does NOT come with `include = ["ui/dist/**"]` in
//! `Cargo.toml`. `ui/dist/.gitkeep` is tracked on purpose, so `cargo package`
//! already includes the directory; an `include` would additionally package the
//! built artifacts into the published crate.
fn main() {
    println!("cargo:rerun-if-changed=ui/dist");
}
