# Installing claude-bus.
#
# `cargo install --path .` on its own would drop the binary in ~/.cargo/bin,
# which works fine. This defaults to ~/.local/bin instead, so claude-bus sits
# alongside the `claude` binary it plugs into.
#
#   make install                      -> ~/.local/bin/claude-bus
#   make install PREFIX=/usr/local    -> /usr/local/bin/claude-bus
#   make uninstall                    -> removes it from PREFIX
#
# PREFIX must already be on your PATH: .mcp.json and docs/DEPLOY.md both invoke
# `claude-bus` by name, not by path.

PREFIX ?= $(HOME)/.local
BUS ?= ws://127.0.0.1:7777/ws

.PHONY: install uninstall where test deploy config config-project config-check bus-up bus-down bus-logs bus-nuke

## Build in release mode and install to $(PREFIX)/bin.
##
## `cargo install` is release-by-default (you would have to pass --debug to get
## otherwise) and builds into this workspace's target/release, so it shares the
## cache with `cargo build --release` rather than rebuilding in a temp dir.
##
## The result is not stripped — that is cargo's default and worth keeping for a
## long-running service, since a stripped binary gives useless panic backtraces.
## Add `[profile.release] strip = true` to Cargo.toml if you want the ~4M
## artifact instead of ~9M.
install:
	cargo install --path . --root "$(PREFIX)" --locked
	@echo
	@echo "installed: $(PREFIX)/bin/claude-bus"
	@command -v claude-bus >/dev/null 2>&1 \
		|| echo "WARNING: $(PREFIX)/bin is not on your PATH — claude-bus will not be found by name"

uninstall:
	cargo uninstall --root "$(PREFIX)" claude-bus

## Show which claude-bus a shell would actually run — useful when an old copy
## is shadowing a fresh install from an earlier PATH entry.
where:
	@command -v claude-bus || echo "claude-bus is not on PATH"

test:
	cargo test

## Rebuild and deploy both sides after a code change. The default for dev.
##
## The same source builds two independent things: the binary on your PATH (which
## Claude Code spawns as `claude-bus agent`, and which you run as `tail`/`init`),
## and the image inside the container. Neither command updates the other, and the
## Docker build compiles its own copy rather than reusing target/ — so a change to
## shared code like src/proto.rs needs both, or the agent and bus end up disagreeing
## about the wire format at runtime with no compile error to warn you.
deploy: install bus-up
	@echo
	@echo "Both sides deployed."
	@echo
	@echo "Claude Code sessions already running still hold the OLD agent binary —"
	@echo "it is spawned once at session start. Restart a session to pick this up."
	@echo "New sessions get it immediately."

## Bring the bus up, rebuilding first. Safe to re-run after any code change —
## it rebuilds the image, recreates the container, and leaves the data volume
## alone. This replaces the stop/rm/build/run sequence that is easy to get
## half-right by hand.
bus-up:
	docker compose up -d --build
	@echo
	@docker compose ps

## Stop and remove the container. The named data volume survives, so bus.db and
## blobs/ are still there next time. Use `make bus-nuke` to discard them too.
bus-down:
	docker compose down

bus-logs:
	docker compose logs -f

## Remove the container AND its data volume. Destroys every room, message, and
## stored artifact.
bus-nuke:
	docker compose down -v

## Thin wrappers over `claude-bus init` — see docs/DEPLOY.md for what each
## scope actually writes. All three honor BUS, e.g.:
##   make config BUS=ws://nas.lan:7777/ws
config:
	cargo run --quiet -- init --user --bus "$(BUS)"

config-project:
	cargo run --quiet -- init --project --bus "$(BUS)"

## Preview what `make config` (user scope) would do, without writing or
## running anything.
config-check:
	cargo run --quiet -- init --user --bus "$(BUS)" --dry-run
