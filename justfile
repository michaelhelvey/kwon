# kwon development justfile
# Run `just --list` to see all available recipes.

# Use a login interactive zsh so that shell profile config (PATH additions,
# aliases like docker → podman, etc.) is available to every recipe.
set shell := ["zsh", "-lic"]

# The cross-compilation target for e2e tests.  kwon targets linux, so on macOS
# we cross-compile a statically-linked musl binary and run it inside a container.
target := "aarch64-unknown-linux-musl"

# ---------------------------------------------------------------------------
# Building
# ---------------------------------------------------------------------------

# Build kwon for the host platform (fast, for local iteration / type-checking)
build:
    cargo build

# Cross-compile kwon for the linux container target used by e2e tests
build-cross:
    cargo build --target {{ target }}

# Build an optimised release binary for the host platform
build-release:
    cargo build --release

# ---------------------------------------------------------------------------
# Code quality
# ---------------------------------------------------------------------------

# Run all cargo checks: format, lint, and type-check
check: fmt-check lint build

# Check formatting without modifying files (useful in CI)
fmt-check:
    cargo fmt -- --check

# Auto-format all source files
fmt:
    cargo fmt

# Run clippy lints (deny warnings so noisy code doesn't slip through)
lint:
    cargo clippy -- -D warnings

# ---------------------------------------------------------------------------
# E2E test environment
#
# The e2e tests run the cross-compiled kwon binary inside an Ubuntu container.
# The workflow is:
#
#   1. `just setup`    — build the binary + start the container (slow, once)
#   2. `just test`     — run tests against the running container (fast, repeat)
#   3. `just teardown` — stop and remove the container (optional cleanup)
#
# `just e2e` is a convenience that does all three in sequence.
# ---------------------------------------------------------------------------

# One-time setup: cross-compile the binary and start the test container
setup: build-cross
    @# ensure the log file exists so the directory mount has something to show
    touch ./data/kwon.log
    @# build the image (only re-layers when the Dockerfile changes) and start
    docker compose up -d --build

# Run e2e tests against an already-running container (fast, repeatable)
test:
    @# truncate — not delete — the log so the directory bind-mount stays valid
    @: > ./data/kwon.log
    @# ensure rsyslogd is up inside the container (strips the imklog module
    @# on first run because there is no kernel log device in the container)
    docker compose exec kwon bash -c \
        "pgrep rsyslogd || (sed -i '/imklog/d' /etc/rsyslog.conf && rsyslogd)"
    uv run --script ./scripts/test.py -v

# Stop and remove the test container and network
teardown:
    docker compose down

# Full e2e cycle: setup → test → teardown (handy for one-shot CI runs)
e2e: setup test teardown

# ---------------------------------------------------------------------------
# Convenience
# ---------------------------------------------------------------------------

# Rebuild the cross binary and re-run e2e tests (quick feedback loop; no container recreate)
retest: build-cross test

# Remove all build artifacts
clean:
    cargo clean
