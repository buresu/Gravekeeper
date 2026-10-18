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

### Runtime environment variables

| Variable | Purpose |
|---|---|
| `RUST_LOG` | Log level (e.g. `RUST_LOG=debug`, `RUST_LOG=info,gravekeeper=debug`) |
| `GRAVEKEEPER_CLOSE_BEHAVIOR` | Override what closing the window does: `hide` / `destroy` / `quit` (for testing) |
| `WAYLAND_DISPLAY=` | Set to empty to use X11 (XWayland) on Linux |

## Tests

| Kind | Location | Covers |
|---|---|---|
| Unit | `#[cfg(test)]` in each module | Line splitting, ring buffer, exit status classification, engine start and shutdown |
| Integration | `crates/gravekeeper-core/tests/process.rs` | Real processes: output capture, heavy output, stopping, stopping descendants, escalation to kill |

Integration tests use `gk-test-helper` (`crates/gravekeeper-core/src/bin/`) instead of shell commands, so the same tests run on every OS. The helper is not part of the product.

There are no automated GUI or tray tests yet. In Phase 0 they were checked by hand using D-Bus and KWin scripts (see [phase0-report.md](phase0-report.md)).

## CI

`.github/workflows/ci.yml` runs:

- Ubuntu / Windows / macOS: fmt, clippy, test, and a check of the wgpu build
- A check at the MSRV (1.95)
- cargo-deny
