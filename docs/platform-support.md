# Platform Support

Legend: ✅ tested on a real machine / 🧪 built and tested in CI only / ⏳ not implemented / ⚠️ has limitations

| Feature | Linux (Wayland) | Linux (X11) | Windows | macOS |
|---|---|---|---|---|
| Window | ✅ KDE Plasma | ✅ via XWayland | 🧪 | 🧪 |
| CJK font fallback | ✅ Noto Sans CJK | ✅ | 🧪 | 🧪 |
| Tray icon and menu | ✅ KDE | ✅ KDE | 🧪 | 🧪 |
| Close to tray | ✅ Destroy | ✅ Destroy / Hide | 🧪 Destroy | 🧪 Hide |
| Start with `--minimized` | ✅ | ✅ | 🧪 | 🧪 |
| Stop process with descendants | ✅ process group | ✅ | 🧪 Job Object | 🧪 process group |
| Graceful stop request | ✅ SIGTERM | ✅ | ⏳ killed immediately | 🧪 SIGTERM |
| Window icon | ✅ with the installed `.desktop` file | ✅ | 🧪 embedded in the exe | 🧪 app bundle |
| Launch at login | ⚠️ implemented, toggle not tested | ⚠️ same | 🧪 HKCU Run | 🧪 LaunchAgent |
| Single instance | ✅ lock file | ✅ | 🧪 | 🧪 |
| Tray host detection | ✅ | ✅ | — (always available) | — (always available) |
| Supervision (start, stop, restart policy, history) | ✅ | ✅ | 🧪 | 🧪 |
| Headless mode (`--headless`, signals) | ✅ | ✅ | 🧪 Ctrl-C only | 🧪 |
| User service (`service install`) | ✅ systemd user (unit verified, not installed) | ✅ same | ⏳ | 🧪 LaunchAgent |
| Children stopped if Gravekeeper is killed | ✅ PDEATHSIG | ✅ | 🧪 Job Object | ⏳ |
| Installer | ✅ .deb / AppImage built and run (x64); 🧪 arm64 (CI) | ✅ same | 🧪 NSIS (CI) | 🧪 dmg (CI) |
| Schedules (incl. DST, missed runs) | ✅ | ✅ | 🧪 | 🧪 |
| Webhooks | ✅ | ✅ | 🧪 | 🧪 |
| CPU / memory / disk monitoring | ✅ | ✅ | 🧪 | 🧪 |
| Boot time (`boot_delay`, after-boot schedules) | ✅ | ✅ | 🧪 | 🧪 |
| Log files and rotation | ✅ | ✅ | 🧪 | 🧪 |
| Administrator rights for OS settings | 🧪 pkexec | 🧪 same | ✅ UAC | 🧪 osascript |
| Automatic updates on/off | ⏳ | ⏳ | ✅ Pro (services + policy); 🧪 Home (services only) | ⏳ |
| Automatic logon on/off | ⏳ | ⏳ | 🧪 Winlogon + LSA secret (password via OS prompt) | ⏳ |
| Automatic sleep on/off | ✅ KDE Plasma 6 (PowerDevil); ⚠️ GNOME (gsettings, untested) | ✅ same | 🧪 power scheme standby timeout (no elevation) | ⏳ |
| Turn off display on/off | ✅ KDE Plasma 6 (PowerDevil); ⚠️ GNOME (gsettings, untested) | ✅ same | 🧪 power scheme video timeout (no elevation) | ⏳ |
| Hibernation on/off | ⏳ | ⏳ | 🧪 `powercfg /hibernate` (elevated) | ⏳ |
| Notifications on/off | ⏳ | ⏳ | 🧪 per-user toast switch (no elevation) | ⏳ |
| Windows Security notifications on/off | ⏳ | ⏳ | 🧪 Defender per-category (HKLM elevated + HKCU account protection) | ⏳ |
| Firewall on/off | ⏳ | ⏳ | 🧪 `Set-NetFirewallProfile -Enabled` (elevated) | ⏳ |

## Limitations and notes

### All platforms

- **Daemonizing applications** (those that fork and exit) are not supported: when the main process exits, its remaining descendants are killed. Run such programs in foreground mode.
- **Launch at login** starts the GUI in the user's session. It does not run Gravekeeper before login; that is the OS service feature planned for Phase 3.

- **Sleep**: schedules notice a suspend/resume within 30 seconds of waking up and apply the missed-run policy to runs that were due while asleep.
- **Disks**: removable drives and pseudo file systems (tmpfs, overlay, squashfs, …) are not monitored.

### Linux

- **Tray**: uses StatusNotifierItem over D-Bus. KDE Plasma, Xfce, Cinnamon and others support it out of the box.
  - GNOME does not by default; it needs the "AppIndicator and KStatusNotifierItem Support" extension.
  - Without a tray host there would be no way to reopen the window after closing it, so Gravekeeper checks for one (`org.kde.StatusNotifierWatcher` on the session bus). If there is none, it skips the tray, always opens the window, and closing the window quits.
- **Wayland**: windows cannot be hidden (a winit limitation). That is why Linux defaults to `Destroy`: closing destroys the window, and a new one is created when needed.
- **Wayland window icon**: taken from the `.desktop` file matching the `app_id` (`gravekeeper`), installed by the `.deb` and by `packaging/linux/install.sh`. When run straight from a build directory or an un-integrated AppImage, the desktop environment shows its default icon.
- **User services** run only while the user is logged in unless lingering is enabled (`loginctl enable-linger`).
- **System settings** (the System page) that belong to the desktop are supported on KDE Plasma 6 and GNOME, detected from `XDG_CURRENT_DESKTOP`; on other desktops they show as unavailable. Values replaced by turning a setting off are saved in `os-settings-backup.json` in the data directory and restored when it is turned on again; with no saved value the desktop's default is restored.
- **Automatic sleep** and **turning off the display**: on KDE, PowerDevil's "When inactive: sleep" action (`AutoSuspendAction`) and "Turn off screen" switch (`TurnOffDisplayWhenIdle`) in `~/.config/powerdevilrc`, for the AC, Battery and Low battery profiles, after which PowerDevil is told over D-Bus to reload; the timeouts are left as they are. On GNOME, `sleep-inactive-ac-type` / `sleep-inactive-battery-type` (set to `nothing`) and the screen-blank delay `org.gnome.desktop.session idle-delay` (set to 0). As on Windows, the battery profile is ignored when judging the state of a computer without a battery. No administrator rights are needed. Plasma 5 is not supported (it stores these differently).
- **Build packages**: on Debian / Ubuntu, `libxkbcommon-dev libwayland-dev libfontconfig1-dev`. GTK and libappindicator are not needed.

### Windows

- **Tray**: needs a message loop on the thread that created it (the main thread). While there is no window, Gravekeeper runs that loop itself, so closing the window destroys it (`Destroy`) and a minimized start creates no window.
- **Stopping processes**: the Job Object is terminated immediately; there is no graceful stop request yet.
- **`wait()` behavior**: does not return until every process in the Job has exited. On Unix it returns as soon as the direct child exits. Until this is reconciled, an application whose main process exits while its descendants keep running is still shown as running on Windows.
- **If Gravekeeper is killed**: the Job Objects are closed with it and the supervised processes are terminated. (On Unix they keep running.)
- **Console**: release builds use `windows_subsystem = "windows"`, so no console window appears — and command-line output (`--help`, `service status`) is not shown either. Run a debug build from a terminal to see it.
- **Service**: `gravekeeper service install` is not supported yet; use "Launch at login".

- **Time zones**: jiff uses its bundled time zone database on Windows.
- **Automatic logon**: the account goes in the Winlogon registry key and the password in the LSA secret `DefaultPassword` (never in plain text), as the Sysinternals Autologon tool does. The elevated helper asks for the password through the Windows credential dialog and validates it with `LogonUser` before saving. On Windows 11 it also sets `DevicePasswordLessBuildVersion = 0` (backed up) so password sign-in, and therefore auto-logon, is allowed. BitLocker without a TPM-only protector, or a Microsoft-account requirement to use Windows Hello, can still prevent it.
- **Automatic sleep** and **turning off the display**: each controls the active power scheme's timeout (standby or video) for both AC and DC. "Off" sets both to never; the previous values are saved under `HKCU\SOFTWARE\Gravekeeper\Power` and restored when turned back on. No administrator rights are needed, matching the Settings app, where a standard user can change these timeouts.
- **Hibernation**: turning it off runs `powercfg /hibernate off`, which disables hibernation and frees `hiberfil.sys`; turning it on runs `powercfg /hibernate on`. This is system-wide and needs administrator rights. The state is read from `HibernateEnabled` in the registry.
- **Notifications**: the master per-user switch; "off" suppresses all app notifications (toasts) for the signed-in user by setting `NOC_GLOBAL_SETTING_TOASTS_ENABLED` and `ToastEnabled` to 0, then restarting the per-user `WpnUserService` so the platform re-reads them (the live state is kept in WNF, not read from the registry except at start-up). No administrator rights are needed. It affects only the user running Gravekeeper, not other accounts.
- **Automatic updates**: turning them off disables the Windows Update services (`wuauserv`, `UsoSvc`, `WaaSMedicSvc`) by setting their `Start` to 4, and on Pro and above also sets two group policies: `NoAutoUpdate = 1` ("Configure Automatic Updates" = Disabled) and `SetDisableUXWUAccess = 1` ("Remove access to use all Windows Update features"). The services make this work on Home too, where the policy is ignored. The original `Start` values are saved under `HKLM\SOFTWARE\Gravekeeper\WindowsUpdate` and restored when updates are turned back on (falling back to the Windows defaults if no backup is found).
  - The visible effect in the Settings page comes from the policies, not the services: `SetDisableUXWUAccess` is what makes "Windows Update" show as managed and blocks the manual "Check for updates". Stopping the services alone changes behaviour but not that page. Both policies only apply on editions that honor them (Pro and above).
  - `WaaSMedicSvc` is the "medic" that otherwise revives `wuauserv`; its security descriptor grants stop and config rights to the Administrators group (verified on Windows 11 Home), so it is disabled as a service rather than by renaming its DLL.
  - To take effect without a reboot, the running services are also stopped (or started again). This is best-effort: on a typical machine the UAC administrator can stop all three, but a build that locks one to SYSTEM/TrustedInstaller refuses. Either way the `Start` change takes full effect at the next boot.
  - The Windows Update **scheduled tasks** (under `\Microsoft\Windows\WindowsUpdate`, `UpdateOrchestrator`, `UpdateAssistant`, `InstallService`, `WaaSMedic` and `\Microsoft\WindowsUpdate`) are disabled too, so the tasks cannot re-start the services. The tasks actually disabled are recorded under `HKLM\SOFTWARE\Gravekeeper\WindowsUpdate\DisabledTasks` and only those are re-enabled when updates are turned back on. This runs through PowerShell's `ScheduledTasks` module and is best-effort: some tasks are owned by SYSTEM/TrustedInstaller and cannot be disabled by the UAC administrator.
  - Nothing is renamed, deleted or has its ownership changed, so the change is fully reversible and does not break Windows servicing. A domain group policy can still override the setting.

### macOS

- **Tray**: must be created on the main thread after the event loop starts, so it is created inside eframe's app-creation callback.
- **Gatekeeper**: signing and notarization of release builds are handled in Phase 3.
- **Disks**: of the system volume's helper mounts under `/System/Volumes/`, only `/System/Volumes/Data` is monitored.
