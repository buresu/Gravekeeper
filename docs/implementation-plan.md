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
| GUI | eframe / egui 0.37 (glow renderer), rfd (file dialogs), egui_kittest (tests) | Requirement. glow is smaller and starts faster than wgpu ([Phase 0 report](phase0-report.md)) |
| Tray | tray-icon 0.26 (`ksni` feature) | Same API on all three OSes; on Linux it uses D-Bus (StatusNotifierItem) and needs no GTK |
| Processes | process-wrap 10 | Process groups / Job Objects / KillOnDrop behind one API |
| Async | tokio 1 | Process waiting, timers, HTTP. Used only inside the engine |
| Serialization | serde | Config files and future IPC |
| Logging | tracing / tracing-subscriber | Structured logs |
| Config | toml | TOML is the planned format. Per-OS locations are computed with the standard library (`directories` was dropped: it pulls in an MPL-2.0 crate for a few lines of path logic) |
| History | rusqlite (`bundled`) | SQLite built in, so no system library is needed on any OS |
| Time and schedules | jiff (`serde`) | IANA time zones with explicit DST handling for the scheduler; local timestamps in the GUI |
| Webhooks | reqwest (rustls + OS certificate store), serde_json | HTTP client integrated with the engine's Tokio runtime; no OpenSSL |
| Metrics | sysinfo | CPU, memory, disks, per-process usage and boot time on all three OSes |
| Login items | windows-registry (Windows), zbus (Linux, already pulled in by ksni) | Run key on Windows; tray host detection on Linux |
| Packaging | cargo-packager (tool), embed-resource, png (icon generator only) | Installers for all OSes from one config; Windows exe icon |
| Headless | tokio `signal` | SIGTERM / SIGHUP handling in headless mode |
| Planned | interprocess | Local IPC (Phase 4) |

## Phase 0: Technology check — done

See [phase0-report.md](phase0-report.md) for details.

- [x] Workspace, LICENSE, README, CI, cargo-deny configuration
- [x] Minimal eframe window (checked on Wayland and X11), CJK font fallback
- [x] Tray icon, menu (Show / Quit), click to reopen
- [x] Close the window and keep running in the tray (Linux: Destroy; X11 also works with Hide)
- [x] Process control checks: stopping descendants, 64 MiB × 2 of output, SIGTERM-to-SIGKILL escalation, exit status
- [x] Startup time, memory and binary size measurements
- [ ] Testing on real Windows / macOS machines (CI only builds and tests)

## Phase 1: MVP — done (except items marked below)

- [x] **config**: `AppSpec` (id, name, program, arguments, working directory, environment, enabled, autostart, start delay, startup grace, stop timeout, restart policy) in TOML with `schema_version`, validation, unknown-key rejection, atomic save with `.bak`, private permissions on Unix ([configuration.md](configuration.md))
- [x] **Single instance**: exclusive lock file in the data directory (`File::try_lock`, all OSes); a second launch shows a notice and exits
- [x] **supervisor**: state machine (Stopped / Waiting / Starting / Running / Stopping / Backoff / GaveUp / Disabled, plus Paused), restart policy with exponential backoff, jitter, give-up limit and stability reset; exit reasons that distinguish requested stops from failures
- [x] **store**: SQLite event history (bundled), migrations, batching writer thread, retention limit; the app keeps working without it
- [x] **API**: `Command` (Start / Stop / Restart / SetPaused / SaveApp / RemoveApp / SaveGeneral / ReloadConfig / Shutdown), `Snapshot` with per-app `AppStatus` and recent events, `logs_since`
- [x] **GUI**: dashboard, application list, details and controls, editor form, output viewer (stdout / stderr / lifecycle notes, filter), event history with filters, settings, config-error banner, quit confirmation
- [x] **Window state**: size and the selected tab/application persist (eframe `persistence`); start-minimized setting and `--minimized`
- [x] **Launch at login** (`AutostartManager`): XDG autostart `.desktop` / LaunchAgent plist / HKCU Run value
- [x] Tray host detection on Linux (falls back to closing = quit)
- [x] Leftover descendants are killed when the main process exits on its own
- [ ] Graceful stop on Windows (CTRL_BREAK for console apps, WM_CLOSE for GUI apps) — needs a Windows machine to develop and test
- [ ] On Windows, `wait()` returns only when every process in the Job has exited, so an exit of the main process alone is not noticed while descendants run
- [ ] Orphans when Gravekeeper itself is killed (Unix: the supervised processes keep running; Windows: the Job Object kills them). Options: adopt them on restart via a PID file, or `PR_SET_PDEATHSIG` on Linux
- [ ] Manual check of the editor form and launch-at-login toggle in a real session (input could not be automated on KWin Wayland)

### Decisions made in Phase 1

- **Durations are whole seconds** in the file and the editor. Sub-second precision has no use for supervision and keeps the file readable.
- **The startup grace replaces a separate "start timeout"**: a process that exits within `startup_grace` counts as a failed start. A start that "hangs" cannot be detected without a health check, which belongs to Phase 2 monitoring.
- **A stable run clears the failure history entirely**, so `max_restarts` / `window` only limit crash loops, not rare crashes spread over days.
- **Edits apply at the next start** rather than restarting a running application implicitly.
- **The lock file uses the standard library's `File::try_lock`** (Rust 1.89+) on every OS instead of a named mutex on Windows.
- **Event kinds and levels are stored as text** in SQLite, so the history stays readable with any SQLite tool and survives enum reordering.

## Phase 2: Operations — done

- [x] **Start order and boot delay**: automatic starts go top to bottom in the list (reorder in the GUI); `boot_delay` waits for the computer's uptime
- [x] **Scheduler** (jiff): once / daily / weekly / interval / after boot; start, stop, restart an application or run a program with a time limit; per-schedule time zone; DST gaps run at the next valid time and folds run once; missed runs (Gravekeeper stopped or computer asleep) are skipped or run once, also across restarts; results recorded as events and shown per schedule with output
- [x] **Webhook notifications** (reqwest + rustls): JSON / Slack / Discord payloads, per-webhook event and application filters, custom headers, 4 attempts with exponential backoff, test messages, failures recorded without the URL
- [x] **Resource monitoring** (sysinfo): CPU, memory, disks, per-application CPU and memory, charts on the dashboard; thresholds with a sustain period and hysteresis raise and clear alerts
- [x] **Log rotation**: application and schedule output to `logs/` (size-based rotation), Gravekeeper's own log file
- [x] **Configuration backups**: a timestamped copy before every save (20 kept); schema version 2 with automatic migration from version 1
- [ ] Notifications through the OS (desktop notifications) — not planned yet; webhooks cover remote alerts
- [ ] Moving webhook secrets to the OS credential store

### Decisions made in Phase 2

- **The scheduler re-checks the wall clock at least every 30 seconds.** Tokio timers use the monotonic clock, which does not count time asleep on every OS; comparing against wall-clock time handles sleep, resume and clock changes in one place.
- **"Late" means more than 2 minutes.** Within that, a run is just late (e.g. the engine was busy); beyond it, the missed-run policy applies.
- **`after-boot` schedules are never "missed"**: they run once per boot however late Gravekeeper starts, which is what "after the computer starts" means in practice.
- **An `interval` continues from the last run across restarts**, so a "every 6 hours" task does not run every time Gravekeeper restarts.
- **Each webhook delivery is its own task**, so one slow endpoint cannot delay others; the cost is that deliveries are not strictly ordered.
- **Thresholds are whole percentages with 0 meaning off**, because TOML has no null to turn off a default.
- **Memory usage is total minus available**, not "used", which on Linux would count reclaimable cache.
- **The sampler thread is never joined**: a hung network disk must not freeze the engine when settings change or on shutdown.
- **Log file settings apply after a restart**, to avoid re-opening every application's log while it is being written.
- **Binary size grew from 12.0 MB (Phase 0) to 22.9 MB** (Linux release, glow), mostly TLS (rustls with aws-lc), the HTTP client and SQLite. Acceptable for a desktop tool; revisit (e.g. a smaller HTTP client or the `ring` provider) if distribution size matters.
- **HTTP is a Cargo feature (`http`, on by default)** only so that other targets can be type-checked from Linux without a C cross-compiler for TLS.

## Phase 3: OS integration and distribution — done (except items marked below)

- [x] **Command line**: `--minimized`, `--headless`, `service install|uninstall|status`, `--help`, `--version`
- [x] **Headless mode**: the supervisor without a GUI; graceful stop on SIGTERM/SIGINT/Ctrl-C, reload on SIGHUP
- [x] **User service** (`ServiceManager`): systemd user unit (with reload) on Linux, LaunchAgent on macOS
- [x] **Children stopped if Gravekeeper is killed** on Linux (`PR_SET_PDEATHSIG`)
- [x] **Icon** drawn in code with anti-aliasing; generated PNG / ICO / ICNS; `.desktop` entry with `StartupWMClass`; icon embedded in the Windows executable
- [x] **Installers** with cargo-packager: `.deb` and AppImage (built and run on Linux), NSIS and dmg (CI); user-local `install.sh`; release workflow with checksums and draft releases
- [x] **Update and code signing design** ([distribution.md](distribution.md))
- [x] AppImage mounts and other read-only file systems are no longer monitored (they caused false disk alerts)
- [x] Linux arm64 (aarch64) `.deb` and AppImage, built and tested on GitHub's arm64 runners
- [ ] Windows service (session 0, service control handler) — with the separate supervisor process
- [ ] Graceful stop and exit detection on Windows (from Phase 1)
- [ ] macOS: stopping children when Gravekeeper is killed; adopting orphans after a restart
- [ ] Code signing (needs certificates) and the update check (needs a release location)
- [ ] Real-machine tests on Windows and macOS, X11-only and GNOME desktops

### Decisions made in Phase 3

- **Services are per user, not system-wide.** Applications usually belong to a user and need their environment; a system service would need privilege separation and access control first (Phase 4).
- **Service and GUI exclude each other** through the existing lock rather than one silently taking over. The GUI attaching to a running service needs IPC (Phase 4).
- **cargo-packager** builds all formats from one configuration in `Cargo.toml`, is MIT/Apache licensed, and comes with a signed updater for later.
- **Linux packages target Ubuntu 22.04's glibc** in CI so the `.deb` and AppImage run on older distributions too.
- **Releases are drafts** until published by hand.
- **Icons are generated, not drawn by hand**, from the same code as the window icon, and committed so packaging needs no extra tool.

## Usability pass (between Phase 3 and 4) — done

Requested before Phase 4: native file dialogs and a less dated look.

- [x] **Native file and folder dialogs** (rfd; XDG desktop portal on Linux, no GTK) for programs and working directories in the application and schedule editors; picking a program fills in an empty name
- [x] **"Open folder" buttons** for the configuration, log files, backups and an application's log
- [x] **New layout**: sidebar navigation with problem badges, page headers with the main action, master–detail pages with cards, status pills, empty states with a call to action, grouped forms with help text
- [x] **Theme**: bundled Noto Sans (regular and semibold) and Phosphor icons, both subset to a few dozen KB; higher-contrast text; tuned light and dark palettes; System / Light / Dark choice in Settings
- [x] **Accessibility**: every control has an accessible name (enforced by the GUI tests); status dots are drawn instead of relying on a glyph that Noto Sans lacks
- [x] **GUI tests** with egui_kittest, and on-demand screenshots of every page (also used for the README)

## Phase 4: Extensions

- Run the supervisor as a separate process with local IPC
- Screen capture, OS settings backup and restore
- OS settings for unattended machines ([architecture.md](architecture.md#os-settings-and-administrator-rights)):
  - [x] Elevated helper (`gravekeeper elevated`): UAC, pkexec, osascript; closed set of tasks
  - [x] Automatic updates on/off (Windows: services, tasks, policy)
  - [x] Automatic updates on Linux (elevated; apt and dnf-automatic timers)
  - [x] Automatic updates on macOS (elevated; Software Update preferences)
  - [x] Automatic login (Windows: Winlogon + LSA secret; password via the OS credential prompt, not the command line)
  - [x] Automatic login on Linux (display-manager config: Plasma Login Manager, GDM)
  - [x] Automatic login on macOS (`autoLoginUser` + `kcpassword`; password via dialog, checked with PAM)
  - [x] Notifications on/off (Windows; per-user toast switch + Windows tips, no elevation. Linux: Do not disturb on KDE and GNOME. Not possible on macOS: Focus has no interface for other apps)
  - [x] Windows Security (Defender) notifications on/off (Windows; elevated)
  - [x] Firewall on/off (Windows; elevated, all profiles. Linux: ufw or firewalld, elevated. macOS: application firewall, elevated)
  - [x] Power: automatic sleep on/off (Windows; no elevation, per-user power scheme. Linux: KDE PowerDevil, GNOME gsettings)
  - [x] Power: turn off the display on/off (Windows; no elevation. Linux: KDE PowerDevil, GNOME gsettings)
  - [x] Power: turn off hard disks on/off (Windows; no elevation. macOS: elevated, `pmset disksleep`)
  - [x] Power: hibernation on/off (Windows; elevated, `powercfg /hibernate`. Linux: elevated, systemd `sleep.conf.d` drop-in)
  - [x] Power: macOS sleep, display and hibernation settings (elevated, `pmset`)
  - [x] Windows: wake timers, screen saver (no elevation); requiring sign-in on wake, lock screen, locking when inactive, Wake on LAN (elevated)
  - [x] Schedules that wake the computer from sleep (Windows; waitable timer)
  - [x] App Nap on/off (macOS; per-user `NSAppSleepDisabled`, no elevation)
  - [x] macOS: screen saver, reopening apps at login (per-user); starting up after a power failure, wake for network access, Bluetooth Setup Assistant at startup (elevated)
- MCP server and a control API for AI agents (with allow lists, audit logs and access control)
