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

.PHONY: install uninstall where test

## Build in release mode and install to $(PREFIX)/bin.
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
