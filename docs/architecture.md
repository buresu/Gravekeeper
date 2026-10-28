# Architecture

## Overview

```
gravekeeper (bin: CLI → GUI | --headless | service install/uninstall/status)
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
    ├─ store          SQLite event history and schedule run/plan times on a writer thread
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

## Modes

`main.rs` parses the command line (`cli.rs`) and runs one of:

| Mode | Module | What runs |
|---|---|---|
| `gravekeeper [--minimized]` | `gui.rs` | Engine + window + tray in one process |
| `gravekeeper --headless` | `headless.rs` | Engine only; SIGTERM / SIGINT / Ctrl-C stop it gracefully, SIGHUP reloads the configuration |
| `gravekeeper service …` | `service.rs` | Registers the headless mode as a user service (systemd user unit, LaunchAgent) |

Both running modes take the same lock, so only one supervises. Logging goes to stderr and `logs/gravekeeper.log` in both. See [distribution.md](distribution.md) for packaging and services.

## Startup

1. `Paths::from_env()` picks the config file and data directory.
2. `CoreHandle::start` takes an exclusive lock on `<data>/gravekeeper.lock` (`File::try_lock`). If another instance holds it, the GUI shows a small "already running" notice and exits with code 3, so two supervisors never start the same applications.
3. The history database is opened; failure is recorded as a `HistoryUnavailable` event and events are then kept in memory only.
4. The configuration is loaded. If it is invalid, the engine starts with no applications, sets `Snapshot::config_error`, and refuses to save until the file is fixed and reloaded — it never overwrites a file it could not read.
5. Applications with `autostart` enter `Waiting` until `max(start_delay, boot_delay − uptime)`. Only the first waiting application in list order is eligible, so automatic starts happen top to bottom.
6. Schedules are planned with catch-up: a run missed since the last recorded run (or, if later, since the schedule was added or its timing changed) is reported and, with `run-once`, made due immediately (see [configuration.md](configuration.md#schedules)).
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
- **If Gravekeeper itself dies** (killed, crashed): on Linux every child is spawned with `PR_SET_PDEATHSIG = SIGTERM`, so the kernel stops the supervised processes; on Windows the Job Objects close with the process and kill their trees. On macOS the processes keep running (no equivalent mechanism; planned together with adopting orphans after a restart).
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

Events (`Event { at, level, kind, app, message }`) are kept in the snapshot (latest 500) and written to `<data>/history.sqlite3` in WAL mode, together with each schedule's last run time and when its timetable began. The schema is versioned with `PRAGMA user_version` (migration 1: events, 2: schedule runs, 3: schedule plans); the oldest events beyond 100,000 are deleted at startup.

## Window and tray lifecycle

What closing the main window does (`CloseBehavior`) has a per-OS default.

| Value | Behavior | Default on | Why |
|---|---|---|---|
| `Hide` | `CancelClose` + `Visible(false)`; the event loop keeps running | macOS | The tray depends on the main-thread event loop, so the loop cannot stop |
| `Destroy` | Destroy the window and return from `run_native_ext`; a new window is created when the tray asks for one | Linux / Windows | winit cannot hide windows on Wayland. ksni runs on its own thread; on Windows the tray only needs the main thread to process messages, which `wait_for_tray` does while there is no window |
| `Quit` | Closing the window exits | No tray | A hidden window could never be shown again |

- On Linux the tray is only created if a StatusNotifierItem host (`org.kde.StatusNotifierWatcher`) is on the session bus; otherwise `Quit` is used and the window always opens.
- The same `egui::Context` is passed to every `run_native_ext` call, so the `Notifier` stays valid when the window is recreated. The selected tab and application are saved with eframe's persistence and restored with the window.
- Tray callbacks may run on any thread. `Shell::handle` sends a `ViewportCommand` while a window exists, and otherwise (`Destroy` mode) sends the action to the main loop's channel.
- A minimized start creates no window in `Destroy` mode. In `Hide` mode the window is created hidden, but eframe shows every window after its first frame, so the app sends `Visible(false)` at creation, which takes effect right after that show.
- Quitting from the window asks for confirmation when applications are running; quitting from the tray does not.
- `GRAVEKEEPER_CLOSE_BEHAVIOR=hide|destroy|quit` overrides the default (for testing).

## Launch at login

`autostart.rs` registers the GUI (with `--minimized`) in the user's session: an XDG autostart `.desktop` file on Linux, a LaunchAgent plist on macOS, and an `HKCU\…\Run` value on Windows. The state shown in Settings is read from the OS, not stored in the configuration. Running the supervisor as an OS service before login is a separate feature (Phase 3).

## OS settings and administrator rights

`os_settings.rs` reads and changes OS settings for machines that run unattended (so far: automatic updates, automatic logon, automatic sleep, turning off the display, hibernation and notifications). Most need administrator rights and go through `elevate` (below); the sleep and display-off timeouts and the notifications switch do not — they are per-user settings a standard user may change — so they are applied in-process. On Windows, turning updates off disables the Windows Update services (`wuauserv`, `UsoSvc`, `WaaSMedicSvc`) and their scheduled tasks, and on editions that honor them also sets two group policies (`NoAutoUpdate` and `SetDisableUXWUAccess`, the latter being what makes the Settings page show updates as managed); the services and tasks make it work on Home too. The original service `Start` values and the list of tasks that were disabled are backed up under `HKLM\SOFTWARE\Gravekeeper\WindowsUpdate` so the change is reversible exactly, and nothing is renamed, deleted or re-owned. The services are controlled through the Win32 service API; the scheduled tasks through PowerShell's `ScheduledTasks` module (the Task Scheduler COM API would mean hand-building `VARIANT`s). They are kept apart from Gravekeeper's own settings: the System page (`ui/system.rs`) shows them, the Settings page only Gravekeeper's. Values are read from the OS (which needs no special rights) and never stored in the configuration file.

Gravekeeper itself never runs elevated: it installs per user, launch at login (`HKCU\…\Run`) does not start programs that need elevation, and supervised applications would inherit the rights. Machine-wide changes go through `elevate.rs` instead, which starts the same executable as `gravekeeper elevated <request>` through the OS's consent prompt, only for as long as the change takes:

| | Prompt | Report back |
|---|---|---|
| Windows | UAC (`ShellExecuteExW` with `runas`) | Named pipe `\\.\pipe\gravekeeper-elevated-…` created by Gravekeeper (first instance only, local clients only); only the helper's process ID is accepted as the client, and the helper opens it with `SECURITY_IDENTIFICATION` so it cannot be impersonated |
| Linux | polkit (`pkexec`); an AppImage runs the `.AppImage` file, since root cannot read its FUSE mount | stdout |
| macOS | `osascript` `do shell script … with administrator privileges` | stdout (the script's result) |

- A request is a list of `Task`s, a closed set of operations with typed parameters. It never names a command, file or registry path, and unknown tasks are rejected, so the helper is not a general way to run something as an administrator.
- The request is hex-encoded JSON on the command line, which cannot be changed after the process starts. Command lines are readable by other processes, so a request never contains a secret. The auto-logon password, for instance, is not in the request at all: the elevated helper collects it through the OS credential dialog (see below).
- The report is `{"Ok":null}` or `{"Err":"…"}`. The helper does not log, so that it never creates files owned by another account in the user's data directory.
- Per-user settings (e.g. `HKCU`) must not be changed by the helper: when a standard user approves with an administrator's credentials, the helper runs as that administrator.
- If Gravekeeper already runs elevated (or as root), tasks are applied in-process.
- `elevate::spawn` runs the request on its own thread, so the GUI keeps drawing while the prompt is open.

### Automatic logon (Windows)

Turning auto-logon on writes the account to the Winlogon registry key (`AutoAdminLogon = 1`, `DefaultUserName`, `DefaultDomainName`) and, so Windows 11 honors it, sets `DevicePasswordLessBuildVersion = 0` (its previous value is backed up). The **password is never stored in plain text**: it goes into the LSA secret `DefaultPassword` via `LsaStorePrivateData`, the same place the Sysinternals Autologon tool uses. The elevated helper collects it through the OS credential dialog (`CredUIPromptForWindowsCredentials`), so it never reaches Gravekeeper's own process, the command line or a file, and validates it with `LogonUser` first so a typo cannot lock the user out of interactive sign-in (account types that cannot be checked, such as Microsoft accounts, are trusted). Turning it off sets `AutoAdminLogon = 0`, deletes the LSA secret and removes any plain-text `DefaultPassword` value. The current account is read from the registry without elevation for the System page.

### Power settings (Windows)

**Automatic sleep** and **turning off the display** are both timeouts on the active power scheme, read and written through the Power Management API (`PowerReadACValueIndex` / `PowerWriteACValueIndex` and the DC pair, then `PowerSetActiveScheme` to apply) — the standby timeout (`GUID_STANDBY_TIMEOUT`) and the video timeout (`GUID_VIDEO_POWERDOWN_TIMEOUT`), in seconds, where 0 = never. The AC and DC timeouts are controlled together; turning one off saves its current values under `HKCU\SOFTWARE\Gravekeeper\Power` and sets both to 0, and turning it on restores them (or falls back to 30 min AC / 15 min DC when nothing is saved). These need no elevation and run on the GUI thread, so they are not `elevate` tasks.

**Hibernation** is system-wide (it manages `hiberfil.sys`), so turning it on or off does need elevation and runs in the helper via `powercfg /hibernate on|off` (a `Hibernate` task). The state is read without elevation from `HKLM\SYSTEM\CurrentControlSet\Control\Power\HibernateEnabled` — off only when that value is present and 0.

### Notifications (Windows)

The master "Notifications" switch is per user, so turning it on or off needs no elevation and is applied in-process. It sets the Settings app's toggle `HKCU\…\Notifications\Settings\NOC_GLOBAL_SETTING_TOASTS_ENABLED` and the toast gate `HKCU\…\PushNotifications\ToastEnabled` together (1 = on, 0 = off). It also sets the two "Additional settings" suggestion switches on the same page — Windows tips (`ContentDeliveryManager\SubscribedContent-338389Enabled`) and "finish setting up this device" (`UserProfileEngagement\ScoobeSystemSettingEnabled`) — so turning notifications off silences those too. Notifications are on by default, so they count as off only when the master value is explicitly 0. The notification platform keeps the live state in WNF and only reads these values at start-up — so the Settings page and the actual behavior do not change from the registry write alone — so Gravekeeper then restarts the per-user `WpnUserService` (via PowerShell, no elevation) to make it re-read them. That restart is best-effort; the registry is the durable part and takes effect at the next sign-in regardless.

**Windows Security notifications** (Microsoft Defender) are turned off per category, matching the Security app's own toggles rather than one master switch: virus &amp; threat protection (`SummaryNotificationDisabled`, `NoActionNotificationDisabled`, `FilesBlockedNotificationDisabled`) and the informational toggle (`Notifications\DisableEnhancedNotifications`) are HKLM registry values; the firewall "notify when an app is blocked" toggle goes through its own service, so it is set with `Set-NetFirewallProfile -NotifyOnListen` (a direct registry write is not applied live). These go through the elevated `SecurityNotifications` task. **Account protection notifications are per user (HKCU)**, so that part (`DisableNotifications`, `DisableWindowsHelloNotifications`, `DisableDynamiclockNotifications` under `HKCU\…\Account protection`) is written by `set_security_notifications_user` in Gravekeeper's own process — the elevated helper may run as a different account, whose HKCU is the wrong one. The System page applies the per-user part only after the elevated part succeeds.

### Firewall (Windows)

Turning the firewall itself on or off for all profiles is machine-wide, so it is an elevated `Firewall` task that runs `Set-NetFirewallProfile -Enabled True|False` (same reason as the firewall notification: the firewall service must apply it). The state is read without elevation from the per-profile `EnableFirewall` value (on unless every profile is 0). Turning the firewall off leaves the machine unprotected, so the System page labels it as such.

## Separate supervisor process (Phase 4)

```
GUI process ── local IPC (Named Pipe / Unix Domain Socket) ── Supervisor process
```

- The `interprocess` crate is the IPC candidate. The protocol reuses the types in the `api` module.
- Connections are limited to a per-user socket or pipe, with permissions that reject other users.
- Work that needs a user session, such as screen capture, is kept out of the supervisor.
