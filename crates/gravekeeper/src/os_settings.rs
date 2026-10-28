//! Operating system settings for machines that run unattended.
//!
//! Reading a setting needs no special rights. Machine-wide settings need administrator
//! rights to change, so they are changed through [`crate::elevate`]: the `set_*` functions
//! here are what its elevated helper calls.

use std::io;

/// Whether the OS installs updates on its own (its default).
pub fn automatic_updates() -> io::Result<bool> {
    imp::automatic_updates()
}

/// Needs administrator rights.
pub(crate) fn set_automatic_updates(enabled: bool) -> io::Result<()> {
    imp::set_automatic_updates(enabled)
}

/// Turning automatic updates off works on every edition because it disables the Windows
/// Update services (and their scheduled tasks); the group policy, which Windows Home ignores,
/// is only an extra signal.
///
/// Reversibility is deliberate: each service's original `Start` value and the list of tasks
/// actually disabled are saved before the change and restored when updates are turned back
/// on. Unlike some third-party "update blockers", nothing here renames system files, deletes
/// `SoftwareDistribution` or takes ownership of protected keys — those break Windows
/// servicing and cannot be undone cleanly.
#[cfg(windows)]
mod imp {
    use std::io;

    use windows_registry::LOCAL_MACHINE;

    /// The Windows Update group-policy keys. Honored on Pro and above; ignored on Home.
    /// `AU_KEY\NoAutoUpdate = 1` is "Configure Automatic Updates" = Disabled;
    /// `WU_KEY\SetDisableUXWUAccess = 1` is "Remove access to use all Windows Update features".
    const WU_KEY: &str = r"SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate";
    const AU_KEY: &str = r"SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU";
    const NO_AUTO_UPDATE: &str = "NoAutoUpdate";
    const DISABLE_UX_ACCESS: &str = "SetDisableUXWUAccess";

    /// Services driving automatic updates, with the `Start` value to restore when no backup
    /// was taken (Windows defaults: `wuauserv` = manual/3, `UsoSvc` = automatic/2,
    /// `WaaSMedicSvc` = manual/3). `BITS` is left out because other software shares it.
    ///
    /// `WaaSMedicSvc` (the update "medic") is the service that revives `wuauserv`, so it is
    /// included rather than fought with file tricks: its security descriptor grants stop and
    /// config to administrators (verified on Windows 11 Home), so it can be disabled and
    /// restored cleanly. Where a build locks it to SYSTEM/TrustedInstaller the best-effort
    /// changes simply fail and the other two still apply (see docs/platform-support.md).
    const SERVICES: &[(&str, u32)] = &[("wuauserv", 3), ("UsoSvc", 2), ("WaaSMedicSvc", 3)];
    const SERVICE_BASE: &str = r"SYSTEM\CurrentControlSet\Services";
    const START: &str = "Start";
    /// `SERVICE_DISABLED` from the Win32 service start types.
    const DISABLED: u32 = 4;

    /// Where the original `Start` values are kept so the change can be undone exactly. In
    /// HKLM because the elevated helper may run as another account than the user.
    const BACKUP_KEY: &str = r"SOFTWARE\Gravekeeper\WindowsUpdate";

    /// `Ok(None)` when the key or value is absent (not every service exists on every build).
    fn read_u32(key_path: &str, value: &str) -> io::Result<Option<u32>> {
        match LOCAL_MACHINE
            .open(key_path)
            .and_then(|key| key.get_u32(value))
        {
            Ok(found) => Ok(Some(found)),
            Err(err) => {
                let err = io::Error::from(err);
                if err.kind() == io::ErrorKind::NotFound {
                    Ok(None)
                } else {
                    Err(err)
                }
            }
        }
    }

    fn service_start(name: &str) -> io::Result<Option<u32>> {
        read_u32(&format!(r"{SERVICE_BASE}\{name}"), START)
    }

    fn set_service_start(name: &str, value: u32) -> io::Result<()> {
        LOCAL_MACHINE
            .options()
            .read()
            .write()
            .open(format!(r"{SERVICE_BASE}\{name}"))
            .and_then(|key| key.set_u32(START, value))
            .map_err(io::Error::from)
    }

    /// Starts or stops a service now, so the change does not wait for the next boot. The
    /// `Start` value above governs boot; this governs the currently running instance. A
    /// service that is not installed, or already in the wanted state, is treated as done.
    ///
    /// Best-effort: callers ignore the result. On a typical machine an administrator can stop
    /// all of these services (their security descriptors grant SERVICE_STOP to the
    /// Administrators group), but a build that locks one to SYSTEM/TrustedInstaller refuses.
    /// The durable `Start` value still takes full effect at the next boot, so a stop that is
    /// refused only delays the change for that one service.
    fn set_service_running(name: &str, running: bool) -> io::Result<()> {
        use std::time::Duration;

        use windows::Win32::Foundation::{
            ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_NOT_ACTIVE,
        };
        use windows::Win32::System::Services::{
            CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, QueryServiceStatus,
            SC_MANAGER_CONNECT, SERVICE_CONTROL_STOP, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
            SERVICE_START, SERVICE_STATUS, SERVICE_STOP, SERVICE_STOPPED, StartServiceW,
        };
        use windows::core::{HSTRING, PCWSTR};

        fn code(err: &windows::core::Error) -> i32 {
            io::Error::from(err.clone()).raw_os_error().unwrap_or(-1)
        }

        // SAFETY: both handles are closed on every path; the status buffer outlives each call.
        unsafe {
            let scm = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
                .map_err(io::Error::from)?;
            let access = SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS;
            let service = match OpenServiceW(scm, &HSTRING::from(name), access) {
                Ok(service) => service,
                Err(err) => {
                    let _ = CloseServiceHandle(scm);
                    return if code(&err) == ERROR_SERVICE_DOES_NOT_EXIST.0 as i32 {
                        Ok(())
                    } else {
                        Err(io::Error::from(err))
                    };
                }
            };

            let mut status = SERVICE_STATUS::default();
            let outcome = if running {
                match StartServiceW(service, None) {
                    Ok(()) => Ok(()),
                    Err(err) if code(&err) == ERROR_SERVICE_ALREADY_RUNNING.0 as i32 => Ok(()),
                    Err(err) => Err(io::Error::from(err)),
                }
            } else {
                match ControlService(service, SERVICE_CONTROL_STOP, &mut status) {
                    Ok(()) => Ok(()),
                    Err(err) if code(&err) == ERROR_SERVICE_NOT_ACTIVE.0 as i32 => Ok(()),
                    Err(err) => Err(io::Error::from(err)),
                }
            };

            // Give the transition up to ~5 s to finish before reporting back.
            let wanted = if running {
                SERVICE_RUNNING
            } else {
                SERVICE_STOPPED
            };
            if outcome.is_ok() {
                for _ in 0..50 {
                    if QueryServiceStatus(service, &mut status).is_err()
                        || status.dwCurrentState == wanted
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }

            let _ = CloseServiceHandle(service);
            let _ = CloseServiceHandle(scm);
            outcome
        }
    }

    fn policy_disabled() -> io::Result<bool> {
        Ok(read_u32(AU_KEY, NO_AUTO_UPDATE)?.is_some_and(|value| value == 1))
    }

    fn policies_honored() -> io::Result<bool> {
        let edition = LOCAL_MACHINE
            .open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
            .and_then(|key| key.get_string("EditionID"))
            .unwrap_or_default();
        // Home editions report an EditionID starting with "Core".
        Ok(!edition.starts_with("Core"))
    }

    /// Sets a policy value to 1, or removes it to return to the default.
    fn set_policy_value(key_path: &str, value: &str, on: bool) -> io::Result<()> {
        if on {
            return LOCAL_MACHINE
                .create(key_path)
                .and_then(|key| key.set_u32(value, 1))
                .map_err(io::Error::from);
        }
        match LOCAL_MACHINE.options().read().write().open(key_path) {
            Ok(key) => {
                if key.get_u32(value).is_ok() {
                    key.remove_value(value).map_err(io::Error::from)?;
                }
                Ok(())
            }
            Err(err) => {
                let err = io::Error::from(err);
                // No policy key: already the default.
                if err.kind() == io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(err)
                }
            }
        }
    }

    /// Both update policies honored on Pro and above:
    /// - `NoAutoUpdate` ("Configure Automatic Updates" = Disabled) stops automatic updates.
    /// - `SetDisableUXWUAccess` ("Remove access to use all Windows Update features") makes the
    ///   Settings page show that updates are managed and blocks the manual check — this is what
    ///   makes the change visible in the Windows UI, which stopping services alone does not do.
    fn set_policy(disabled: bool) -> io::Result<()> {
        set_policy_value(AU_KEY, NO_AUTO_UPDATE, disabled)?;
        set_policy_value(WU_KEY, DISABLE_UX_ACCESS, disabled)?;
        Ok(())
    }

    fn save_backup(name: &str, value: u32) -> io::Result<()> {
        LOCAL_MACHINE
            .create(BACKUP_KEY)
            .and_then(|key| key.set_u32(name, value))
            .map_err(io::Error::from)
    }

    fn read_backup(name: &str) -> io::Result<Option<u32>> {
        read_u32(BACKUP_KEY, name)
    }

    /// Removes a saved value once it has been restored. Best-effort: a leftover backup is
    /// harmless (it is only read while disabled) and would be overwritten next time.
    fn clear_backup(name: &str) {
        if let Ok(key) = LOCAL_MACHINE.options().read().write().open(BACKUP_KEY) {
            let _ = key.remove_value(name);
        }
    }

    /// Disables (or re-enables) the Windows Update scheduled tasks, which otherwise re-start
    /// the services. It is driven by PowerShell's `ScheduledTasks` module because enumerating
    /// tasks by folder through the Task Scheduler COM API means hand-building `VARIANT`s. The
    /// script records every task it disables under `BACKUP_KEY\DisabledTasks` and, on
    /// re-enable, only turns those back on — tasks already disabled are left as they were.
    ///
    /// The script is a fixed string with no interpolated input, so nothing can be injected,
    /// and it swallows per-task failures: as the UAC administrator (not SYSTEM) some protected
    /// update tasks cannot be disabled, which is expected and left best-effort.
    fn set_update_tasks(disable: bool) -> io::Result<()> {
        use std::os::windows::process::CommandExt;
        use std::process::Command;

        // So no console window flashes from the windowed helper process.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let script = if disable { DISABLE_TASKS } else { ENABLE_TASKS };
        let status = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("powershell exited with {status}")))
        }
    }

    const DISABLE_TASKS: &str = concat!(
        "$ErrorActionPreference='SilentlyContinue';",
        r"$b='HKLM:\SOFTWARE\Gravekeeper\WindowsUpdate';",
        "New-Item -Path $b -Force | Out-Null;",
        "$paths=@(",
        r"'\Microsoft\Windows\WindowsUpdate\*','\Microsoft\WindowsUpdate\*',",
        r"'\Microsoft\Windows\UpdateOrchestrator\*','\Microsoft\Windows\UpdateAssistant\*',",
        r"'\Microsoft\Windows\InstallService\*','\Microsoft\Windows\WaaSMedic\*');",
        "$changed=@();",
        "foreach($p in $paths){foreach($t in Get-ScheduledTask -TaskPath $p){",
        "if($t.State -ne 'Disabled'){try{",
        "Disable-ScheduledTask -TaskPath $t.TaskPath -TaskName $t.TaskName -ErrorAction Stop|Out-Null;",
        "$changed+=\"$($t.TaskPath)$($t.TaskName)\"",
        "}catch{}}}}",
        "if($changed.Count -gt 0){",
        "Set-ItemProperty -Path $b -Name DisabledTasks -Value ([string[]]$changed) -Type MultiString}",
    );

    const ENABLE_TASKS: &str = concat!(
        "$ErrorActionPreference='SilentlyContinue';",
        r"$b='HKLM:\SOFTWARE\Gravekeeper\WindowsUpdate';",
        "$list=(Get-ItemProperty -Path $b -Name DisabledTasks).DisabledTasks;",
        "foreach($full in $list){$i=$full.LastIndexOf('\\');",
        "if($i -ge 0){$path=$full.Substring(0,$i+1);$name=$full.Substring($i+1);",
        "Enable-ScheduledTask -TaskPath $path -TaskName $name|Out-Null}}",
        "Remove-ItemProperty -Path $b -Name DisabledTasks",
    );

    pub fn automatic_updates() -> io::Result<bool> {
        if policies_honored()? && policy_disabled()? {
            return Ok(false);
        }
        for (name, _) in SERVICES {
            if service_start(name)? == Some(DISABLED) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn set_automatic_updates(enabled: bool) -> io::Result<()> {
        if enabled {
            for (name, default) in SERVICES {
                if service_start(name)?.is_some() {
                    // Restore the boot setting before dropping the backup, so a failure here
                    // never leaves the service disabled with no saved value to recover.
                    let restore = read_backup(name)?.unwrap_or(*default);
                    set_service_start(name, restore)?;
                    clear_backup(name);
                    // Bring it back now so updates resume without a reboot (best-effort).
                    let _ = set_service_running(name, true);
                }
            }
            set_policy(false)?;
            // Re-enable the scheduled tasks we disabled (best-effort).
            let _ = set_update_tasks(false);
        } else {
            for (name, _) in SERVICES {
                if let Some(current) = service_start(name)? {
                    if current != DISABLED {
                        save_backup(name, current)?;
                    }
                    set_service_start(name, DISABLED)?;
                    // `Start` only governs the next boot, so stop the running instance too
                    // (best-effort; see set_service_running).
                    let _ = set_service_running(name, false);
                }
            }
            // An extra, edition-aware signal; the services above do the actual work.
            if policies_honored()? {
                set_policy(true)?;
            }
            // The update "medic" can re-start wuauserv; disabling its and the orchestrator's
            // scheduled tasks keeps updates from creeping back (best-effort).
            let _ = set_update_tasks(true);
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "not supported on this platform yet",
        )
    }

    pub fn automatic_updates() -> io::Result<bool> {
        Err(unsupported())
    }

    pub fn set_automatic_updates(_enabled: bool) -> io::Result<()> {
        Err(unsupported())
    }
}
