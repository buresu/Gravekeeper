# Phase 0 Technology Check Report

- **Date**: 2026-10-19
- **Environment**: Arch-based Linux 6.18 (LTS), KDE Plasma (Wayland), Rust 1.97.1
- **Windows / macOS**: not run on real machines. Only verified that `cargo clippy --target x86_64-pc-windows-msvc` and `--target aarch64-apple-darwin` compile without warnings.

## Summary

| Item | Result | Notes |
|---|---|---|
| eframe 0.37 window (Wayland / X11) | ✅ | |
| CJK text | ✅ | eframe 0.37's `system_fonts` uses OS fonts (Noto Sans CJK) automatically, so no custom font loading is needed |
| Tray (KDE) | ✅ | tray-icon + ksni. Registration with StatusNotifierWatcher, menu and clicks checked over D-Bus |
| Close to tray (Wayland) | ✅ | `Destroy` mode. `Hide` has no effect on Wayland (as expected) |
| Close to tray (X11) | ✅ | Both `Hide` and `Destroy` work |
| Start with `--minimized` | ✅ | |
| Stopping a process with its descendants | ✅ | Process group |
| Heavy output (64 MiB × stdout/stderr) | ✅ | No blocking; under 1 second in a debug build |
| Escalation from ignored SIGTERM to SIGKILL | ✅ | |
| Licenses | ✅ | All permissive. `cargo deny check` passes (exceptions below) |

## Decisions

1. **Use tray-icon (`ksni` feature) everywhere**
   - The plan was ksni on Linux and tray-icon elsewhere.
   - tray-icon 0.26 has a ksni backend, so all three OSes share one API, and Linux needs neither GTK nor libappindicator.
2. **Drop custom font loading in favor of eframe's `system_fonts`**
   - It uses fontique internally, which looks fonts up through fontconfig on Linux.
3. **Make glow the default renderer; wgpu stays selectable as a Cargo feature**
   - See the measurements below.
4. **Choose the close-window behavior per OS**
   - Linux: `Destroy`; Windows / macOS: `Hide`. Details in [architecture.md](architecture.md).
5. **Ignore three unmaintained notices in cargo-deny**
   - `zvariant`, `zbus_names` and `zvariant_derive`, all via zbus 5 (a ksni dependency). They are not vulnerabilities and will disappear once ksni moves to zbus 6.
   - The two quick-xml vulnerabilities were fixed by `cargo update` (now 0.41).

## Measurements (release build, Linux / Wayland, 3 runs each)

| | glow | wgpu |
|---|---|---|
| Binary size (stripped, thin LTO) | 12.0 MB | 16.9 MB |
| Time to first frame | 37–45 ms (150 ms on the first run only) | 100–116 ms (190 ms on the first run only) |
| RSS, window open | 67.5–68.5 MiB | 74–79 MiB |
| RSS, window closed (in tray) | 67.5–68.5 MiB | 64–66.5 MiB |
| CPU per 10 s, window open and idle | 2–3 ticks (about 0.25%) | 2–3 ticks |

- "Time to first frame" is from process start to the first `App::ui` call.
- Most of the CPU time comes from the engine updating the Snapshot and requesting a redraw every second. Phase 1 will redraw only when something changes.
- With glow, RSS does not drop after the window is destroyed (GL driver and font data stay loaded). Running the GUI as a separate process (Phase 4) is one way to reduce resident memory. Phase 1 will look into the breakdown.

## How the checks were done (for reproduction)

The GUI and tray were driven on Linux as follows:

- **Tray registration**: `qdbus org.kde.StatusNotifierWatcher /StatusNotifierWatcher org.kde.StatusNotifierWatcher.RegisteredStatusNotifierItems`
- **Menu**: `busctl --user call <SNI name> /MenuBar com.canonical.dbusmenu GetLayout iias -- 0 -1 1 label`
- **Clicks and menu actions**:
  - call `org.kde.StatusNotifierItem.Activate`
  - send `com.canonical.dbusmenu.Event <id> clicked`
- **Closing and counting windows**: KWin scripts (`workspace.windowList()` and `closeWindow()`) loaded over D-Bus

## Known issues (for Phase 1 and later)

| Issue | When |
|---|---|
| On Windows, stopping a process terminates the Job Object right away without a graceful stop request | Phase 1 |
| On Windows, `wait()` does not return until every process in the Job has exited (differs from Unix) | Phase 1 |
| If only the parent exits, its descendants keep running on Unix | Phase 1 |
| If Gravekeeper itself crashes, supervised processes are left as orphans on Unix | Phase 1 |
| Environments without a tray host (e.g. GNOME without the extension) are not detected | Phase 1 |
| On Wayland the window icon requires an installed `.desktop` file | Phase 3 |
| Testing on real Windows / macOS machines | Needs hardware |

## What to check on Windows / macOS

1. `cargo run -p gravekeeper` shows the window and the tray icon
2. Closing the window with × keeps the app in the tray; left-clicking the tray icon or choosing "Show Gravekeeper" from its menu brings the window back
3. "Quit" in the tray menu exits the app
4. `cargo run -p gravekeeper -- --minimized` starts with only the tray icon
5. `cargo test --workspace` passes
