//! Native file and folder dialogs that do not block the UI, plus opening folders in the
//! system file manager.
//!
//! On Linux the dialogs go through the XDG desktop portal (no GTK needed); on Windows and
//! Linux they run on a helper thread. macOS requires dialogs on the main thread, where the
//! dialog is modal anyway.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Folder,
}

/// One pending dialog at a time, tagged with what the result is for.
pub struct Picker<T> {
    pending: Option<(T, mpsc::Receiver<Option<PathBuf>>)>,
}

impl<T> Default for Picker<T> {
    fn default() -> Self {
        Self { pending: None }
    }
}

impl<T: Copy + PartialEq> Picker<T> {
    pub fn is_open(&self) -> bool {
        self.pending.is_some()
    }

    /// Opens a dialog unless one is already open. `start` is shown initially if it exists.
    pub fn open(
        &mut self,
        ctx: &egui::Context,
        tag: T,
        kind: Kind,
        title: &str,
        start: Option<&Path>,
    ) {
        if self.pending.is_some() {
            return;
        }
        let mut dialog = rfd::FileDialog::new().set_title(title);
        if let Some(dir) = start.and_then(existing_dir) {
            dialog = dialog.set_directory(dir);
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let run = move || {
            let result = match kind {
                Kind::File => dialog.pick_file(),
                Kind::Folder => dialog.pick_folder(),
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        };
        if cfg!(target_os = "macos") {
            run();
        } else {
            std::thread::spawn(run);
        }
        self.pending = Some((tag, rx));
    }

    /// The chosen path, once, when the dialog for `tag` has closed with a selection.
    pub fn take(&mut self, tag: T) -> Option<PathBuf> {
        let (pending_tag, rx) = self.pending.as_ref()?;
        if *pending_tag != tag {
            return None;
        }
        match rx.try_recv() {
            Ok(result) => {
                self.pending = None;
                result
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                None
            }
        }
    }
}

/// The directory to start a dialog in: the path itself if it is a directory, else its parent.
fn existing_dir(path: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        Some(path.to_path_buf())
    } else {
        path.parent().filter(|p| p.is_dir()).map(Path::to_path_buf)
    }
}

/// Opens a folder (or the folder containing a file) in the system file manager.
pub fn reveal(path: &Path) {
    let target = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map_or_else(|| path.to_path_buf(), Path::to_path_buf)
    };
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(err) = std::process::Command::new(program).arg(&target).spawn() {
        tracing::warn!(%err, path = %target.display(), "cannot open the file manager");
    }
}
