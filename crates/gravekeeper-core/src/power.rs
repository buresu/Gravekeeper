//! Restarting, shutting down and sleeping the computer for schedules that ask for it.
//!
//! On Windows any signed-in user may do this unless a policy says otherwise, so it needs no
//! administrator rights. For a restart or shutdown the OS counts down on its own: it goes
//! ahead even if Gravekeeper exits meanwhile.

use std::io;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Whether this platform supports these operations.
pub const SUPPORTED: bool = cfg!(windows);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Restart,
    ShutDown,
    Sleep,
}

impl Operation {
    /// What is done to the computer, e.g. in "cannot restart the computer".
    pub fn describe(self) -> &'static str {
        match self {
            Self::Restart => "restart the computer",
            Self::ShutDown => "shut down the computer",
            Self::Sleep => "put the computer to sleep",
        }
    }

    /// As in "the computer did not restart".
    pub fn verb(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::ShutDown => "shut down",
            Self::Sleep => "sleep",
        }
    }

    /// As in "the computer restarts".
    pub fn verb_s(self) -> &'static str {
        match self {
            Self::Restart => "restarts",
            Self::ShutDown => "shuts down",
            Self::Sleep => "sleeps",
        }
    }
}

/// How this computer sleeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepSupport {
    Sleeps,
    /// Modern Standby (S0): it has no sleep to ask for, but turning the display off enters it.
    ModernStandby,
    Unsupported,
}

pub fn sleep_support() -> SleepSupport {
    imp::sleep_support()
}

/// What the engine needs from the OS. A trait so that tests never restart the computer.
pub trait Power: Send + Sync {
    /// Asks the OS to shut down or restart the computer after `grace`, closing every program
    /// whether or not it agrees. Being accepted is not a guarantee: the OS, a service or a user
    /// may still abort it.
    fn shut_down(&self, restart: bool, grace: Duration, message: &str) -> io::Result<()>;

    /// Stops a restart or shutdown during its grace period.
    fn cancel(&self) -> io::Result<()>;

    /// Puts the computer to sleep at once. May block until it wakes up again.
    fn sleep(&self) -> io::Result<()>;

    /// How long the computer has been up.
    fn uptime(&self) -> Option<Duration>;

    /// How long users are warned before the computer restarts or shuts down.
    fn grace(&self) -> Duration {
        Duration::from_secs(60)
    }

    /// A computer still up this long after it should have shut down did not shut down.
    fn settle(&self) -> Duration {
        Duration::from_secs(180)
    }
}

/// The computer Gravekeeper runs on.
pub struct System;

impl Power for System {
    fn shut_down(&self, restart: bool, grace: Duration, message: &str) -> io::Result<()> {
        imp::shut_down(restart, grace, message)
    }

    fn cancel(&self) -> io::Result<()> {
        imp::cancel()
    }

    fn sleep(&self) -> io::Result<()> {
        imp::sleep()
    }

    fn uptime(&self) -> Option<Duration> {
        sysinfo::System::uptime().ok().map(Duration::from_secs)
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::time::Duration;

    use windows::Win32::Foundation::{
        ERROR_NO_SHUTDOWN_IN_PROGRESS, ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, LPARAM, LUID,
        WPARAM,
    };
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
        SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Power::{
        GetPwrCapabilities, SYSTEM_POWER_CAPABILITIES, SetSuspendState,
    };
    use windows::Win32::System::Shutdown::{
        AbortSystemShutdownW, InitiateShutdownW, SHTDN_REASON_FLAG_PLANNED,
        SHTDN_REASON_MAJOR_APPLICATION, SHTDN_REASON_MINOR_MAINTENANCE, SHUTDOWN_FORCE_OTHERS,
        SHUTDOWN_FORCE_SELF, SHUTDOWN_POWEROFF, SHUTDOWN_REASON, SHUTDOWN_RESTART,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SC_MONITORPOWER, SendMessageW, WM_SYSCOMMAND,
        WS_EX_TOOLWINDOW, WS_POPUP,
    };
    use windows::core::{HSTRING, PCWSTR, w};

    use super::SleepSupport;

    pub fn shut_down(restart: bool, grace: Duration, message: &str) -> io::Result<()> {
        let message = HSTRING::from(message);
        let reason = SHUTDOWN_REASON(
            SHTDN_REASON_MAJOR_APPLICATION.0
                | SHTDN_REASON_MINOR_MAINTENANCE.0
                | SHTDN_REASON_FLAG_PLANNED.0,
        );
        let end = if restart {
            SHUTDOWN_RESTART
        } else {
            SHUTDOWN_POWEROFF
        };
        enable_shutdown_privilege()?;
        // Without the force flags an unsaved document or another signed-in user would stop it,
        // and a grace period is refused.
        // SAFETY: the message outlives the call.
        let status = unsafe {
            InitiateShutdownW(
                PCWSTR::null(),
                &message,
                grace.as_secs() as u32,
                end | SHUTDOWN_FORCE_OTHERS | SHUTDOWN_FORCE_SELF,
                reason,
            )
        };
        match status {
            0 => Ok(()),
            code => Err(io::Error::from_raw_os_error(code as i32)),
        }
    }

    pub fn cancel() -> io::Result<()> {
        enable_shutdown_privilege()?;
        // SAFETY: plain call; null is this computer.
        match unsafe { AbortSystemShutdownW(PCWSTR::null()) } {
            // Cancelled elsewhere (`shutdown /a`).
            Err(err) if err.code() == ERROR_NO_SHUTDOWN_IN_PROGRESS.to_hresult() => Ok(()),
            result => Ok(result?),
        }
    }

    pub fn sleep_support() -> SleepSupport {
        let mut can = SYSTEM_POWER_CAPABILITIES::default();
        // SAFETY: `can` is a valid out pointer.
        if !unsafe { GetPwrCapabilities(&mut can) } {
            SleepSupport::Unsupported
        } else if can.AoAc {
            SleepSupport::ModernStandby
        } else if can.SystemS1 || can.SystemS2 || can.SystemS3 {
            SleepSupport::Sleeps
        } else {
            SleepSupport::Unsupported
        }
    }

    pub fn sleep() -> io::Result<()> {
        match sleep_support() {
            SleepSupport::Sleeps => {
                enable_shutdown_privilege()?;
                // Wake events stay enabled, for schedules that wake the computer.
                // SAFETY: plain call.
                if unsafe { SetSuspendState(false, false, false) } {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            }
            // SetSuspendState fails there, or hibernates.
            SleepSupport::ModernStandby => turn_off_display(),
            SleepSupport::Unsupported => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "this computer does not support sleep",
            )),
        }
    }

    /// Sent to a window of our own: broadcast, every program would turn the display off, some
    /// of them again after the computer has woken up.
    fn turn_off_display() -> io::Result<()> {
        const OFF: isize = 2;
        // SAFETY: plain calls on a window that this thread owns, of a class the system provides.
        unsafe {
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )?;
            SendMessageW(
                window,
                WM_SYSCOMMAND,
                Some(WPARAM(SC_MONITORPOWER as usize)),
                Some(LPARAM(OFF)),
            );
            DestroyWindow(window)?;
        }
        Ok(())
    }

    pub(super) fn enable_shutdown_privilege() -> io::Result<()> {
        let mut token = HANDLE::default();
        // SAFETY: `token` is a valid out pointer; the new handle is owned below.
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            )
        }?;
        // SAFETY: a new handle, closed only by this `OwnedHandle`.
        let token = unsafe { OwnedHandle::from_raw_handle(token.0) };

        let mut luid = LUID::default();
        // SAFETY: `luid` is a valid out pointer.
        unsafe { LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid) }?;
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        // SAFETY: the handle is open and `privileges` outlives the call. It succeeds without
        // changing anything when the token lacks the privilege; only the last error tells.
        unsafe {
            AdjustTokenPrivileges(
                HANDLE(token.as_raw_handle()),
                false,
                Some(&privileges),
                0,
                None,
                None,
            )?;
            if GetLastError() == ERROR_NOT_ALL_ASSIGNED {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "this account may not shut down the computer",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;
    use std::time::Duration;

    use super::SleepSupport;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "this is only supported on Windows",
        )
    }

    pub fn shut_down(_restart: bool, _grace: Duration, _message: &str) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn sleep_support() -> SleepSupport {
        SleepSupport::Unsupported
    }

    pub fn sleep() -> io::Result<()> {
        Err(unsupported())
    }

    pub fn cancel() -> io::Result<()> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_is_known() {
        assert!(System.uptime().is_some_and(|up| !up.is_zero()));
    }

    /// Only the privilege: this must never actually shut down or sleep.
    #[cfg(windows)]
    #[test]
    fn the_shutdown_privilege_can_be_enabled() {
        imp::enable_shutdown_privilege().unwrap();
    }
}
