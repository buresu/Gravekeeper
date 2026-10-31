# Platform Support

Legend: ✅ tested on a real machine / 🧪 built and tested in CI only / ⏳ not implemented / ⚠️ has limitations

| Feature | Linux (Wayland) | Linux (X11) | Windows | macOS |
|---|---|---|---|---|
| Window | ✅ KDE Plasma | ✅ via XWayland | 🧪 | 🧪 |
| CJK font fallback | ✅ Noto Sans CJK | ✅ | 🧪 | 🧪 |
| Tray icon and menu | ✅ KDE | ✅ KDE | 🧪 | 🧪 |
| Close to tray | ✅ Destroy | ✅ Destroy / Hide | 🧪 Destroy | ✅ Hide (Dock icon hidden while closed) |
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
| Automatic updates on/off | ⚠️ apt / dnf-automatic timers (elevated; untested) | ⚠️ same | ✅ Pro (services + policy); 🧪 Home (services only) | ⚠️ Software Update preferences (elevated; reading verified, change untested) |
| Automatic logon on/off | ⚠️ Plasma Login Manager, GDM (elevated; not yet run through pkexec, GDM untested) | ⚠️ same | 🧪 Winlogon + LSA secret (password via OS prompt) | ⚠️ `autoLoginUser` + `/etc/kcpassword` (elevated, password via dialog and checked with PAM; reading verified, change untested; not with FileVault) |
| Automatic sleep on/off | ✅ KDE Plasma 6 (PowerDevil); ⚠️ GNOME (gsettings, untested) | ✅ same | 🧪 power scheme standby timeout (no elevation) | ⚠️ `pmset sleep` (elevated; reading verified, change untested) |
| Turn off display on/off | ✅ KDE Plasma 6 (PowerDevil); ⚠️ GNOME (gsettings, untested) | ✅ same | 🧪 power scheme video timeout (no elevation) | ⚠️ `pmset displaysleep` (elevated; reading verified, change untested) |
| Turn off hard disks on/off | — | — | ✅ power scheme disk timeout (no elevation) | ⚠️ `pmset disksleep` (elevated; reading verified, change untested; hard disk drives only) |
| Hibernation on/off | ⚠️ systemd `sleep.conf.d` drop-in (elevated; not yet run through pkexec) | ⚠️ same | 🧪 `powercfg /hibernate` (elevated) | ⚠️ `pmset hibernatemode` (elevated; untested; unavailable on desktops with Apple silicon) |
| Notifications on/off | ✅ KDE Plasma 6 (Do not disturb); ⚠️ GNOME (`show-banners`, untested) | ✅ same | 🧪 per-user toast switch (no elevation) | — (Focus has no interface for other apps) |
| Windows Security notifications on/off | — | — | 🧪 Defender per-category (HKLM elevated + HKCU account protection) | — |
| Firewall on/off | ⚠️ ufw or firewalld (elevated; untested) | ⚠️ same | 🧪 `Set-NetFirewallProfile -Enabled` (elevated) | ⚠️ application firewall, `socketfilterfw` (elevated; reading verified, change untested) |
| App Nap on/off | — | — | — | ✅ `NSAppSleepDisabled` (no elevation) |
| Require sign-in on wake on/off | — | — | ⚠️ power scheme `CONSOLELOCK` (elevated; reading verified, change untested) | — |
| Lock screen on/off | — | — | ⚠️ `NoLockScreen` policy (elevated; reading verified, change untested) | — |
| Lock when inactive on/off | — | — | ⚠️ `InactivityTimeoutSecs` policy (elevated; reading verified, change untested) | — |
| Wake timers on/off | — | — | ✅ power scheme `RTCWAKE` (no elevation) | — |
| Schedules that wake the computer | — | — | ⚠️ waitable timer (the timer is tested; waking from real sleep is untested) | — |
| Screen saver on/off | — | — | ⚠️ `SPI_SETSCREENSAVEACTIVE` (no elevation; turning off tested, turning on with a screen saver chosen untested) | ✅ `com.apple.screensaver idleTime` (per computer, no elevation) |
| Reopen apps at login on/off | — | — | — | ⚠️ `TALLogoutSavesState`, `LoginwindowLaunchesRelaunchApps` (no elevation; reading verified, change untested) |
| Start up after a power failure on/off | — | — | — | ⚠️ `pmset autorestart` (elevated; reading verified, change untested; desktops only) |
| Wake for network access on/off | — | — | ⚠️ per adapter (elevated; reading verified, change untested) | ⚠️ `pmset womp` (elevated; reading verified, change untested) |
| Bluetooth Setup Assistant at startup on/off | — | — | — | ⚠️ `BluetoothAutoSeekKeyboard` / `…PointingDevice` (elevated; reading verified, change untested) |

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
- **Automatic updates**: Linux has no single switch, so Gravekeeper turns off the systemd timers that update on their own: `apt-daily.timer` and `apt-daily-upgrade.timer` on Debian and Ubuntu (these also run unattended-upgrades) and the `dnf-automatic` / `dnf5-automatic` timers on Fedora and RHEL. The timers it disables are recorded in `/var/lib/gravekeeper/disabled-update-units` and only those are re-enabled; with no record the usual timers are enabled. Nothing is masked or edited. Distributions without these timers, such as Arch, do not update on their own, and the setting shows as unavailable. Software centres (Discover, GNOME Software) and snap refreshes have their own update settings, which are not changed. Needs administrator rights.
- **Firewall**: ufw or firewalld, whichever is installed (if both, the one that is on). ufw is switched with `ufw --force enable|disable` and read from `ENABLED` in `/etc/ufw/ufw.conf`; turning it on also enables `ufw.service` where it is disabled (as Arch ships it), so the firewall comes back after a reboot; firewalld with `systemctl enable|disable --now firewalld` and read from its unit. Needs administrator rights. With neither installed the setting shows as unavailable.
- **Automatic logon** is set in the display manager's configuration and needs administrator rights. Supported are Plasma Login Manager (`[Autologin] User` and `Session` in `/etc/plasmalogin.conf`) and GDM (`[daemon] AutomaticLoginEnable` and `AutomaticLogin` in `/etc/gdm/custom.conf`, or `/etc/gdm3/custom.conf` on Debian and Ubuntu); the display manager is found from `display-manager.service`, and others show as unavailable. The account is always the one running Gravekeeper (from pkexec's `PKEXEC_UID`); root is refused. No password is stored. Because no password is typed, KWallet and the GNOME keyring are not unlocked at sign-in, so apps that keep secrets there will ask for it. For Plasma Login Manager the session is the one already configured, else `plasma.desktop`.
- **Hibernation**: turning it off writes `/etc/systemd/sleep.conf.d/90-gravekeeper-hibernation.conf` with `AllowHibernation=no` (and `AllowSuspendThenHibernate=no`, `AllowHybridSleep=no`, which hibernate too); turning it on deletes that file, or sets the three to `yes` if other configuration still forbids hibernation. The state is the effective `AllowHibernation` from `systemd-analyze cat-config systemd/sleep.conf`. It needs administrator rights. Unlike Windows no disk space is freed (the swap is left as it is). Desktops that ask logind whether hibernation is possible only at start-up may keep offering it until the next login.
- **Notifications**: "off" is the desktop's Do not disturb mode, so notifications still reach the history but show no pop-ups. Plasma has no permanent switch, only Do not disturb until a date (`[DoNotDisturb] Until` in `~/.config/plasmanotifyrc`; its own "until manually disabled" is one year ahead), so Gravekeeper sets the year 9999; the tray shows Do not disturb as lasting until then. Turning notifications on removes the date. On GNOME it is `org.gnome.desktop.notifications show-banners`. No administrator rights are needed.
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
- **Automatic sleep**, **turning off the display** and **turning off hard disks**: each controls the active power scheme's timeout (standby, video or disk) for both AC and DC. "Off" sets both to never; the previous values are saved under `HKCU\SOFTWARE\Gravekeeper\Power` and restored when turned back on. No administrator rights are needed, matching the Settings app, where a standard user can change these timeouts.
- **Require sign-in on wake**: the active power scheme's `CONSOLELOCK` for both AC and DC. Unlike the timeouts, Windows refuses this to a process without administrator rights (verified), so it asks for them. A group policy can override it.
- **Lock screen**: the `NoLockScreen` policy in HKLM, set to 1 when off and removed when on. Asks for administrator rights. Support across Windows editions could not be confirmed from Microsoft's documentation, so the lock screen may still appear on some.
- **Lock when inactive**: the `InactivityTimeoutSecs` policy in HKLM. Off sets it to 0 and saves the previous limit under `HKLM\SOFTWARE\Gravekeeper\Lock`; on restores it, or sets 15 minutes. Asks for administrator rights.
- **Screen saver**: counts as on only if `ScreenSaveActive` is not 0 and a screen saver is chosen, so a default Windows with none chosen shows it as off. Turning it on with none chosen reports an error rather than choosing one. A group policy under `HKCU\Software\Policies` takes precedence and is reported as such when changing it.
- **Wake for network access**: applied to each physical network adapter: `*WakeOnMagicPacket`, the adapter's power management and its "allow this device to wake the computer" switch (`MSPower_DeviceWakeEnable`), and "only wake with a magic packet" when turning it on. Other devices are left alone. The state is on when `powercfg /devicequery wake_armed` lists an adapter that does not have magic packets turned off, so an armed Wi-Fi adapter counts. To keep the network up, adapters are not restarted, so a driver may apply its part after the next restart. Waking also depends on the firmware. Asks for administrator rights.
- **Wake timers**: the active power scheme's `RTCWAKE` for both AC and DC, on (1) or off (0). "Important wake timers only" (2) shows as off, since it does not let Gravekeeper's timer through, and is not restored by turning it on. No administrator rights are needed.
- **Schedules that wake the computer**: a waitable timer with its resume flag, set a minute before the run. It belongs to the Gravekeeper process, so it needs no rights but only works while Gravekeeper runs. Some computers, especially with Modern Standby, or virtual machines may not honor it; where the OS reports that, an event says so.
- **Hibernation**: turning it off runs `powercfg /hibernate off`, which disables hibernation and frees `hiberfil.sys`; turning it on runs `powercfg /hibernate on`. This is system-wide and needs administrator rights. The state is read from `HibernateEnabled` in the registry.
- **Notifications**: the master per-user switch; "off" suppresses all app notifications (toasts) for the signed-in user by setting `NOC_GLOBAL_SETTING_TOASTS_ENABLED` and `ToastEnabled` to 0, then restarting the per-user `WpnUserService` so the platform re-reads them (the live state is kept in WNF, not read from the registry except at start-up). No administrator rights are needed. It affects only the user running Gravekeeper, not other accounts.
- **Automatic updates**: turning them off disables the Windows Update services (`wuauserv`, `UsoSvc`, `WaaSMedicSvc`) by setting their `Start` to 4, and on Pro and above also sets two group policies: `NoAutoUpdate = 1` ("Configure Automatic Updates" = Disabled) and `SetDisableUXWUAccess = 1` ("Remove access to use all Windows Update features"). The services make this work on Home too, where the policy is ignored. The original `Start` values are saved under `HKLM\SOFTWARE\Gravekeeper\WindowsUpdate` and restored when updates are turned back on (falling back to the Windows defaults if no backup is found).
  - The visible effect in the Settings page comes from the policies, not the services: `SetDisableUXWUAccess` is what makes "Windows Update" show as managed and blocks the manual "Check for updates". Stopping the services alone changes behaviour but not that page. Both policies only apply on editions that honor them (Pro and above).
  - `WaaSMedicSvc` is the "medic" that otherwise revives `wuauserv`; its security descriptor grants stop and config rights to the Administrators group (verified on Windows 11 Home), so it is disabled as a service rather than by renaming its DLL.
  - To take effect without a reboot, the running services are also stopped (or started again). This is best-effort: on a typical machine the UAC administrator can stop all three, but a build that locks one to SYSTEM/TrustedInstaller refuses. Either way the `Start` change takes full effect at the next boot.
  - The Windows Update **scheduled tasks** (under `\Microsoft\Windows\WindowsUpdate`, `UpdateOrchestrator`, `UpdateAssistant`, `InstallService`, `WaaSMedic` and `\Microsoft\WindowsUpdate`) are disabled too, so the tasks cannot re-start the services. The tasks actually disabled are recorded under `HKLM\SOFTWARE\Gravekeeper\WindowsUpdate\DisabledTasks` and only those are re-enabled when updates are turned back on. This runs through PowerShell's `ScheduledTasks` module and is best-effort: some tasks are owned by SYSTEM/TrustedInstaller and cannot be disabled by the UAC administrator.
  - Nothing is renamed, deleted or has its ownership changed, so the change is fully reversible and does not break Windows servicing. A domain group policy can still override the setting.

### macOS

- **Tray**: must be created on the main thread after the event loop starts, so it is created inside eframe's app-creation callback. It shows only the icon in the menu bar, without the app's name.
- **Dock**: the Dock icon is shown only while the window is; with the window closed (or after a minimized start) Gravekeeper is reachable only from the menu bar.
- **Gatekeeper**: signing and notarization of release builds are handled in Phase 3.
- **Disks**: of the system volume's helper mounts under `/System/Volumes/`, only `/System/Volumes/Data` is monitored.
- **System settings**: unlike Windows and Linux, the power settings are machine-wide on macOS (System Settings asks for an administrator to change them), so automatic sleep and turning off the display ask for administrator rights too. Values replaced by turning a setting off are recorded in `/Library/Application Support/Gravekeeper/os-settings-backup.json` (written by the elevated helper) and restored when it is turned on again. Settings pushed by a configuration profile (MDM) override these and are not shown.
- **Automatic sleep**, **turning off the display** and **turning off hard disks**: `pmset`'s `sleep`, `displaysleep` and `disksleep` for each power source the Mac has (AC, battery, UPS), set to 0 (never). With nothing recorded, turning them on sets 30 / 15 minutes of system sleep (AC / battery), the macOS defaults of 10 / 2 minutes for the display and 10 minutes for disks. Disk sleep only spins down hard disk drives, such as external ones; SSDs are not affected.
- **Start up after a power failure** and **wake for network access**: `pmset autorestart` and `womp` (1 = on) for each power source. Laptops have no `autorestart`, and the setting then shows as unavailable. Both need administrator rights.
- **Hibernation**: `pmset hibernatemode` 0 (sleep only) for each power source, and the sleep image `/var/vm/sleepimage` is deleted; turning it on restores the recorded mode, else 3 (the laptop default). Desktops with Apple silicon do not hibernate, and the setting shows as unavailable.
- **Automatic updates**: the five switches under Software Update > Automatic Updates in `/Library/Preferences/com.apple.SoftwareUpdate` (`AutomaticCheckEnabled`, `AutomaticDownload`, `AutomaticallyInstallMacOSUpdates`, `CriticalUpdateInstall`, `ConfigDataInstall`) are all turned off; turning updates on restores the recorded values, or with no record removes the keys so the macOS defaults apply. App Store app updates are not changed.
- **Automatic logon**: `autoLoginUser` in `/Library/Preferences/com.apple.loginwindow` and the password in `/etc/kcpassword` (readable by root only), as System Settings writes them. That file is only obfuscated with a fixed key, not encrypted, which is how macOS itself stores it. The account is the one signed in at the screen; the elevated helper asks for its password in a dialog and checks it through PAM (`checkpw`) before saving. macOS does not sign in automatically while FileVault is on, so the setting then shows as unavailable.
- **Firewall**: the application firewall (System Settings > Network > Firewall), through `socketfilterfw --setglobalstate`. The packet filter (`pf`) is not changed.
- **Notifications**: Focus (Do Not Disturb) has no interface other apps can use since macOS 12, so the setting shows as unavailable.
- **Screen saver**: `idleTime` (seconds) in the user's per-computer `com.apple.screensaver` (`defaults -currentHost`), which Lock Screen > "Start Screen Saver when inactive" sets; off is 0. The value replaced is saved in `os-settings-backup.json` in the user's data directory. No administrator rights are needed. The Lock Screen's "Require password after screen saver begins or display is turned off" is not offered: the old `askForPassword` preference has been ignored since macOS 10.13, and its replacement (`sysadminctl -screenLock`) takes the password on its command line; with the screen saver, display off and sleep all off, the screen never locks anyway.
- **Reopen apps at login**: off sets `TALLogoutSavesState` (the "Reopen windows when logging back in" checkbox of the restart and shut-down dialogs, whose last state macOS remembers there) and `LoginwindowLaunchesRelaunchApps` to false in the user's `com.apple.loginwindow`; either being off counts as off. This keeps macOS from starting apps that Gravekeeper starts itself a second time after a restart. No administrator rights are needed.
- **Bluetooth Setup Assistant**: off sets `BluetoothAutoSeekKeyboard` and `BluetoothAutoSeekPointingDevice` in `/Library/Preferences/com.apple.Bluetooth` to 0, so a Mac without a keyboard or mouse does not open the assistant over the screen at startup. Needs administrator rights; applies at the next startup.
- **Not offered**: Gatekeeper (`spctl --master-disable` needs confirming in System Settings since macOS 15, and it lowers security for every app rather than helping unattended operation), Power Nap (moot with sleep off), scheduled power events (`pmset repeat`; one-off rather than a switch, and Gravekeeper has its own schedules) and the Bluetooth controller's power (no supported command-line interface).
- **App Nap**: turning it off sets `NSAppSleepDisabled` in the user's global preferences, which every app reads at launch, so it applies to apps started afterwards (restart supervised applications for it to take effect). Turning it on removes the key. No administrator rights are needed.
