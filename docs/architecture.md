# Architecture

## Overview

```
gravekeeper (bin: eframe GUI, tray, main)
    │  CoreHandle::send(Command) / snapshot() -> Snapshot   ← event subscription comes later
    ▼
gravekeeper-core (lib: no GUI dependency)
    ├─ engine      Dedicated thread + Tokio current-thread runtime. Handles Commands, updates the Snapshot
    ├─ api         Command / Snapshot (serde-enabled; becomes the IPC protocol in Phase 4)
    ├─ process     Spawn, stop, process-tree termination, stdout/stderr capture
    ├─ supervisor  (Phase 1) Per-app state machine and restart policy
    ├─ config      (Phase 1) TOML read/write, validation, schema migration
    ├─ store       (Phase 1) SQLite history on its own thread; degrades gracefully on failure
    ├─ scheduler / monitoring / notify   (Phase 2)
    └─ platform/{windows,macos,linux}    (Phase 1+) OS-specific implementations
```

### Boundary rules

- `gravekeeper-core` does not depend on egui, eframe or tray-icon. Splitting the crates lets the compiler enforce this.
- The GUI never touches engine internals. It reads a `Snapshot` (kept small enough to copy every frame) and changes things only by sending a `Command`.
- The engine asks the GUI to redraw through a `Notifier` (`Fn() + Send + Sync`) passed in at startup. The GUI passes `egui::Context::request_repaint`.
- All message types are serde-enabled, so moving the engine into its own process (Phase 4) only means swapping the transport from a channel to local IPC.

## Threads

| Thread | Role |
|---|---|
| main | eframe / winit event loop (must be the main thread on macOS) |
| gravekeeper-core | Tokio current-thread runtime: process supervision and timers |
| tray (Linux only) | D-Bus thread owned by tray-icon's ksni backend |

The GUI thread never waits on I/O. All engine work runs as async tasks, including the continuous reading of child process output.

## Window and tray lifecycle

What closing the main window does (`CloseBehavior`) has a per-OS default.

| Value | Behavior | Default on | Why |
|---|---|---|---|
| `Hide` | `CancelClose` + `Visible(false)`; the event loop keeps running | Windows / macOS | The tray depends on the main-thread event loop, so the loop cannot stop |
| `Destroy` | Destroy the window and return from `run_native_ext`; a new window is created when the tray asks for one | Linux | winit cannot hide windows on Wayland. ksni runs on its own thread, so the tray keeps working without the loop |
| `Quit` | Closing the window exits | When tray creation failed | A hidden window could never be shown again |

- The same `egui::Context` is passed to every `run_native_ext` call, so the `Notifier` stays valid when the window is recreated.
- Tray callbacks may run on any thread. `Shell::handle` sends a `ViewportCommand` while a window exists, and otherwise (`Destroy` mode) sends the action to the main loop's channel.
- The environment variable `GRAVEKEEPER_CLOSE_BEHAVIOR=hide|destroy|quit` overrides the default (for testing).

## Process control

- Children start in a new process group on Unix and inside a Job Object on Windows (via `process-wrap`), so stopping a child also stops its descendants.
- `KillOnDrop` is applied, so dropping a `ManagedChild` kills the whole tree.
- stdout / stderr are captured line by line into a `LogBuffer`, a ring buffer with a line limit. The writer never waits for readers, so a chatty child cannot block on a full pipe.
  - A line is at most 16 KiB; anything longer is split into the next line.
  - Invalid UTF-8 is replaced with U+FFFD.
  - Each line gets a sequence number (`seq`) so the GUI can fetch only new lines.
- Stopping: on Unix, SIGTERM to the process group, then SIGKILL after the grace period. On Windows the Job Object is currently terminated immediately (graceful stop is a Phase 1 task).
- How a process ended is recorded as `ExitKind` (exit code / signal / unknown). Why it ended (user stop, timeout, crash) is decided by the Phase 1 supervisor, which combines `ExitKind` with its own intent. `looks_like_crash()` is only a hint.

### Supervised app state machine (Phase 1)

```
Stopped ─start→ Starting ─(alive until start_timeout)→ Running ─stop→ Stopping → Stopped
Running/Starting ─unexpected exit→ Backoff ─after delay→ Starting
                                   └─limit exceeded→ GaveUp (notify and stop)
Disabled (not supervised) / Paused (supervision suspended)
```

Restart policy: `delay = min(base * 2^n, max) + jitter`. Running for at least `stable_after` resets the failure count. Exceeding `max_restarts` within `window` moves the app to `GaveUp`.

## Separate supervisor process (Phase 4)

```
GUI process ── local IPC (Named Pipe / Unix Domain Socket) ── Supervisor process
```

- The `interprocess` crate is the IPC candidate. The protocol reuses the types in the `api` module.
- Connections are limited to a per-user socket or pipe, with permissions that reject other users.
- Work that needs a user session, such as screen capture, is kept out of the supervisor.
