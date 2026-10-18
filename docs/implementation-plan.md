# Implementation Plan

This document tracks what is done, what is not, and known issues for each phase. Update it at the end of every phase.

## Principles

- Rust stable (MSRV 1.95), egui / eframe for the GUI, MIT license.
- UI and documentation are in English. CJK text that appears in supervised apps' logs is displayed through fallback to OS fonts.
- A two-crate workspace (`gravekeeper-core` has no GUI dependency).
- Add a dependency only when its need can be explained. Check licenses with `cargo deny check` when adopting one.
- Keep OS-specific code behind common interfaces, but introduce an abstraction only once a second implementation is needed.

## Technology choices

| Purpose | Crate | Reason |
|---|---|---|
| GUI | eframe / egui 0.37 (glow renderer) | Requirement. glow is smaller and starts faster than wgpu ([Phase 0 report](phase0-report.md)) |
| Tray | tray-icon 0.26 (`ksni` feature) | Same API on all three OSes; on Linux it uses D-Bus (StatusNotifierItem) and needs no GTK |
| Processes | process-wrap 10 | Process groups / Job Objects / KillOnDrop behind one API |
| Async | tokio 1 | Process waiting, timers, HTTP. Used only inside the engine |
| Serialization | serde | Config files and future IPC |
| Logging | tracing / tracing-subscriber | Structured logs |
| Planned | toml, rusqlite (bundled), reqwest (rustls), sysinfo, jiff, directories, interprocess | Introduced in the phase that needs them |

## Phase 0: Technology check — done

See [phase0-report.md](phase0-report.md) for details.

- [x] Workspace, LICENSE, README, CI, cargo-deny configuration
- [x] Minimal eframe window (checked on Wayland and X11), CJK font fallback
- [x] Tray icon, menu (Show / Quit), click to reopen
- [x] Close the window and keep running in the tray (Linux: Destroy; X11 also works with Hide)
- [x] Process control checks: stopping descendants, 64 MiB × 2 of output, SIGTERM-to-SIGKILL escalation, exit status
- [x] Startup time, memory and binary size measurements
- [ ] Testing on real Windows / macOS machines (CI only builds and tests)

## Phase 1: MVP

Implemented in small steps, each with tests.

1. **config**: `AppSpec` (ID, name, executable, arguments, working directory, environment, delay, autostart, restart policy, timeouts)
   - TOML, `schema_version`, validation, atomic save (temp file + rename) and a `.bak`
   - Location chosen with the `directories` crate; permissions 0600 / 0700 on Unix
2. **Single instance**: a lock file (flock) on Unix, a named mutex on Windows. A second launch brings the existing window to the front (notification only until the Phase 4 IPC exists)
3. **supervisor**: state machine and restart policy, with an injectable clock for deterministic tests
   - Exit reasons: `UserStop | StopTimeoutKilled | Exited(code) | Signaled(sig) | SpawnFailed | StartTimeout`
4. **store**: SQLite (rusqlite, bundled) event history and migrations. If the DB cannot be opened, the app still starts and runs without history
5. **API extension**: `Command` (Start / Stop / Restart / Pause / Enable / UpsertApp / RemoveApp), per-app state in `Snapshot`, `Event` subscription
6. **GUI**: app list, detail and edit form, log viewer (incremental fetch via `LogBuffer::since`), event history, general settings
7. **Window state save/restore** (eframe `persistence` feature) and a start-minimized setting
8. **Launch at login** (`AutostartManager`): HKCU\…\Run on Windows, a LaunchAgent plist on macOS, an XDG autostart `.desktop` on Linux

Issues found in Phase 0 that Phase 1 resolves:

- Graceful stop on Windows (CTRL_BREAK for console apps, WM_CLOSE for GUI apps)
- On Windows, `wait()` does not return until every process in the Job has exited. Design a way to detect the parent's exit first
- What to do when only the parent exits and descendants remain; decide whether to terminate the whole process group by default
- Orphaned processes when Gravekeeper itself crashes (they keep running on Unix): adopt them or terminate them after restart
- Detect environments without a tray host (GNOME without the extension) by checking for `org.kde.StatusNotifierWatcher`, and fall back to `Quit`

## Phase 2: Operations

- Delayed start and start order (given as order plus delay, not as a dependency graph)
- Scheduler (jiff): Once / Daily / Weekly / Interval / AfterBoot
  - A time that does not exist because of DST moves forward to the next valid time. A time that occurs twice runs once, at the earlier occurrence
  - Missed runs (scheduled time passed while stopped) are configurable per job: `Skip` / `RunOnce`
- Webhook notifications (reqwest + rustls) with bounded exponential-backoff retries. URLs and tokens are held in a `Secret` type that is masked in `Debug` and logs
- CPU / memory / disk monitoring (sysinfo) with thresholds. Metrics that cannot be collected are shown as "unsupported"
- Log rotation (size limit and generation count), config backups

## Phase 3: OS integration and distribution

- Finish per-OS autostart, OS service registration (`ServiceManager`, a separate responsibility from `AutostartManager`)
- Installers (MSI / dmg / AppImage; Flatpak and others to be evaluated)
- Install a `.desktop` file and icon on Linux (on Wayland the window icon comes from the `.desktop` matching the `app_id`)
- Update mechanism, code signing, testing on real Wayland / X11 / macOS / Windows machines

## Phase 4: Extensions

- Run the supervisor as a separate process with local IPC
- Screen capture, OS settings backup and restore
- MCP server and a control API for AI agents (with allow lists, audit logs and access control)
