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
#[cfg(not(any(windows, target_os = "linux")))]
#[path = "unsupported.rs"]
mod imp;
