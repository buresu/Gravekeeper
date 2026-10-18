# Platform Support

Legend: ✅ tested on a real machine / 🧪 built and tested in CI only / ⏳ not implemented / ⚠️ has limitations

| Feature | Linux (Wayland) | Linux (X11) | Windows | macOS |
|---|---|---|---|---|
| Window | ✅ KDE Plasma | ✅ via XWayland | 🧪 | 🧪 |
| CJK font fallback | ✅ Noto Sans CJK | ✅ | 🧪 | 🧪 |
| Tray icon and menu | ✅ KDE | ✅ KDE | 🧪 | 🧪 |
| Close to tray | ✅ Destroy | ✅ Destroy / Hide | 🧪 Hide | 🧪 Hide |
| Start with `--minimized` | ✅ | ✅ | 🧪 | 🧪 |
| Stop process with descendants | ✅ process group | ✅ | 🧪 Job Object | 🧪 process group |
| Graceful stop request | ✅ SIGTERM | ✅ | ⏳ killed immediately | 🧪 SIGTERM |
| Window icon | ⚠️ needs a `.desktop` file | ✅ | 🧪 | 🧪 |
| Launch at login | ⏳ | ⏳ | ⏳ | ⏳ |

## Limitations and notes

### Linux

- **Tray**: uses StatusNotifierItem over D-Bus. KDE Plasma, Xfce, Cinnamon and others support it out of the box.
  - GNOME does not by default; it needs the "AppIndicator and KStatusNotifierItem Support" extension.
  - Without a tray host there is no way to reopen the window after closing it. Detection and fallback to `Quit` are planned for Phase 1.
- **Wayland**: windows cannot be hidden (a winit limitation). That is why Linux defaults to `Destroy`: closing destroys the window, and a new one is created when needed.
- **Wayland window icon**: taken from the `.desktop` file matching the `app_id` (`gravekeeper`). Until one is installed, the desktop environment shows its default icon.
- **Build packages**: on Debian / Ubuntu, `libxkbcommon-dev libwayland-dev libfontconfig1-dev`. GTK and libappindicator are not needed.

### Windows

- **Tray**: needs a message loop on the main thread, so the window is hidden while the event loop keeps running (`Hide`).
- **Stopping processes**: the Job Object is terminated immediately. A graceful stop request is planned for Phase 1.
- **`wait()` behavior**: does not return until every process in the Job has exited. On Unix it returns as soon as the direct child exits. Phase 1 will reconcile this difference.
- **Console**: release builds use `windows_subsystem = "windows"`, so no console window appears.

### macOS

- **Tray**: must be created on the main thread after the event loop starts, so it is created inside eframe's app-creation callback.
- **Gatekeeper**: signing and notarization of release builds are handled in Phase 3.
