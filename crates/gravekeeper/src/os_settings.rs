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

/// The account set to log in automatically, or `None` when automatic logon is off.
pub fn auto_login() -> io::Result<Option<String>> {
    imp::auto_login()
}

/// Whether the computer sleeps on its own after a period of inactivity.
pub fn automatic_sleep() -> io::Result<bool> {
    imp::automatic_sleep()
}

/// Turns automatic sleep on or off. Unlike the other settings this needs no administrator
/// rights — a standard user may change their own power timeouts — so it runs in-process.
pub fn set_automatic_sleep(enabled: bool) -> io::Result<()> {
    imp::set_automatic_sleep(enabled)
}

/// Whether the display turns off on its own after a period of inactivity.
pub fn display_off() -> io::Result<bool> {
    imp::display_off()
}

/// Turns the display-off timeout on or off. Needs no administrator rights (in-process).
pub fn set_display_off(enabled: bool) -> io::Result<()> {
    imp::set_display_off(enabled)
}

/// Whether hibernation is enabled on this computer.
pub fn hibernate_enabled() -> io::Result<bool> {
    imp::hibernate_enabled()
}

/// Enables or disables hibernation. Needs administrator rights (goes through `elevate`).
pub(crate) fn set_hibernate(enabled: bool) -> io::Result<()> {
    imp::set_hibernate(enabled)
}

/// Whether notifications (toasts) are shown for the current user.
pub fn notifications_enabled() -> io::Result<bool> {
    imp::notifications_enabled()
}

/// Turns notifications on or off for the current user. Needs no administrator rights.
pub fn set_notifications(enabled: bool) -> io::Result<()> {
    imp::set_notifications(enabled)
}

/// Needs administrator rights. When enabling, the elevated helper collects the password
/// through the OS credential prompt; it is never passed in or stored in plain text.
pub(crate) fn set_auto_login(enabled: bool) -> io::Result<()> {
    imp::set_auto_login(enabled)
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

    // --- Automatic logon -----------------------------------------------------------------
    //
    // The user name and domain go in the Winlogon registry key; the password is kept as the
    // LSA secret `DefaultPassword`, exactly as the Sysinternals Autologon tool does, never in
    // the plain-text `DefaultPassword` registry value. The password is collected inside this
    // elevated helper through the OS credential dialog, so it never reaches Gravekeeper's own
    // process or the command line, and is validated with `LogonUser` before it is stored so a
    // typo cannot lock the user out of interactive sign-in.

    const WINLOGON_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon";
    const AUTO_ADMIN_LOGON: &str = "AutoAdminLogon";
    const DEFAULT_USER: &str = "DefaultUserName";
    const DEFAULT_DOMAIN: &str = "DefaultDomainName";
    /// The plain-text password value. We never create it, and remove it if something else did.
    const DEFAULT_PASSWORD: &str = "DefaultPassword";
    /// Windows 11 hides password sign-in (and ignores auto-logon) while this is 2; 0 re-enables
    /// it. Saved and restored so the machine's original choice is not lost.
    const PASSWORDLESS_KEY: &str =
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\PasswordLess\Device";
    const PASSWORDLESS_VALUE: &str = "DevicePasswordLessBuildVersion";
    /// The LSA secret Winlogon reads the password from.
    const LSA_SECRET: &str = "DefaultPassword";

    pub fn auto_login() -> io::Result<Option<String>> {
        let Ok(key) = LOCAL_MACHINE.open(WINLOGON_KEY) else {
            return Ok(None);
        };
        let on = key
            .get_string(AUTO_ADMIN_LOGON)
            .is_ok_and(|value| value.trim() == "1");
        if !on {
            return Ok(None);
        }
        let user = key.get_string(DEFAULT_USER).unwrap_or_default();
        Ok(Some(user))
    }

    pub fn set_auto_login(enabled: bool) -> io::Result<()> {
        if enabled {
            enable_auto_login()
        } else {
            disable_auto_login()
        }
    }

    fn disable_auto_login() -> io::Result<()> {
        let key = LOCAL_MACHINE
            .create(WINLOGON_KEY)
            .map_err(io::Error::from)?;
        key.set_string(AUTO_ADMIN_LOGON, "0")
            .map_err(io::Error::from)?;
        // Never leave a plain-text password behind.
        let _ = key.remove_value(DEFAULT_PASSWORD);
        // Remove the stored secret.
        lsa_store_password(None)?;
        // Restore the passwordless setting we may have changed.
        if let Some(previous) = read_backup_u32(PASSWORDLESS_BACKUP)? {
            let _ = set_passwordless(previous);
            clear_backup(PASSWORDLESS_BACKUP);
        }
        Ok(())
    }

    fn enable_auto_login() -> io::Result<()> {
        let creds = prompt_credentials()?;
        validate_credentials(&creds)?;

        let key = LOCAL_MACHINE
            .create(WINLOGON_KEY)
            .map_err(io::Error::from)?;
        key.set_string(AUTO_ADMIN_LOGON, "1")
            .map_err(io::Error::from)?;
        key.set_string(DEFAULT_USER, &creds.user)
            .map_err(io::Error::from)?;
        key.set_string(DEFAULT_DOMAIN, &creds.domain)
            .map_err(io::Error::from)?;
        // The password lives only in the LSA secret, never in this value.
        let _ = key.remove_value(DEFAULT_PASSWORD);

        // On Windows 11, allow password sign-in so auto-logon is honored (best-effort; save
        // the old value so it can be put back).
        if let Ok(Some(previous)) = read_u32(PASSWORDLESS_KEY, PASSWORDLESS_VALUE)
            && previous != 0
        {
            let _ = save_backup(PASSWORDLESS_BACKUP, previous);
            let _ = set_passwordless(0);
        }

        lsa_store_password(Some(&creds.password))?;
        Ok(())
    }

    /// A backup value name (reuses the `BACKUP_KEY` store shared with the update settings).
    const PASSWORDLESS_BACKUP: &str = "PasswordLessBuildVersion";

    fn read_backup_u32(name: &str) -> io::Result<Option<u32>> {
        read_u32(BACKUP_KEY, name)
    }

    fn set_passwordless(value: u32) -> io::Result<()> {
        LOCAL_MACHINE
            .create(PASSWORDLESS_KEY)
            .and_then(|key| key.set_u32(PASSWORDLESS_VALUE, value))
            .map_err(io::Error::from)
    }

    /// Credentials from the prompt. The password buffer is wiped when this is dropped.
    struct Credentials {
        user: String,
        domain: String,
        /// Null-terminated UTF-16, as the Win32 logon and LSA calls want it.
        password: Vec<u16>,
    }

    impl Drop for Credentials {
        fn drop(&mut self) {
            self.password.iter_mut().for_each(|c| *c = 0);
        }
    }

    /// Shows the OS "Windows Security" credential dialog and returns what was entered.
    fn prompt_credentials() -> io::Result<Credentials> {
        use std::ffi::c_void;

        use windows::Win32::Foundation::{
            ERROR_CANCELLED, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS,
        };
        use windows::Win32::Security::Credentials::{
            CRED_PACK_GENERIC_CREDENTIALS, CREDUI_INFOW, CREDUIWIN_GENERIC,
            CredUIPromptForWindowsCredentialsW, CredUnPackAuthenticationBufferW,
        };
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::core::{HSTRING, PCWSTR, PWSTR};

        let caption = HSTRING::from("Gravekeeper");
        let message =
            HSTRING::from("Enter the password of the account to sign in automatically at startup.");
        let info = CREDUI_INFOW {
            cbSize: size_of::<CREDUI_INFOW>() as u32,
            pszCaptionText: PCWSTR(caption.as_ptr()),
            pszMessageText: PCWSTR(message.as_ptr()),
            ..Default::default()
        };

        let mut auth_package = 0u32;
        let mut out_buffer: *mut c_void = std::ptr::null_mut();
        let mut out_size = 0u32;

        // SAFETY: `info` and its strings outlive the call; the out buffer is freed below.
        let result = unsafe {
            CredUIPromptForWindowsCredentialsW(
                Some(&info),
                0,
                &mut auth_package,
                None,
                0,
                &mut out_buffer,
                &mut out_size,
                None,
                CREDUIWIN_GENERIC,
            )
        };
        if result == ERROR_CANCELLED.0 {
            return Err(io::Error::other("the password prompt was cancelled"));
        }
        if result != ERROR_SUCCESS.0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }

        // Unpack: first call sizes the buffers, second fills them.
        let mut user_len = 0u32;
        let mut domain_len = 0u32;
        let mut password_len = 0u32;
        // SAFETY: a sizing call with null buffers; it reports the lengths it needs.
        let _ = unsafe {
            CredUnPackAuthenticationBufferW(
                CRED_PACK_GENERIC_CREDENTIALS,
                out_buffer,
                out_size,
                None,
                &mut user_len,
                None,
                Some(&mut domain_len),
                None,
                &mut password_len,
            )
        };
        let mut user = vec![0u16; user_len.max(1) as usize];
        let mut domain = vec![0u16; domain_len.max(1) as usize];
        let mut password = vec![0u16; password_len.max(1) as usize];
        // SAFETY: the buffers are sized as the first call asked.
        let unpacked = unsafe {
            CredUnPackAuthenticationBufferW(
                CRED_PACK_GENERIC_CREDENTIALS,
                out_buffer,
                out_size,
                Some(PWSTR(user.as_mut_ptr())),
                &mut user_len,
                Some(PWSTR(domain.as_mut_ptr())),
                Some(&mut domain_len),
                Some(PWSTR(password.as_mut_ptr())),
                &mut password_len,
            )
        };

        // Wipe and free the system-allocated credential buffer whatever happened.
        // SAFETY: `out_buffer`/`out_size` come from the prompt above and are freed once.
        unsafe {
            std::slice::from_raw_parts_mut(out_buffer.cast::<u8>(), out_size as usize).fill(0);
            CoTaskMemFree(Some(out_buffer));
        }

        unpacked.map_err(|err| {
            if io::Error::from(err.clone()).raw_os_error()
                == Some(ERROR_INSUFFICIENT_BUFFER.0 as i32)
            {
                io::Error::other("could not read the entered credentials")
            } else {
                io::Error::from(err)
            }
        })?;

        let entered = String::from_utf16_lossy(trim_nul(&user));
        let typed_domain = String::from_utf16_lossy(trim_nul(&domain));
        // Keep the password null-terminated for the Win32 calls that read it.
        password.truncate(password_len as usize + 1);
        if password.last() != Some(&0) {
            password.push(0);
        }

        let (user, domain) = split_account(&entered, &typed_domain);
        Ok(Credentials {
            user,
            domain,
            password,
        })
    }

    fn trim_nul(wide: &[u16]) -> &[u16] {
        let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
        &wide[..end]
    }

    /// Splits what the dialog returned into the user name and domain Winlogon expects:
    /// `DOMAIN\user`, a `user@domain` UPN (kept whole, domain blank), or a bare local name
    /// (domain set to this computer).
    fn split_account(entered: &str, typed_domain: &str) -> (String, String) {
        if let Some((domain, user)) = entered.split_once('\\') {
            (user.to_owned(), domain.to_owned())
        } else if entered.contains('@') {
            (entered.to_owned(), String::new())
        } else if !typed_domain.is_empty() {
            (entered.to_owned(), typed_domain.to_owned())
        } else {
            let computer = std::env::var("COMPUTERNAME").unwrap_or_default();
            (entered.to_owned(), computer)
        }
    }

    /// Confirms the password, so auto-logon is not set up with a wrong one. Only a clear
    /// "wrong password" is fatal; account types `LogonUser` cannot check (e.g. Microsoft or
    /// Entra accounts) are trusted, since the user just typed them into the OS dialog.
    fn validate_credentials(creds: &Credentials) -> io::Result<()> {
        use windows::Win32::Foundation::{CloseHandle, ERROR_LOGON_FAILURE, HANDLE};
        use windows::Win32::Security::{
            LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, LogonUserW,
        };
        use windows::core::{HSTRING, PCWSTR};

        let user = HSTRING::from(&creds.user);
        // A UPN (user@domain) carries its own domain; otherwise pass the parsed domain.
        let domain = (!creds.domain.is_empty() && !creds.user.contains('@'))
            .then(|| HSTRING::from(&creds.domain));
        let domain_ptr = domain
            .as_ref()
            .map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr()));

        let mut token = HANDLE::default();
        // SAFETY: all string pointers outlive the call; a token handed back is closed below.
        let result = unsafe {
            LogonUserW(
                PCWSTR(user.as_ptr()),
                domain_ptr,
                PCWSTR(creds.password.as_ptr()),
                LOGON32_LOGON_INTERACTIVE,
                LOGON32_PROVIDER_DEFAULT,
                &mut token,
            )
        };
        match result {
            Ok(()) => {
                if !token.is_invalid() {
                    // SAFETY: a valid token handle from the successful logon.
                    let _ = unsafe { CloseHandle(token) };
                }
                Ok(())
            }
            Err(err)
                if io::Error::from(err.clone()).raw_os_error()
                    == Some(ERROR_LOGON_FAILURE.0 as i32) =>
            {
                Err(io::Error::other("the password is incorrect"))
            }
            // Could not verify (e.g. a Microsoft account); trust the typed password.
            Err(_) => Ok(()),
        }
    }

    /// Stores the auto-logon password as the LSA secret, or deletes it when `password` is
    /// `None`.
    fn lsa_store_password(password: Option<&[u16]>) -> io::Result<()> {
        use windows::Win32::Foundation::NTSTATUS;
        use windows::Win32::Security::Authentication::Identity::{
            LSA_HANDLE, LSA_OBJECT_ATTRIBUTES, LSA_UNICODE_STRING, LsaClose, LsaNtStatusToWinError,
            LsaOpenPolicy, LsaStorePrivateData, POLICY_CREATE_SECRET,
        };
        use windows::core::PWSTR;

        fn lsa_string(buffer: &mut [u16]) -> LSA_UNICODE_STRING {
            // Length counts the bytes before the trailing null; MaximumLength includes it.
            let chars = buffer.len().saturating_sub(1);
            LSA_UNICODE_STRING {
                Length: (chars * 2) as u16,
                MaximumLength: (buffer.len() * 2) as u16,
                Buffer: PWSTR(buffer.as_mut_ptr()),
            }
        }

        fn check(status: NTSTATUS) -> io::Result<()> {
            if status.0 == 0 {
                Ok(())
            } else {
                // SAFETY: pure translation of a status code.
                let code = unsafe { LsaNtStatusToWinError(status) };
                Err(io::Error::from_raw_os_error(code as i32))
            }
        }

        let attributes = LSA_OBJECT_ATTRIBUTES::default();
        let mut handle = LSA_HANDLE::default();
        // SAFETY: a zeroed attributes struct is valid; the handle is closed below.
        check(unsafe {
            LsaOpenPolicy(None, &attributes, POLICY_CREATE_SECRET as u32, &mut handle)
        })?;

        let mut key: Vec<u16> = LSA_SECRET.encode_utf16().chain([0]).collect();
        let key_str = lsa_string(&mut key);
        let mut data = password.map(<[u16]>::to_vec);
        let data_str = data.as_mut().map(|buffer| lsa_string(buffer));

        // SAFETY: the key (and data, when present) outlive the call; `handle` is open.
        let result = unsafe {
            LsaStorePrivateData(
                handle,
                &key_str,
                data_str.as_ref().map(|s| s as *const LSA_UNICODE_STRING),
            )
        };
        // SAFETY: `handle` came from LsaOpenPolicy and is closed exactly once.
        let _ = unsafe { LsaClose(handle) };
        if let Some(buffer) = data.as_mut() {
            buffer.iter_mut().for_each(|c| *c = 0);
        }
        match check(result) {
            // Deleting a secret that is already gone is success.
            Err(err) if password.is_none() && err.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    // --- Power timeouts (sleep, display off) ---------------------------------------------
    //
    // Each is a timeout on the active power scheme, in seconds, where 0 means "never". A
    // standard user may change their own power timeouts, so these need no elevation. The
    // plugged-in (AC) and on-battery (DC) values are controlled together, and the values in
    // effect before a timeout was turned off are backed up under HKCU so they restore exactly.

    use windows::core::GUID;

    /// Windows defaults used only when turning a timeout on with nothing backed up.
    const DEFAULT_AC_SECS: u32 = 30 * 60;
    const DEFAULT_DC_SECS: u32 = 15 * 60;
    const POWER_BACKUP_KEY: &str = r"SOFTWARE\Gravekeeper\Power";

    /// A power timeout: its setting GUID and the HKCU value names its backup uses.
    struct Timeout {
        setting: GUID,
        ac_backup: &'static str,
        dc_backup: &'static str,
    }

    fn standby() -> Timeout {
        use windows::Win32::System::SystemServices::GUID_STANDBY_TIMEOUT;
        Timeout {
            setting: GUID_STANDBY_TIMEOUT,
            ac_backup: "StandbyAc",
            dc_backup: "StandbyDc",
        }
    }

    fn display() -> Timeout {
        use windows::Win32::System::SystemServices::GUID_VIDEO_POWERDOWN_TIMEOUT;
        Timeout {
            setting: GUID_VIDEO_POWERDOWN_TIMEOUT,
            ac_backup: "VideoAc",
            dc_backup: "VideoDc",
        }
    }

    /// The active power scheme's GUID, copied so the caller owns it.
    fn active_scheme() -> io::Result<GUID> {
        use windows::Win32::Foundation::{ERROR_SUCCESS, HLOCAL, LocalFree};
        use windows::Win32::System::Power::PowerGetActiveScheme;

        let mut active: *mut GUID = std::ptr::null_mut();
        // SAFETY: `active` receives an allocation that we copy out of and then free.
        let err = unsafe { PowerGetActiveScheme(None, &mut active) };
        if err != ERROR_SUCCESS || active.is_null() {
            return Err(io::Error::from_raw_os_error(err.0 as i32));
        }
        // SAFETY: `active` is a valid GUID allocation from the call above.
        let scheme = unsafe { *active };
        // SAFETY: the buffer was allocated by the Power API for us to LocalFree.
        unsafe { LocalFree(Some(HLOCAL(active.cast()))) };
        Ok(scheme)
    }

    fn read_timeout(timeout: &Timeout) -> io::Result<(u32, u32)> {
        use windows::Win32::System::Power::{PowerReadACValueIndex, PowerReadDCValueIndex};
        use windows::Win32::System::SystemServices::GUID_SLEEP_SUBGROUP;
        use windows::Win32::System::SystemServices::GUID_VIDEO_SUBGROUP;

        let scheme = active_scheme()?;
        let subgroup = subgroup_of(&timeout.setting, &GUID_SLEEP_SUBGROUP, &GUID_VIDEO_SUBGROUP);
        let mut ac = 0u32;
        let mut dc = 0u32;
        // SAFETY: all GUID pointers and the scheme outlive the calls.
        unsafe {
            let _ = PowerReadACValueIndex(
                None,
                Some(&scheme),
                Some(&subgroup),
                Some(&timeout.setting),
                &mut ac,
            );
            let _ = PowerReadDCValueIndex(
                None,
                Some(&scheme),
                Some(&subgroup),
                Some(&timeout.setting),
                &mut dc,
            );
        }
        Ok((ac, dc))
    }

    fn write_timeout(timeout: &Timeout, ac: u32, dc: u32) -> io::Result<()> {
        use windows::Win32::Foundation::ERROR_SUCCESS;
        use windows::Win32::System::Power::{
            PowerSetActiveScheme, PowerWriteACValueIndex, PowerWriteDCValueIndex,
        };
        use windows::Win32::System::SystemServices::GUID_SLEEP_SUBGROUP;
        use windows::Win32::System::SystemServices::GUID_VIDEO_SUBGROUP;

        let scheme = active_scheme()?;
        let subgroup = subgroup_of(&timeout.setting, &GUID_SLEEP_SUBGROUP, &GUID_VIDEO_SUBGROUP);
        // SAFETY: the scheme and GUIDs outlive the calls.
        let err = unsafe {
            PowerWriteACValueIndex(None, &scheme, Some(&subgroup), Some(&timeout.setting), ac)
        };
        if err != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(err.0 as i32));
        }
        // SAFETY: as above.
        unsafe {
            let _ =
                PowerWriteDCValueIndex(None, &scheme, Some(&subgroup), Some(&timeout.setting), dc);
        }
        // The writes change the stored scheme; re-selecting it applies them to the live one.
        // SAFETY: `scheme` is the active scheme's own GUID.
        let err = unsafe { PowerSetActiveScheme(None, Some(&scheme)) };
        if err != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(err.0 as i32));
        }
        Ok(())
    }

    fn subgroup_of(setting: &GUID, sleep: &GUID, video: &GUID) -> GUID {
        use windows::Win32::System::SystemServices::GUID_STANDBY_TIMEOUT;
        if *setting == GUID_STANDBY_TIMEOUT {
            *sleep
        } else {
            *video
        }
    }

    fn timeout_on(timeout: &Timeout) -> io::Result<bool> {
        let (ac, dc) = read_timeout(timeout)?;
        // In effect if either power source has a non-zero timeout.
        Ok(ac != 0 || dc != 0)
    }

    fn set_timeout(timeout: &Timeout, enabled: bool) -> io::Result<()> {
        use windows_registry::CURRENT_USER;

        if enabled {
            let key = CURRENT_USER.open(POWER_BACKUP_KEY);
            let read = |name: &str, default: u32| {
                key.as_ref()
                    .ok()
                    .and_then(|k| k.get_u32(name).ok())
                    .unwrap_or(default)
            };
            let ac = read(timeout.ac_backup, DEFAULT_AC_SECS);
            let dc = read(timeout.dc_backup, DEFAULT_DC_SECS);
            // Avoid restoring "never" on both, which would leave the timeout effectively off.
            let (ac, dc) = if ac == 0 && dc == 0 {
                (DEFAULT_AC_SECS, DEFAULT_DC_SECS)
            } else {
                (ac, dc)
            };
            write_timeout(timeout, ac, dc)?;
            if let Ok(key) = CURRENT_USER.options().read().write().open(POWER_BACKUP_KEY) {
                let _ = key.remove_value(timeout.ac_backup);
                let _ = key.remove_value(timeout.dc_backup);
            }
        } else {
            let (ac, dc) = read_timeout(timeout)?;
            // Save the current values (even "never") so turning it back on restores exactly.
            if let Ok(key) = CURRENT_USER.create(POWER_BACKUP_KEY) {
                let _ = key.set_u32(timeout.ac_backup, ac);
                let _ = key.set_u32(timeout.dc_backup, dc);
            }
            write_timeout(timeout, 0, 0)?;
        }
        Ok(())
    }

    pub fn automatic_sleep() -> io::Result<bool> {
        timeout_on(&standby())
    }

    pub fn set_automatic_sleep(enabled: bool) -> io::Result<()> {
        set_timeout(&standby(), enabled)
    }

    pub fn display_off() -> io::Result<bool> {
        timeout_on(&display())
    }

    pub fn set_display_off(enabled: bool) -> io::Result<()> {
        set_timeout(&display(), enabled)
    }

    // --- Hibernation ---------------------------------------------------------------------
    //
    // Enabling or disabling hibernation is system-wide (it manages hiberfil.sys), so it needs
    // administrator rights and runs in the elevated helper via `powercfg /hibernate on|off`,
    // the supported way to toggle it. The state is read from the registry without elevation:
    // `powercfg /hibernate off` sets `HibernateEnabled` to 0, so hibernation counts as off only
    // when that value is present and zero.

    const POWER_CONTROL_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Power";
    const HIBERNATE_ENABLED: &str = "HibernateEnabled";

    pub fn hibernate_enabled() -> io::Result<bool> {
        Ok(read_u32(POWER_CONTROL_KEY, HIBERNATE_ENABLED)? != Some(0))
    }

    pub fn set_hibernate(enabled: bool) -> io::Result<()> {
        use std::os::windows::process::CommandExt;
        use std::process::Command;

        // No console window flashes from the windowed helper process.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let status = Command::new("powercfg.exe")
            .args(["/hibernate", if enabled { "on" } else { "off" }])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("powercfg exited with {status}")))
        }
    }

    // --- Notifications -------------------------------------------------------------------
    //
    // The master "Notifications" switch is per user (HKCU), so this needs no elevation. The
    // Settings app's toggle is `NOC_GLOBAL_SETTING_TOASTS_ENABLED`; `PushNotifications
    // \ToastEnabled` is the toast gate the notification platform also checks, so both are set
    // together. Notifications are on by default, so they count as off only when the master
    // value is explicitly 0.

    const NOTIF_SETTINGS_KEY: &str =
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Notifications\Settings";
    const NOC_GLOBAL: &str = "NOC_GLOBAL_SETTING_TOASTS_ENABLED";
    const PUSH_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\PushNotifications";
    const TOAST_ENABLED: &str = "ToastEnabled";

    /// The two "Additional settings" switches on the notifications page: Windows tips and the
    /// "finish setting up this device" suggestions. Read directly from the registry by the
    /// Settings app, so no service restart is needed for them.
    const CDM_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager";
    const TIPS_VALUE: &str = "SubscribedContent-338389Enabled";
    const UPE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\UserProfileEngagement";
    const SCOOBE_VALUE: &str = "ScoobeSystemSettingEnabled";

    fn read_hkcu_u32(key_path: &str, value: &str) -> io::Result<Option<u32>> {
        use windows_registry::CURRENT_USER;
        match CURRENT_USER
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

    pub fn notifications_enabled() -> io::Result<bool> {
        Ok(read_hkcu_u32(NOTIF_SETTINGS_KEY, NOC_GLOBAL)? != Some(0))
    }

    pub fn set_notifications(enabled: bool) -> io::Result<()> {
        use windows_registry::CURRENT_USER;

        let value = u32::from(enabled);
        CURRENT_USER
            .create(NOTIF_SETTINGS_KEY)
            .and_then(|key| key.set_u32(NOC_GLOBAL, value))
            .map_err(io::Error::from)?;
        // The toast gate as well, so toasts are actually suppressed when off.
        if let Ok(key) = CURRENT_USER.create(PUSH_KEY) {
            let _ = key.set_u32(TOAST_ENABLED, value);
        }
        // The "Additional settings" suggestion switches on the same page, so turning
        // notifications off also silences Windows tips and setup suggestions.
        if let Ok(key) = CURRENT_USER.create(CDM_KEY) {
            let _ = key.set_u32(TIPS_VALUE, value);
        }
        if let Ok(key) = CURRENT_USER.create(UPE_KEY) {
            let _ = key.set_u32(SCOOBE_VALUE, value);
        }
        // The notification platform keeps the live state in WNF and only reads these values
        // at start-up, so the registry write alone changes neither the Settings page nor the
        // actual behavior. Restarting the per-user service makes it re-read them. Best-effort:
        // the registry is the durable part, applied at the next sign-in regardless.
        let _ = restart_notification_service();
        Ok(())
    }

    /// Restarts the current session's `WpnUserService` instance (a per-user service, so no
    /// elevation is needed). Its instance name carries a session suffix, so PowerShell's
    /// wildcard find-and-restart is simpler than enumerating services by hand.
    fn restart_notification_service() -> io::Result<()> {
        use std::os::windows::process::CommandExt;
        use std::process::Command;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const SCRIPT: &str = "Get-Service -Name 'WpnUserService*' -ErrorAction SilentlyContinue | \
             Restart-Service -Force -ErrorAction SilentlyContinue";

        let status = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("powershell exited with {status}")))
        }
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

    pub fn auto_login() -> io::Result<Option<String>> {
        Ok(None)
    }

    pub fn set_auto_login(_enabled: bool) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn automatic_sleep() -> io::Result<bool> {
        Err(unsupported())
    }

    pub fn set_automatic_sleep(_enabled: bool) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn display_off() -> io::Result<bool> {
        Err(unsupported())
    }

    pub fn set_display_off(_enabled: bool) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn hibernate_enabled() -> io::Result<bool> {
        Err(unsupported())
    }

    pub fn set_hibernate(_enabled: bool) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn notifications_enabled() -> io::Result<bool> {
        Err(unsupported())
    }

    pub fn set_notifications(_enabled: bool) -> io::Result<()> {
        Err(unsupported())
    }
}
