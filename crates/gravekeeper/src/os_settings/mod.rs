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

/// Whether automatic sleep, turning off the display and hibernation are machine-wide settings
/// that need administrator rights. On macOS they are (`pmset`), so the sleep and display-off
/// timeouts go through `elevate` there; elsewhere those two are per-user and run in-process.
pub const POWER_NEEDS_ADMIN: bool = cfg!(target_os = "macos");

/// Whether the computer sleeps on its own after a period of inactivity.
pub fn automatic_sleep() -> io::Result<bool> {
    imp::automatic_sleep()
}

/// Turns automatic sleep on or off. On Windows and Linux this needs no administrator rights —
/// a standard user may change their own power timeouts — so it runs in-process; on macOS it
/// does (see [`POWER_NEEDS_ADMIN`]).
pub fn set_automatic_sleep(enabled: bool) -> io::Result<()> {
    imp::set_automatic_sleep(enabled)
}

/// Whether the display turns off on its own after a period of inactivity.
pub fn display_off() -> io::Result<bool> {
    imp::display_off()
}

/// Turns the display-off timeout on or off. Like [`set_automatic_sleep`], in-process except on
/// macOS.
pub fn set_display_off(enabled: bool) -> io::Result<()> {
    imp::set_display_off(enabled)
}

/// Whether hard disks turn off on their own after a period of inactivity. Windows and macOS.
pub fn disk_off() -> io::Result<bool> {
    imp::disk_off()
}

/// Turns the disk-off timeout on or off. Like [`set_automatic_sleep`], in-process except on
/// macOS.
pub fn set_disk_off(enabled: bool) -> io::Result<()> {
    imp::set_disk_off(enabled)
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

/// Whether Windows Security (Defender) shows its notifications.
pub fn security_notifications_enabled() -> io::Result<bool> {
    imp::security_notifications_enabled()
}

/// Turns the machine-wide part of Windows Security notifications on or off. Needs
/// administrator rights (goes through `elevate`). The per-user account-protection part is
/// [`set_security_notifications_user`], applied in-process.
pub(crate) fn set_security_notifications(enabled: bool) -> io::Result<()> {
    imp::set_security_notifications(enabled)
}

/// The per-user account-protection part of the Windows Security notifications setting.
/// Needs no administrator rights and must run in the signed-in user's process.
pub fn set_security_notifications_user(enabled: bool) -> io::Result<()> {
    imp::set_security_notifications_user(enabled)
}

/// Whether the firewall is on for any profile.
pub fn firewall_enabled() -> io::Result<bool> {
    imp::firewall_enabled()
}

/// Turns the firewall on or off for all profiles. Machine-wide, so needs administrator
/// rights (goes through `elevate`).
pub(crate) fn set_firewall(enabled: bool) -> io::Result<()> {
    imp::set_firewall(enabled)
}

/// Declares settings that exist only on macOS; elsewhere they report `Unsupported`.
macro_rules! macos_only {
    ($($(#[$attr:meta])* $vis:vis fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {$(
        $(#[$attr])*
        $vis fn $name($($arg: $ty),*) -> io::Result<$ret> {
            #[cfg(target_os = "macos")]
            return imp::$name($($arg),*);
            #[cfg(not(target_os = "macos"))]
            {
                $(let _: $ty = $arg;)*
                Err(io::Error::new(io::ErrorKind::Unsupported, "only on macOS"))
            }
        }
    )*};
}

macos_only! {
    /// Whether App Nap may slow down apps that are not visible (its default).
    pub fn app_nap_enabled() -> bool;
    /// Turns App Nap on or off for the current user's apps. Needs no administrator rights.
    pub fn set_app_nap(enabled: bool) -> ();
    /// Whether the screen saver starts after a period of inactivity.
    pub fn screen_saver_enabled() -> bool;
    /// Turns the screen saver on or off for the current user. Needs no administrator rights.
    pub fn set_screen_saver(enabled: bool) -> ();
    /// Whether apps that were open at logout or restart are opened again at login.
    pub fn reopen_apps_enabled() -> bool;
    /// Turns reopening apps at login on or off for the current user. Needs no administrator
    /// rights.
    pub fn set_reopen_apps(enabled: bool) -> ();
    /// Whether the computer starts up on its own after a power failure.
    pub fn restart_after_power_failure() -> bool;
    /// Needs administrator rights (goes through `elevate`).
    pub(crate) fn set_restart_after_power_failure(enabled: bool) -> ();
    /// Whether network access (Wake on LAN) wakes the computer from sleep.
    pub fn wake_for_network() -> bool;
    /// Needs administrator rights (goes through `elevate`).
    pub(crate) fn set_wake_for_network(enabled: bool) -> ();
    /// Whether the Bluetooth Setup Assistant opens at startup when no keyboard or mouse is
    /// found.
    pub fn bluetooth_setup_assistant() -> bool;
    /// Needs administrator rights (goes through `elevate`).
    pub(crate) fn set_bluetooth_setup_assistant(enabled: bool) -> ();
}

/// Needs administrator rights. When enabling, the elevated helper collects the password
/// through the OS credential prompt; it is never passed in or stored in plain text.
pub(crate) fn set_auto_login(enabled: bool) -> io::Result<()> {
    imp::set_auto_login(enabled)
}

#[cfg(windows)]
#[path = "windows.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod imp;
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
#[path = "unsupported.rs"]
mod imp;
