# Architecture

## Overview

```
gravekeeper (bin: eframe GUI, tray, autostart, main)
    │  CoreHandle::send(Command) / snapshot() -> Arc<Snapshot> / logs_since(app, seq)
    ▼
gravekeeper-core (lib: no GUI dependency)
    ├─ engine      Engine thread (Tokio current-thread runtime): state machine, timers, commands
    ├─ supervisor  Restart decisions (pure, no I/O) — RestartTracker
    ├─ api         Command / Snapshot / AppStatus / Event (serde-enabled; the future IPC protocol)
    ├─ config      TOML schema, validation, atomic save, migrations
    ├─ store       SQLite event history on its own writer thread
    ├─ process     Spawn, stop, process-tree termination, stdout/stderr capture (LogBuffer)
    ├─ paths       Per-user locations ($GRAVEKEEPER_HOME override)
    └─ scheduler / monitoring / notify   (Phase 2)
```

### Boundary rules

- `gravekeeper-core` does not depend on egui, eframe or tray-icon. Splitting the crates lets the compiler enforce this.
- The GUI never touches engine internals. It reads an `Arc<Snapshot>` (replaced as a whole after every change) and changes things only by sending a `Command`. Failures come back as `Event`s, not return values.
- Output lines are fetched with `CoreHandle::logs_since(app, seq)`, which becomes a request/response message once there is IPC.
- The engine asks the GUI to redraw through a `Notifier` (`Fn() + Send + Sync`) passed in at startup. The GUI passes `egui::Context::request_repaint`. The GUI also redraws every 500 ms while visible, for relative times and live output.
- The GUI may call pure helpers from the core (e.g. `config::validate_app`) to validate input before sending it; the engine validates again.

## Threads

| Thread | Role |
|---|---|
| main | eframe / winit event loop (must be the main thread on macOS) |
| gravekeeper-core | Tokio current-thread runtime: commands, timers, one task per running process |
| gravekeeper-store | SQLite writer; batches queued events into one transaction |
| tray (Linux only) | D-Bus thread owned by tray-icon's ksni backend |

Neither the GUI thread nor the engine thread ever waits on disk or on a child process.

## Startup

1. `Paths::from_env()` picks the config file and data directory.
2. `CoreHandle::start` takes an exclusive lock on `<data>/gravekeeper.lock` (`File::try_lock`). If another instance holds it, the GUI shows a small "already running" notice and exits with code 3, so two supervisors never start the same applications.
3. The history database is opened; failure is recorded as a `HistoryUnavailable` event and events are then kept in memory only.
4. The configuration is loaded. If it is invalid, the engine starts with no applications, sets `Snapshot::config_error`, and refuses to save until the file is fixed and reloaded — it never overwrites a file it could not read.
5. Applications with `autostart` enter `Waiting` with their `start_delay`.

## Supervised application lifecycle

```
            ┌──────────── manual start ────────────┐
            ▼                                      │
Stopped ─start→ Starting ─(alive for startup_grace)→ Running ─stop→ Stopping → Stopped
   ▲                │ exits                          │ exits
   │                ▼                                ▼
   │          restart policy says restart? ── no ──→ Stopped
   │                │ yes
   │                ▼
   └── stop ── Backoff ─(delay)→ Starting
                    │ too many failures
                    ▼
                 GaveUp (error event; needs a manual start)

Waiting: autostart delay before the first start.   Disabled: `enabled = false`.
Paused (a flag, not a state): exits are recorded but never restarted.
```

- **Exit reasons** (`ExitReason`): `UserStop`, `StopTimeoutKilled`, `Exited(code)`, `Signaled(sig)`, `Unknown`, `SpawnFailed(error)`. Whether the exit was requested comes from the engine's own intent, not from the exit code, so a user stop is never mistaken for a crash. `crash_hint` (crash-like signal on Unix, NTSTATUS code on Windows) is a hint shown to the user only.
- **Restart decision** (`supervisor::RestartTracker`, pure and unit-tested):
  - `never`: no restart. `on-failure`: restart unless the exit code was 0 after a successful start. `always`: restart after any unrequested exit.
  - Exiting within `startup_grace` counts as a failed start, even with code 0.
  - Delay = `min(backoff_initial × 2^(n−1), backoff_max)` plus up to 10% jitter.
  - A run of at least `stable_after` clears the failure history. More than `max_restarts` failures within `window` → `GaveUp`.
  - A manual start resets the history.
- **Leftover descendants**: when the main process exits on its own, whatever remains of its process group / Job Object is killed. Applications that daemonize (fork and exit) are therefore not supported; run them in foreground mode.
- **Shutdown**: quitting stops every running application (in parallel, each with its `stop_timeout`) before the engine exits. As a last resort, dropping the engine kills remaining process trees (`KillOnDrop`).

## Configuration changes

`SaveApp`, `RemoveApp` and `SaveGeneral` build a new configuration, validate and save it, then reconcile the running state: new applications appear as `Stopped`, removed ones are stopped and dropped, disabling a running application stops it. Changes to a running application's command take effect at its next start. See [configuration.md](configuration.md) for the file format.

## Event history

Events (`Event { at, level, kind, app, message }`) are kept in the snapshot (latest 500) and written to `<data>/history.sqlite3` in WAL mode. The schema is versioned with `PRAGMA user_version`; the oldest rows beyond 100,000 are deleted at startup.

## Window and tray lifecycle

What closing the main window does (`CloseBehavior`) has a per-OS default.

| Value | Behavior | Default on | Why |
|---|---|---|---|
| `Hide` | `CancelClose` + `Visible(false)`; the event loop keeps running | Windows / macOS | The tray depends on the main-thread event loop, so the loop cannot stop |
| `Destroy` | Destroy the window and return from `run_native_ext`; a new window is created when the tray asks for one | Linux | winit cannot hide windows on Wayland. ksni runs on its own thread, so the tray keeps working without the loop |
| `Quit` | Closing the window exits | No tray | A hidden window could never be shown again |

- On Linux the tray is only created if a StatusNotifierItem host (`org.kde.StatusNotifierWatcher`) is on the session bus; otherwise `Quit` is used and the window always opens.
- The same `egui::Context` is passed to every `run_native_ext` call, so the `Notifier` stays valid when the window is recreated. The selected tab and application are saved with eframe's persistence and restored with the window.
- Tray callbacks may run on any thread. `Shell::handle` sends a `ViewportCommand` while a window exists, and otherwise (`Destroy` mode) sends the action to the main loop's channel.
- Quitting from the window asks for confirmation when applications are running; quitting from the tray does not.
- `GRAVEKEEPER_CLOSE_BEHAVIOR=hide|destroy|quit` overrides the default (for testing).

## Launch at login

`autostart.rs` registers the GUI (with `--minimized`) in the user's session: an XDG autostart `.desktop` file on Linux, a LaunchAgent plist on macOS, and an `HKCU\…\Run` value on Windows. The state shown in Settings is read from the OS, not stored in the configuration. Running the supervisor as an OS service before login is a separate feature (Phase 3).

## Separate supervisor process (Phase 4)

```
GUI process ── local IPC (Named Pipe / Unix Domain Socket) ── Supervisor process
```

- The `interprocess` crate is the IPC candidate. The protocol reuses the types in the `api` module.
- Connections are limited to a per-user socket or pipe, with permissions that reject other users.
- Work that needs a user session, such as screen capture, is kept out of the supervisor.
