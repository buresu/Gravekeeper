# Architecture

## Overview

```
gravekeeper (bin: eframe GUI, tray, autostart, main)
    │  CoreHandle::send(Command) / snapshot() -> Arc<Snapshot>
    │            logs_since(app, seq) / schedule_logs_since(schedule, seq)
    ▼
gravekeeper-core (lib: no GUI dependency)
    ├─ engine         Engine thread (Tokio current-thread runtime): state machine, timers, commands
    │  ├─ schedules   running scheduled tasks
    │  ├─ monitoring  samples → per-app usage, chart history, threshold alerts
    │  └─ notify      events → webhook deliveries
    ├─ supervisor     Restart decisions (pure) — RestartTracker
    ├─ schedule       Schedule types and next-run / missed-run planning (pure)
    ├─ monitor        Sampler thread (sysinfo) and debounced thresholds (pure)
    ├─ webhook        Webhook types, payloads, HTTP delivery with retries
    ├─ api            Command / Snapshot / AppStatus / Event (serde-enabled; the future IPC protocol)
    ├─ config         TOML schema, validation, atomic save, migrations, Secret
    ├─ store          SQLite event history and schedule run times on a writer thread
    ├─ logfile        Rotating files, log writer thread, config backups
    ├─ process        Spawn, stop, process-tree termination, stdout/stderr capture (LogBuffer)
    └─ paths          Per-user locations ($GRAVEKEEPER_HOME override)
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
| gravekeeper-core | Tokio current-thread runtime: commands, timers, one task per running process, schedule run or webhook delivery |
| gravekeeper-store | SQLite writer; batches queued writes into one transaction |
| gravekeeper-logs | Writes application and schedule output to rotating files; never blocks senders (drops and counts lines if it falls behind) |
| gravekeeper-monitor | Samples CPU, memory, disks and supervised processes; not joined on shutdown because a disk query can hang |
| tray (Linux only) | D-Bus thread owned by tray-icon's ksni backend |

Neither the GUI thread nor the engine thread ever waits on disk, the network or a child process. Tasks and threads report back to the engine through one `Internal` message channel (process exits, schedule runs, webhook outcomes) and a sample channel.

## Startup

1. `Paths::from_env()` picks the config file and data directory.
2. `CoreHandle::start` takes an exclusive lock on `<data>/gravekeeper.lock` (`File::try_lock`). If another instance holds it, the GUI shows a small "already running" notice and exits with code 3, so two supervisors never start the same applications.
3. The history database is opened; failure is recorded as a `HistoryUnavailable` event and events are then kept in memory only.
4. The configuration is loaded. If it is invalid, the engine starts with no applications, sets `Snapshot::config_error`, and refuses to save until the file is fixed and reloaded — it never overwrites a file it could not read.
5. Applications with `autostart` enter `Waiting` until `max(start_delay, boot_delay − uptime)`. Only the first waiting application in list order is eligible, so automatic starts happen top to bottom.
6. Schedules are planned with catch-up: a run missed since the last recorded run is reported and, with `run-once`, made due immediately (see [configuration.md](configuration.md#schedules)).
7. Monitoring starts; webhooks start delivering once the runtime runs (events from startup are queued).

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

Every change (`SaveApp`, `MoveApp`, `SaveSchedule`, `SaveWebhook`, `SaveMonitoring`, …) builds a new configuration, backs up the current file to `backups/`, validates and saves the new one, then reconciles the running state: new applications appear as `Stopped`, removed ones are stopped and dropped, disabling a running application stops it, changed schedules are re-planned (without catch-up), and the sampler is restarted if monitoring settings changed. Changes to a running application's command take effect at its next start. See [configuration.md](configuration.md) for the file format.

## Schedules

Planning (`schedule::plan`, `schedule::next_after`) is pure calendar arithmetic with jiff and is unit-tested for DST gaps and folds, missed runs, and after-boot behavior. The engine keeps each schedule's next run as a wall-clock time. Because Tokio timers run on the monotonic clock, which does not advance during sleep on every OS, the engine wakes at least every 30 seconds to compare against the wall clock; a run found more than 2 minutes late is handled as missed. The last run time is persisted (`schedule_runs` table) so missed runs and intervals survive restarts.

`run` actions execute like applications (own process group / Job Object, output captured to a per-schedule buffer and log file) under a time limit, after which the tree is stopped. `start` / `stop` / `restart` actions go through the same code as the GUI buttons.

## Monitoring and alerts

The sampler thread uses sysinfo: global CPU (needs two samples), memory as total minus available, non-removable disks (pseudo file systems and macOS system-volume helpers are hidden), and the main process of each supervised application (CPU normalized so all cores are 100%). Values the platform cannot provide are `None` and shown as unsupported.

Each metric (`cpu`, `memory`, `disk:<mount>`) has a `Threshold`: it alerts after the value has stayed at or above the limit for `sustain`, and recovers only below `limit − 5` points, so a value hovering around the limit does not flap. Alerts and recoveries are events (and therefore webhook notifications). The latest sample and 120 samples of history are in the snapshot.

## Notifications

`emit` passes every event to the notifier, which sends it to each enabled webhook whose `events` (and optional `apps`) match. Each delivery is its own task with up to 4 attempts and exponential backoff; at most 50 run at once. Outcomes come back to the engine; failures become `webhook_failed` events, which are excluded from notifications so a broken endpoint cannot cause a loop. URLs and header values are `Secret`s: hidden from `Debug`, and errors are reported with `reqwest::Error::without_url`.

## Log files

With `app_log_files`, each application's and schedule's `LogBuffer` also hands lines (timestamped, tagged `out` / `err` / `---`) to the log writer thread, which appends them to `logs/<id>.log` and rotates by size. Gravekeeper's own `tracing` output goes to `logs/gravekeeper.log` through the same `RotatingFile`. Log file settings are read at startup.

## Event history

Events (`Event { at, level, kind, app, message }`) are kept in the snapshot (latest 500) and written to `<data>/history.sqlite3` in WAL mode, together with each schedule's last run time. The schema is versioned with `PRAGMA user_version` (migration 1: events, 2: schedule runs); the oldest events beyond 100,000 are deleted at startup.

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
