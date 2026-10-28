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

#[cfg(windows)]
mod imp {
    use std::io;

    use windows_registry::LOCAL_MACHINE;

    /// The "Configure Automatic Updates" group policy. `NoAutoUpdate = 1` is its "Disabled"
    /// state; without the value Windows Update keeps its default (automatic).
    const AU_KEY: &str = r"SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU";
    const NO_AUTO_UPDATE: &str = "NoAutoUpdate";

    /// Windows Home ignores update policies (its edition IDs start with "Core").
    fn require_policies() -> io::Result<()> {
        let edition = LOCAL_MACHINE
            .open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
            .and_then(|key| key.get_string("EditionID"))
            .unwrap_or_default();
        if edition.starts_with("Core") {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Windows Home ignores update policies",
            ))
        } else {
            Ok(())
        }
    }

    pub fn automatic_updates() -> io::Result<bool> {
        require_policies()?;
        let disabled = LOCAL_MACHINE
            .open(AU_KEY)
            .and_then(|key| key.get_u32(NO_AUTO_UPDATE))
            .is_ok_and(|value| value == 1);
        Ok(!disabled)
    }

    pub fn set_automatic_updates(enabled: bool) -> io::Result<()> {
        require_policies()?;
        if !enabled {
            return LOCAL_MACHINE
                .create(AU_KEY)
                .and_then(|key| key.set_u32(NO_AUTO_UPDATE, 1))
                .map_err(io::Error::from);
        }
        let key = match LOCAL_MACHINE.options().read().write().open(AU_KEY) {
            Ok(key) => key,
            Err(err) => {
                let err = io::Error::from(err);
                // No policy key: already the default.
                return if err.kind() == io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(err)
                };
            }
        };
        if key.get_u32(NO_AUTO_UPDATE).is_ok() {
            key.remove_value(NO_AUTO_UPDATE).map_err(io::Error::from)?;
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
