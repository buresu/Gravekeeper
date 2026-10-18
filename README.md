# Gravekeeper

A desktop tool for supervising and operating applications on Windows, macOS and Linux:
start/stop/restart processes, capture their output, detect crashes, restart with back-off,
run schedules and send webhook notifications. Written in Rust with an [egui] GUI.

> **Status:** Phase 0 (technology check). Nothing is usable yet beyond a window, a system
> tray icon and the process-control foundation. See [the implementation plan](docs/implementation-plan.md).

## Layout

| Path | Contents |
|---|---|
| `crates/gravekeeper-core` | Supervisor engine. No GUI dependency; talks to front-ends only through `CoreHandle`. |
| `crates/gravekeeper` | Desktop application (eframe window, system tray). |
| `docs/` | Architecture, plan, platform notes, build instructions, phase reports. |

## Building

Requires Rust 1.95 or newer. See [docs/building.md](docs/building.md) for platform packages.

```sh
cargo run -p gravekeeper            # debug build of the GUI
cargo test --workspace              # all tests
cargo build --release -p gravekeeper
```

## License

[MIT](LICENSE)

[egui]: https://github.com/emilk/egui
