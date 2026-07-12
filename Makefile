# ais-forwarder-rs — top-level Makefile
#
# Convenience wrappers over `cargo` for the most common workflows. Nothing
# here is required: `cargo build --release`, `cargo test --workspace`, etc.
# do the same job directly. The value is remembering the right invocation
# in one place, and having a `precommit` target that mirrors what CI checks.
#
# Common targets (see `make help` for the full list):
#   make               - Release build of every workspace member
#   make debug         - Debug build of every workspace member
#   make test          - Run the workspace test suite
#   make fmt           - `cargo fmt --all`
#   make clippy        - Workspace clippy at `-D warnings`
#   make precommit     - fmt + clippy + test — run this before pushing
#   make ais-forwarder - Release build of just the `ais-forwarder` binary
#   make clean         - `cargo clean`
#
# Machine-specific targets (cross-compile deploys to a particular Pi, the
# systemd unit name, hardware experiments) belong in `Makefile.local`, which
# is gitignored and `-include`d at the bottom of this file.

CARGO ?= cargo

.PHONY: all build debug check test fmt fmt-check clippy precommit \
        ais-forwarder clean help

all: build

# Full-workspace release build (ais-forwarder, common, location-receiver).
build:
	$(CARGO) build --release --workspace

# Full-workspace debug build. Faster to compile, slower to run.
debug:
	$(CARGO) build --workspace

# Quick type-check without producing binaries.
check:
	$(CARGO) check --workspace --all-targets

# Workspace test suite.
test:
	$(CARGO) test --workspace

fmt:
	$(CARGO) fmt --all

# fmt but read-only (fails when files aren't formatted).
fmt-check:
	$(CARGO) fmt --all --check

# Workspace-wide clippy at CI strictness.
clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Everything you'd want green before pushing.
precommit: fmt clippy test

# Release build of just the front-end binary.
ais-forwarder:
	$(CARGO) build --release -p ais-forwarder

clean:
	$(CARGO) clean

help:
	@echo "ais-forwarder-rs Makefile targets:"
	@echo ""
	@echo "  make                Release build of every workspace member"
	@echo "  make debug          Debug build of every workspace member"
	@echo "  make check          Type-check without producing binaries"
	@echo "  make test           Run the workspace test suite"
	@echo "  make fmt            cargo fmt --all"
	@echo "  make fmt-check      cargo fmt --all --check"
	@echo "  make clippy         Workspace clippy at -D warnings"
	@echo "  make precommit      fmt + clippy + test"
	@echo "  make ais-forwarder  Release build of just the ais-forwarder binary"
	@echo "  make clean          cargo clean"
	@echo ""
	@echo "Machine-specific deploy targets live in Makefile.local (gitignored)."

# Optional machine-specific extensions — cross-compile deploys, the systemd
# unit name, hardware rigs. The leading dash makes the include silent when the
# file is absent, so a fresh clone behaves identically to having no file.
-include Makefile.local
