//! Placeholders for platforms whose settings are not implemented yet.

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

pub fn security_notifications_enabled() -> io::Result<bool> {
    Err(unsupported())
}

pub fn set_security_notifications(_enabled: bool) -> io::Result<()> {
    Err(unsupported())
}

pub fn set_security_notifications_user(_enabled: bool) -> io::Result<()> {
    Err(unsupported())
}

pub fn firewall_enabled() -> io::Result<bool> {
    Err(unsupported())
}

pub fn set_firewall(_enabled: bool) -> io::Result<()> {
    Err(unsupported())
}

pub fn app_nap_enabled() -> io::Result<bool> {
    Err(unsupported())
}

pub fn set_app_nap(_enabled: bool) -> io::Result<()> {
    Err(unsupported())
}
