//! Running privileged tasks in a short-lived elevated copy of Gravekeeper.
//!
//! Gravekeeper runs with the user's normal rights. Machine-wide OS settings need
//! administrator rights, so [`run`] starts this executable again as
//! `gravekeeper elevated <request>` through the OS's own consent prompt: UAC on Windows,
//! polkit's `pkexec` on Linux, and the administrator dialog of `osascript` on macOS. That
//! copy applies the tasks, reports back and exits.
//!
//! - A request is a list of [`Task`]s, a closed set of operations. It never names a command,
//!   file or registry path, so the helper cannot be used to run arbitrary code as an
//!   administrator.
//! - The request travels on the command line (hex-encoded JSON), which cannot be changed once
//!   the process has started. Other processes can read command lines, so a request must never
//!   contain secrets.
//! - The report comes back on the helper's stdout (Linux, macOS). A process started through
//!   UAC cannot inherit handles, so on Windows it comes back over a named pipe instead, and
//!   only the helper process is accepted as the pipe's client.
//! - Per-user settings must not be changed here: when a standard user approves with another
//!   (administrator) account, the helper runs as that account, with its own HKCU and home.
//! - When Gravekeeper already runs with administrator rights, tasks are applied in-process.

use std::fmt;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::mpsc;

use serde::{Deserialize, Serialize};

use crate::os_settings;

/// One privileged operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "task", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Task {
    /// Turns automatic OS updates on (the OS default) or off.
    AutomaticUpdates { enabled: bool },
    /// Turns automatic logon on or off. No password travels in the request: when enabling,
    /// the elevated helper collects it through the OS credential prompt (see `os_settings`).
    AutoLogin { enabled: bool },
    /// Enables or disables hibernation (system-wide; manages hiberfil.sys).
    Hibernate { enabled: bool },
    /// Turns Windows Security (Defender) notifications on or off (machine-wide).
    SecurityNotifications { enabled: bool },
}

impl Task {
    fn apply(&self) -> io::Result<()> {
        match *self {
            Task::AutomaticUpdates { enabled } => os_settings::set_automatic_updates(enabled),
            Task::AutoLogin { enabled } => os_settings::set_auto_login(enabled),
            Task::Hibernate { enabled } => os_settings::set_hibernate(enabled),
            Task::SecurityNotifications { enabled } => {
                os_settings::set_security_notifications(enabled)
            }
        }
    }
}

impl fmt::Display for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (what, enabled) = match self {
            Task::AutomaticUpdates { enabled } => ("automatic updates", enabled),
            Task::AutoLogin { enabled } => ("automatic logon", enabled),
            Task::Hibernate { enabled } => ("hibernation", enabled),
            Task::SecurityNotifications { enabled } => ("Windows Security notifications", enabled),
        };
        write!(f, "turning {what} {}", if *enabled { "on" } else { "off" })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The user declined the consent prompt.
    Cancelled,
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Cancelled => f.write_str("cancelled"),
            Error::Failed(message) => f.write_str(message),
        }
    }
}

fn failed(message: impl Into<String>) -> Error {
    Error::Failed(message.into())
}

/// Applies `tasks` with administrator rights, asking the user for consent unless this
/// process already has them. Blocks until the helper exits, however long the prompt stays
/// open; call it off the GUI thread (see [`spawn`]).
pub fn run(tasks: &[Task]) -> Result<(), Error> {
    if imp::is_elevated() {
        tracing::info!(?tasks, "applying with administrator rights");
        return apply(tasks).map_err(Error::Failed);
    }
    tracing::info!(?tasks, "asking for administrator rights");
    let output = imp::launch(&encode(tasks))?;
    parse_report(&output)
}

/// Runs [`run`] on its own thread. The result arrives on the returned channel, after which
/// `done` is called (e.g. to repaint the window).
pub fn spawn(
    tasks: Vec<Task>,
    done: impl FnOnce() + Send + 'static,
) -> mpsc::Receiver<Result<(), Error>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("gravekeeper-elevate".into())
        .spawn(move || {
            let result = run(&tasks);
            match &result {
                Ok(()) => tracing::info!(?tasks, "applied"),
                Err(Error::Cancelled) => tracing::info!("administrator rights were not granted"),
                Err(err) => tracing::warn!(%err, ?tasks, "could not apply"),
            }
            let _ = tx.send(result);
            done();
        })
        .expect("spawn the elevation thread");
    rx
}

/// The elevated side: `gravekeeper elevated <request> [--report <pipe>]`.
///
/// It does not log: it may run as another account, and must not create files owned by that
/// account in the user's data directory. The exit status only says whether a report was
/// delivered, because `osascript` discards the output of a failing command.
pub fn helper(request: &str, report_to: Option<&str>) -> ExitCode {
    let report = match decode(request) {
        Ok(tasks) => apply(&tasks),
        Err(err) => Err(format!("invalid request: {err}")),
    };
    let json = serde_json::to_string(&report).expect("a report serializes");
    match deliver(report_to, &json) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("cannot deliver the report: {err}");
            ExitCode::FAILURE
        }
    }
}

fn deliver(report_to: Option<&str>, json: &str) -> io::Result<()> {
    match report_to {
        #[cfg(windows)]
        Some(pipe) => imp::send_report(pipe, json),
        #[cfg(not(windows))]
        Some(_) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "--report is only used on Windows",
        )),
        None => {
            let mut out = io::stdout().lock();
            writeln!(out, "{json}")?;
            out.flush()
        }
    }
}

/// Runs the tasks in order, stopping at the first failure.
fn apply(tasks: &[Task]) -> Result<(), String> {
    for task in tasks {
        task.apply().map_err(|err| format!("{task}: {err}"))?;
    }
    Ok(())
}

/// Hex keeps the request free of characters that would need quoting on any command line.
fn encode(tasks: &[Task]) -> String {
    let json = serde_json::to_vec(tasks).expect("tasks serialize");
    json.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode(request: &str) -> Result<Vec<Task>, String> {
    if !request.len().is_multiple_of(2) || !request.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("not hex".into());
    }
    let bytes: Vec<u8> = (0..request.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&request[i..i + 2], 16).expect("checked above"))
        .collect();
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// The helper's report is its last line of output. `osascript` turns line feeds into
/// carriage returns.
fn parse_report(output: &str) -> Result<(), Error> {
    let line = output
        .split(['\r', '\n'])
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or_default();
    match serde_json::from_str::<Result<(), String>>(line) {
        Ok(report) => report.map_err(Error::Failed),
        Err(_) => Err(failed(format!(
            "unexpected answer from the elevated helper: {line:?}"
        ))),
    }
}

/// UAC through `ShellExecuteExW("runas")`, with the report on a named pipe.
#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::fs::OpenOptions;
    use std::io::{self, Write};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::Path;

    use windows::Win32::Foundation::{
        ERROR_BROKEN_PIPE, ERROR_CANCELLED, ERROR_IO_PENDING, HANDLE, WAIT_OBJECT_0, WIN32_ERROR,
    };
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_INBOUND, ReadFile,
        SECURITY_IDENTIFICATION,
    };
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::System::IO::{CancelIo, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, GetExitCodeProcess, GetProcessId, INFINITE,
        OpenProcessToken, WaitForMultipleObjects, WaitForSingleObject,
    };
    use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows::core::{HSTRING, PCWSTR, w};

    use super::{Error, failed};

    const PIPE_PREFIX: &str = r"\\.\pipe\gravekeeper-elevated-";
    const MAX_REPORT: usize = 64 * 1024;

    fn is(err: &windows::core::Error, code: WIN32_ERROR) -> bool {
        err.code() == code.to_hresult()
    }

    fn raw(handle: &OwnedHandle) -> HANDLE {
        HANDLE(handle.as_raw_handle())
    }

    pub fn is_elevated() -> bool {
        let mut token = HANDLE::default();
        // SAFETY: the current-process pseudo handle needs no closing; on success `token` is a
        // new handle, owned below.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
            return false;
        }
        // SAFETY: `token` was just opened and is closed only by this `OwnedHandle`.
        let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0;
        // SAFETY: the buffer is a TOKEN_ELEVATION of the size passed.
        let queried = unsafe {
            GetTokenInformation(
                raw(&token),
                TokenElevation,
                Some((&raw mut elevation).cast::<c_void>()),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            )
        };
        queried.is_ok() && elevation.TokenIsElevated != 0
    }

    pub fn launch(request: &str) -> Result<String, Error> {
        let exe = std::env::current_exe()
            .map_err(|e| failed(format!("cannot locate this program: {e}")))?;
        let name = format!("{PIPE_PREFIX}{}-{:016x}", std::process::id(), nonce());
        let mut pipe = ReportPipe::create(&name)
            .map_err(|e| failed(format!("cannot create the report pipe: {e}")))?;
        pipe.listen()
            .map_err(|e| failed(format!("cannot listen on the report pipe: {e}")))?;
        let process = shell_execute(&exe, &format!("elevated {request} --report {name}"))?;
        receive(&mut pipe, &process)
    }

    /// Not secret, only unpredictable enough that two pipes never share a name;
    /// `FILE_FLAG_FIRST_PIPE_INSTANCE` makes creating a taken name fail.
    fn nonce() -> u64 {
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        hasher.finish()
    }

    fn shell_execute(exe: &Path, parameters: &str) -> Result<OwnedHandle, Error> {
        // ShellExecuteEx may hand the request to shell extensions, which need COM.
        // SAFETY: balanced by CoUninitialize below when it succeeds.
        let com =
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        let file = HSTRING::from(exe.as_os_str());
        let parameters = HSTRING::from(parameters);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(parameters.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        // SAFETY: `info` and the strings it points to outlive the call.
        let result = unsafe { ShellExecuteExW(&mut info) };
        if com.is_ok() {
            // SAFETY: pairs with the successful CoInitializeEx above, on the same thread.
            unsafe { CoUninitialize() };
        }
        match result {
            Ok(()) if !info.hProcess.is_invalid() => {
                // SAFETY: SEE_MASK_NOCLOSEPROCESS hands us this process handle to close.
                Ok(unsafe { OwnedHandle::from_raw_handle(info.hProcess.0) })
            }
            Ok(()) => Err(failed("the elevated helper did not start")),
            Err(err) if is(&err, ERROR_CANCELLED) => Err(Error::Cancelled),
            Err(err) => Err(failed(format!(
                "cannot ask for administrator rights: {err}"
            ))),
        }
    }

    /// Waits for `process` to connect to `pipe` and send its report, or to exit without one.
    /// A helper that reports connects before it exits, and the wait returns the first
    /// signaled handle in order, so a report is never missed.
    pub(super) fn receive(pipe: &mut ReportPipe, process: &OwnedHandle) -> Result<String, Error> {
        let handles = [raw(&pipe.event), raw(process)];
        // SAFETY: both handles stay open during the wait.
        let woke = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        if woke != WAIT_OBJECT_0 {
            let mut code = 0;
            // SAFETY: plain wait and query on an open process handle.
            unsafe {
                WaitForSingleObject(raw(process), INFINITE);
                let _ = GetExitCodeProcess(raw(process), &mut code);
            }
            return Err(failed(format!(
                "the elevated helper exited with code {code} without reporting"
            )));
        }
        pipe.finish_pending()
            .map_err(|e| failed(format!("report pipe: {e}")))?;
        let mut client = 0;
        // SAFETY: plain queries on open handles.
        let client_ok = unsafe { GetNamedPipeClientProcessId(raw(&pipe.handle), &mut client) };
        if client_ok.is_err() || client != unsafe { GetProcessId(raw(process)) } {
            return Err(failed("another process answered on the report pipe"));
        }
        let bytes = pipe
            .read_to_end()
            .map_err(|e| failed(format!("report pipe: {e}")))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The server end of the report pipe, with at most one overlapped operation at a time.
    pub(super) struct ReportPipe {
        handle: OwnedHandle,
        /// Signaled when the pending operation completes.
        event: OwnedHandle,
        /// Boxed so it does not move: the kernel writes to it until the pending operation
        /// has completed (see `Drop`).
        overlapped: Box<OVERLAPPED>,
        pending: bool,
    }

    impl ReportPipe {
        /// Inbound only, one instance, local clients only. The default security descriptor
        /// gives write access to the creator and administrators, not to other users.
        pub(super) fn create(name: &str) -> io::Result<Self> {
            // SAFETY: plain call with a valid name; the handle is checked and owned below.
            let handle = unsafe {
                CreateNamedPipeW(
                    &HSTRING::from(name),
                    PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    0,
                    MAX_REPORT as u32,
                    0,
                    None,
                )
            };
            if handle.is_invalid() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: a new handle, closed only by this `OwnedHandle`.
            let handle = unsafe { OwnedHandle::from_raw_handle(handle.0) };
            // SAFETY: plain call; the new handle is owned below.
            let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }?;
            // SAFETY: as above.
            let event = unsafe { OwnedHandle::from_raw_handle(event.0) };
            let overlapped = Box::new(OVERLAPPED {
                hEvent: raw(&event),
                ..Default::default()
            });
            Ok(Self {
                handle,
                event,
                overlapped,
                pending: false,
            })
        }

        /// Starts waiting for the client; the event is signaled when it connects.
        pub(super) fn listen(&mut self) -> io::Result<()> {
            // SAFETY: the OVERLAPPED is boxed and outlives the operation (see `Drop`).
            match unsafe { ConnectNamedPipe(raw(&self.handle), Some(&mut *self.overlapped)) } {
                Err(err) if is(&err, ERROR_IO_PENDING) => {
                    self.pending = true;
                    Ok(())
                }
                // Completed at once, or a client was already connected: neither should happen
                // before the helper starts, so another process got there first.
                Ok(()) | Err(_) => Err(io::Error::other("another process opened the pipe")),
            }
        }

        /// Collects the result of the operation whose event was signaled.
        fn finish_pending(&mut self) -> io::Result<u32> {
            let mut transferred = 0;
            // SAFETY: the OVERLAPPED belongs to the pending operation on this handle.
            let result = unsafe {
                GetOverlappedResult(raw(&self.handle), &*self.overlapped, &mut transferred, true)
            };
            self.pending = false;
            result?;
            Ok(transferred)
        }

        /// Reads until the client closes its end.
        fn read_to_end(&mut self) -> io::Result<Vec<u8>> {
            let mut out = Vec::new();
            let mut buffer = vec![0u8; 4096];
            loop {
                // SAFETY: the buffer and OVERLAPPED outlive the operation: it is always
                // finished below (or cancelled in `Drop`) before either is dropped.
                let started = unsafe {
                    ReadFile(
                        raw(&self.handle),
                        Some(&mut buffer),
                        None,
                        Some(&mut *self.overlapped),
                    )
                };
                match started {
                    Ok(()) => {}
                    Err(err) if is(&err, ERROR_IO_PENDING) => {}
                    Err(err) if is(&err, ERROR_BROKEN_PIPE) => return Ok(out),
                    Err(err) => return Err(err.into()),
                }
                self.pending = true;
                match self.finish_pending() {
                    Ok(n) => out.extend_from_slice(&buffer[..n as usize]),
                    Err(err) if err.raw_os_error() == Some(ERROR_BROKEN_PIPE.0 as i32) => {
                        return Ok(out);
                    }
                    Err(err) => return Err(err),
                }
                if out.len() > MAX_REPORT {
                    return Err(io::Error::other("report too long"));
                }
            }
        }
    }

    impl Drop for ReportPipe {
        fn drop(&mut self) {
            if self.pending {
                // The kernel may still write to the OVERLAPPED: cancel and wait for the
                // cancellation to complete before it is freed.
                let mut transferred = 0;
                // SAFETY: cancels and waits for this handle's own pending operation.
                unsafe {
                    let _ = CancelIo(raw(&self.handle));
                    let _ = GetOverlappedResult(
                        raw(&self.handle),
                        &*self.overlapped,
                        &mut transferred,
                        true,
                    );
                }
            }
        }
    }

    /// The helper's side: writes the report to the pipe Gravekeeper created.
    pub fn send_report(pipe: &str, report: &str) -> io::Result<()> {
        // Only our own pipes: this process runs as an administrator and must not be pointed
        // at arbitrary paths.
        let valid = pipe.strip_prefix(PIPE_PREFIX).is_some_and(|rest| {
            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
        if !valid {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a report pipe",
            ));
        }
        let mut file = OpenOptions::new()
            .write(true)
            // Without this the pipe's server could impersonate this elevated process.
            .security_qos_flags(SECURITY_IDENTIFICATION.0)
            .open(pipe)?;
        file.write_all(report.as_bytes())
    }

    #[cfg(test)]
    mod tests {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

        use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
        use windows::Win32::System::Threading::GetCurrentProcess;

        use super::*;

        fn pipe_name(test: &str) -> String {
            format!(
                "{PIPE_PREFIX}test-{test}-{}-{:016x}",
                std::process::id(),
                nonce()
            )
        }

        fn current_process() -> OwnedHandle {
            let mut handle = HANDLE::default();
            // SAFETY: duplicates the pseudo handle into a real one, owned below.
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    GetCurrentProcess(),
                    GetCurrentProcess(),
                    &mut handle,
                    0,
                    false,
                    DUPLICATE_SAME_ACCESS,
                )
                .unwrap();
                OwnedHandle::from_raw_handle(handle.0)
            }
        }

        #[test]
        fn report_arrives_over_the_pipe() {
            let name = pipe_name("report");
            let mut pipe = ReportPipe::create(&name).unwrap();
            pipe.listen().unwrap();
            // The report fits in the pipe's buffer, so it can be sent before anyone reads.
            send_report(&name, r#"{"Ok":null}"#).unwrap();
            // This process is the client, so its handle passes the client check.
            let report = receive(&mut pipe, &current_process()).unwrap();
            assert_eq!(report, r#"{"Ok":null}"#);
        }

        #[test]
        fn a_helper_that_exits_without_reporting_is_an_error() {
            let name = pipe_name("exit");
            let mut pipe = ReportPipe::create(&name).unwrap();
            pipe.listen().unwrap();
            let child = std::process::Command::new("cmd")
                .args(["/C", "exit 3"])
                .spawn()
                .unwrap();
            // SAFETY: duplicating keeps the handle valid independently of `child`.
            let process = unsafe {
                let mut handle = HANDLE::default();
                DuplicateHandle(
                    GetCurrentProcess(),
                    HANDLE(child.as_raw_handle()),
                    GetCurrentProcess(),
                    &mut handle,
                    0,
                    false,
                    DUPLICATE_SAME_ACCESS,
                )
                .unwrap();
                OwnedHandle::from_raw_handle(handle.0)
            };
            let err = receive(&mut pipe, &process).unwrap_err();
            assert!(err.to_string().contains("code 3"), "{err}");
        }

        #[test]
        fn a_second_server_cannot_take_the_name() {
            let name = pipe_name("squat");
            let _first = ReportPipe::create(&name).unwrap();
            assert!(ReportPipe::create(&name).is_err());
        }

        #[test]
        fn the_helper_only_writes_to_report_pipes() {
            for target in [r"C:\Windows\Temp\x", r"\\.\pipe\other", PIPE_PREFIX] {
                let err = send_report(target, "{}").unwrap_err();
                assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{target}");
            }
        }
    }
}

/// `pkexec` from polkit, with the report on stdout.
#[cfg(target_os = "linux")]
mod imp {
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    use super::{Error, failed};

    pub fn is_elevated() -> bool {
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }

    /// Inside an AppImage this executable is on a FUSE mount that root cannot read, so the
    /// AppImage file itself is run instead.
    fn executable() -> Result<PathBuf, Error> {
        let exe = std::env::current_exe()
            .map_err(|e| failed(format!("cannot locate this program: {e}")))?;
        if let (Some(image), Some(dir)) = (std::env::var_os("APPIMAGE"), std::env::var_os("APPDIR"))
            && exe.starts_with(dir)
        {
            return Ok(PathBuf::from(image));
        }
        Ok(exe)
    }

    pub fn launch(request: &str) -> Result<String, Error> {
        let output = Command::new("pkexec")
            .arg(executable()?)
            .args(["elevated", request])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    failed("pkexec (polkit) is needed to ask for administrator rights")
                }
                _ => failed(format!("cannot run pkexec: {e}")),
            })?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        match output.status.code() {
            Some(0) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
            // pkexec: 126 = the dialog was dismissed, 127 = not authorized or an error.
            Some(126) => Err(Error::Cancelled),
            _ => Err(failed(format!(
                "pkexec failed ({}): {}",
                output.status,
                stderr.trim()
            ))),
        }
    }
}

/// The administrator dialog of AppleScript's `do shell script`, with the report as its result.
#[cfg(target_os = "macos")]
mod imp {
    use std::process::{Command, Stdio};

    use super::{Error, failed};

    pub fn is_elevated() -> bool {
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }

    pub fn launch(request: &str) -> Result<String, Error> {
        let exe = std::env::current_exe()
            .map_err(|e| failed(format!("cannot locate this program: {e}")))?;
        let output = Command::new("osascript")
            .args(["-e", &script(&exe.to_string_lossy(), request)])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| failed(format!("cannot run osascript: {e}")))?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else if stderr.contains("(-128)") {
            // "User canceled."
            Err(Error::Cancelled)
        } else {
            Err(failed(format!("osascript failed: {}", stderr.trim())))
        }
    }

    pub(super) fn script(exe: &str, request: &str) -> String {
        let exe = exe.replace('\\', "\\\\").replace('"', "\\\"");
        format!(
            "do shell script (quoted form of \"{exe}\") & \" elevated {request}\" \
             with prompt \"Gravekeeper wants to change system settings.\" \
             with administrator privileges"
        )
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod imp {
    use super::{Error, failed};

    pub fn is_elevated() -> bool {
        false
    }

    pub fn launch(_request: &str) -> Result<String, Error> {
        Err(failed("not supported on this platform"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_as_hex() {
        let tasks = vec![
            Task::AutomaticUpdates { enabled: false },
            Task::AutomaticUpdates { enabled: true },
            Task::AutoLogin { enabled: true },
            Task::Hibernate { enabled: false },
            Task::SecurityNotifications { enabled: false },
        ];
        let request = encode(&tasks);
        assert!(request.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(decode(&request), Ok(tasks));
    }

    #[test]
    fn tasks_have_a_stable_format() {
        let json = serde_json::to_string(&Task::AutomaticUpdates { enabled: false }).unwrap();
        assert_eq!(json, r#"{"task":"automatic-updates","enabled":false}"#);
        let json = serde_json::to_string(&Task::AutoLogin { enabled: true }).unwrap();
        assert_eq!(json, r#"{"task":"auto-login","enabled":true}"#);
    }

    #[test]
    fn malformed_requests_are_rejected() {
        assert!(decode("abc").is_err());
        assert!(decode("+1").is_err());
        assert!(decode("zz").is_err());
        // Valid hex, unknown task.
        let unknown: String =
            br#"[{"task":"run-command"}]"#.iter().map(|b| format!("{b:02x}")).collect();
        assert!(decode(&unknown).is_err());
    }

    #[test]
    fn reports_are_read_from_the_last_line() {
        assert_eq!(parse_report("noise\n{\"Ok\":null}\n"), Ok(()));
        assert_eq!(
            parse_report("{\"Err\":\"denied\"}\r"),
            Err(Error::Failed("denied".into()))
        );
        assert!(matches!(parse_report(""), Err(Error::Failed(_))));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_applescript_quotes_the_path() {
        let script = imp::script(r#"/Apps/A "b"\c/gk"#, "00");
        assert!(script.starts_with(r#"do shell script (quoted form of "/Apps/A \"b\"\\c/gk")"#));
        assert!(script.ends_with("with administrator privileges"));
    }
}
