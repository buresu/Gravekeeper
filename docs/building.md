# Building and Testing

## Requirements

- Rust 1.95 or newer (stable), preferably installed with `rustup`.
- Platform packages:
  - **Debian / Ubuntu**: `sudo apt install libxkbcommon-dev libwayland-dev libfontconfig1-dev`
  - **Arch Linux**: `sudo pacman -S libxkbcommon wayland fontconfig`
  - **Windows / macOS**: nothing extra.

## Commands

```sh
cargo run -p gravekeeper                 # run the GUI (debug build)
cargo run -p gravekeeper -- --minimized  # start in the tray only
cargo test --workspace                   # all tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo build --release -p gravekeeper     # target/release/gravekeeper
cargo deny check                         # licenses, advisories, sources (cargo install cargo-deny)
```

### Renderer

glow (OpenGL) is the default. To try wgpu instead:

```sh
cargo run -p gravekeeper --no-default-features --features wgpu
```

### Checking other platforms from Linux

The Windows and macOS code paths can be type-checked without those machines. Two dependencies compile C code for the target: the bundled SQLite (point `libsqlite3-sys` at a non-existent system library instead; nothing is linked by `check`/`clippy`) and TLS for webhooks (turn off the `http` feature):

```sh
rustup target add x86_64-pc-windows-msvc aarch64-apple-darwin
LIBSQLITE3_SYS_USE_PKG_CONFIG=1 SQLITE3_LIB_DIR=/nonexistent \
  cargo clippy --workspace --all-targets --target x86_64-pc-windows-msvc \
  --no-default-features --features gravekeeper/glow -- -D warnings
```

(Same for `aarch64-apple-darwin`.)

### Runtime environment variables

| Variable | Purpose |
|---|---|
| `GRAVEKEEPER_HOME` | Keep config, history and lock file in this directory (portable use, testing) |
| `RUST_LOG` | Log level (e.g. `RUST_LOG=debug`, `RUST_LOG=info,gravekeeper=debug`) |
| `GRAVEKEEPER_CLOSE_BEHAVIOR` | Override what closing the window does: `hide` / `destroy` / `quit` (for testing) |
| `WAYLAND_DISPLAY=` | Set to empty to use X11 (XWayland) on Linux |

## Tests

| Kind | Location | Covers |
|---|---|---|
| Unit | `#[cfg(test)]` in each module | Config parsing/validation/saving/migration, restart policy, schedule planning (DST, missed runs), thresholds, webhook filters and payloads, log rotation, history store, line splitting, exit classification, editors, autostart entry, formatting |
| Process | `crates/gravekeeper-core/tests/process.rs` | Real processes: output capture, heavy output, stopping, stopping descendants, escalation to kill |
| Operations | `crates/gravekeeper-core/tests/operations.rs` | Start order, reordering and backups, schedule runs (success, failure, time limit), interval schedules, missed-run catch-up across restarts, webhooks against a local HTTP server (retry, failure without URL, test message), log files, monitoring alerts and recovery |
| Engine | `crates/gravekeeper-core/tests/engine.rs` | The whole engine with real processes: autostart and delay, crash loop and give-up, on-failure policy, failed starts, pause, restart, output, spawn failure, editing/removing, broken config, single instance, history across restarts, shutdown |

Integration tests use `gk-test-helper` (`crates/gravekeeper-core/src/bin/`) instead of shell commands, so the same tests run on every OS. The helper is not part of the product.

There are no automated GUI tests. Window and tray behavior was checked with D-Bus and KWin scripts (see [phase0-report.md](phase0-report.md)); input into the window cannot be automated on KWin Wayland, so forms are checked by hand.

To try the GUI without touching your real configuration:

```sh
GRAVEKEEPER_HOME=/tmp/gk-test cargo run -p gravekeeper
```

## CI

`.github/workflows/ci.yml` runs:

- Ubuntu / Windows / macOS: fmt, clippy, test, and a check of the wgpu build
- A check at the MSRV (1.95)
- cargo-deny
