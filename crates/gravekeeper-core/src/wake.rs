//! Waking the computer from sleep for schedules that ask for it.
//!
//! On Windows a waitable timer with its resume flag wakes the computer, if the power plan
//! allows wake timers. It belongs to this process, so it needs no rights but only works while
//! Gravekeeper is running. Linux and macOS need root for the same, so they do not have it yet.

use std::io;
use std::time::SystemTime;

/// Whether this platform can wake the computer from sleep.
pub const SUPPORTED: bool = cfg!(windows);

pub(crate) use imp::{WakeTimer, keep_awake};

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::time::SystemTime;

    use windows::Win32::Foundation::{
        ERROR_NOT_SUPPORTED, GetLastError, HANDLE, SetLastError, WIN32_ERROR,
    };
    use windows::Win32::System::Power::{
        ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };
    use windows::Win32::System::Threading::{
        CancelWaitableTimer, CreateWaitableTimerW, SetWaitableTimer,
    };
    use windows::core::PCWSTR;

    /// Seconds from 1601, where Windows counts from, to 1970.
    const UNIX_EPOCH_SECS: u64 = 11_644_473_600;

    /// An absolute due time: 100 ns units since 1601, as a positive number.
    pub(super) fn due_time(at: SystemTime) -> i64 {
        let since_unix = at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = since_unix.as_secs() + UNIX_EPOCH_SECS;
        secs as i64 * 10_000_000 + i64::from(since_unix.subsec_nanos() / 100)
    }

    #[derive(Default)]
    pub struct WakeTimer {
        /// Created when first needed, so most computers never have one.
        handle: Option<OwnedHandle>,
    }

    impl WakeTimer {
        /// Wakes the computer at `at`, replacing any earlier time; `None` cancels.
        pub fn set(&mut self, at: Option<SystemTime>) -> io::Result<()> {
            let Some(at) = at else {
                if let Some(handle) = &self.handle {
                    // SAFETY: the handle is open for as long as `self`.
                    unsafe { CancelWaitableTimer(HANDLE(handle.as_raw_handle())) }?;
                }
                return Ok(());
            };
            let handle = match &mut self.handle {
                Some(handle) => handle,
                empty => {
                    // SAFETY: plain call; the new handle is owned below.
                    let created = unsafe { CreateWaitableTimerW(None, true, PCWSTR::null()) }?;
                    // SAFETY: a new handle, closed only by this `OwnedHandle`.
                    empty.insert(unsafe { OwnedHandle::from_raw_handle(created.0) })
                }
            };
            let due = due_time(at);
            // SAFETY: the handle is open and `due` outlives the call, which only reads it. The
            // last error is the only way to learn that the timer cannot resume the computer.
            unsafe {
                SetLastError(WIN32_ERROR(0));
                SetWaitableTimer(HANDLE(handle.as_raw_handle()), &due, 0, None, None, true)?;
                if GetLastError() == ERROR_NOT_SUPPORTED {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "this computer cannot be woken by a timer",
                    ));
                }
            }
            Ok(())
        }
    }

    /// A computer woken by a timer goes back to sleep after a couple of minutes unless told
    /// otherwise. This restarts its idle timers and turns the display on, once: after that the
    /// power plan's own timeouts apply.
    pub fn keep_awake() {
        // SAFETY: plain call without pointers.
        unsafe {
            SetThreadExecutionState(ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED);
        }
    }

    #[cfg(test)]
    pub(super) fn raw(timer: &WakeTimer) -> Option<HANDLE> {
        timer.handle.as_ref().map(|h| HANDLE(h.as_raw_handle()))
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;
    use std::time::SystemTime;

    #[derive(Default)]
    pub struct WakeTimer;

    impl WakeTimer {
        pub fn set(&mut self, at: Option<SystemTime>) -> io::Result<()> {
            match at {
                None => Ok(()),
                Some(_) => Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "waking the computer is only supported on Windows",
                )),
            }
        }
    }

    pub fn keep_awake() {}
}

/// The event text for a wake time that could not be set.
pub(crate) fn describe(err: &io::Error, at: SystemTime) -> String {
    let at = jiff::Timestamp::try_from(at)
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    format!("cannot set the computer to wake at {at}: {err}")
}

#[cfg(all(test, windows))]
mod tests {
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::System::Threading::WaitForSingleObject;

    use super::*;

    #[test]
    fn due_times_count_from_1601() {
        assert_eq!(
            imp::due_time(SystemTime::UNIX_EPOCH),
            116_444_736_000_000_000
        );
        let later = SystemTime::UNIX_EPOCH + Duration::new(1, 500);
        assert_eq!(imp::due_time(later), 116_444_736_010_000_005);
    }

    #[test]
    fn the_timer_fires_at_its_time_and_can_be_cancelled() {
        let mut timer = WakeTimer::default();
        match timer.set(Some(SystemTime::now() + Duration::from_millis(300))) {
            // Virtual machines often have no wake support; the timer itself still runs.
            Err(err) if err.kind() == io::ErrorKind::Unsupported => {}
            other => other.unwrap(),
        }
        let handle = imp::raw(&timer).expect("created by set");
        let started = Instant::now();
        // SAFETY: the handle stays open during the wait.
        assert_eq!(unsafe { WaitForSingleObject(handle, 5_000) }, WAIT_OBJECT_0);
        assert!(started.elapsed() < Duration::from_secs(5));

        let _ = timer.set(Some(SystemTime::now() + Duration::from_secs(3600)));
        timer.set(None).unwrap();
        // SAFETY: as above.
        assert_ne!(unsafe { WaitForSingleObject(handle, 100) }, WAIT_OBJECT_0);
    }
}
