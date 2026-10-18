use std::io;
use std::path::{Path, PathBuf};

/// Where Gravekeeper keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    /// The per-user platform locations, or everything under `$GRAVEKEEPER_HOME` if it is set
    /// (for portable installs and testing).
    ///
    /// - Linux and other Unix: `$XDG_CONFIG_HOME/gravekeeper` and `$XDG_DATA_HOME/gravekeeper`
    ///   (defaulting to `~/.config` and `~/.local/share`)
    /// - macOS: `~/Library/Application Support/Gravekeeper`
    /// - Windows: `%APPDATA%\Gravekeeper\config` and `%APPDATA%\Gravekeeper\data`
    pub fn from_env() -> io::Result<Self> {
        if let Some(home) = std::env::var_os("GRAVEKEEPER_HOME") {
            return Ok(Self::in_dir(PathBuf::from(home)));
        }
        let missing = || io::Error::other("cannot determine the user's home directory");
        let (config_dir, data_dir) = if cfg!(windows) {
            let app = absolute_env("APPDATA")
                .ok_or_else(missing)?
                .join("Gravekeeper");
            (app.join("config"), app.join("data"))
        } else if cfg!(target_os = "macos") {
            let app = home_dir()
                .ok_or_else(missing)?
                .join("Library/Application Support/Gravekeeper");
            (app.clone(), app)
        } else {
            let data = absolute_env("XDG_DATA_HOME")
                .or_else(|| home_dir().map(|h| h.join(".local/share")))
                .ok_or_else(missing)?;
            (
                config_base_dir().ok_or_else(missing)?.join("gravekeeper"),
                data.join("gravekeeper"),
            )
        };
        Ok(Self {
            config_file: config_dir.join("config.toml"),
            data_dir,
        })
    }

    pub fn in_dir(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        Self {
            config_file: dir.join("config.toml"),
            data_dir: dir.to_path_buf(),
        }
    }

    pub fn history_db(&self) -> PathBuf {
        self.data_dir.join("history.sqlite3")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.data_dir.join("gravekeeper.lock")
    }
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::home_dir().filter(|p| p.is_absolute())
}

/// `$XDG_CONFIG_HOME`, or `~/.config` (the XDG base directory for configuration on Unix).
pub fn config_base_dir() -> Option<PathBuf> {
    absolute_env("XDG_CONFIG_HOME").or_else(|| home_dir().map(|h| h.join(".config")))
}

/// An environment variable holding an absolute path; relative values are ignored, as the
/// XDG specification requires.
fn absolute_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// Creates `dir` (and parents) readable only by the current user where the OS supports it.
pub(crate) fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}
