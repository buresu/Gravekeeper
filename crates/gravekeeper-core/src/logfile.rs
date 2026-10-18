//! Size-based log file rotation, and a writer thread so file I/O never blocks the engine.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

/// A file that is renamed to `name.1` (and older ones to `.2`, `.3`, ...) when it would grow
/// beyond `max_bytes`; at most `keep` rotated files are kept.
pub struct RotatingFile {
    path: PathBuf,
    max_bytes: u64,
    keep: u32,
    file: Option<File>,
    size: u64,
}

impl RotatingFile {
    pub fn new(path: impl Into<PathBuf>, max_bytes: u64, keep: u32) -> Self {
        Self {
            path: path.into(),
            max_bytes: max_bytes.max(1),
            keep,
            file: None,
            size: 0,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn open(&mut self) -> io::Result<&mut File> {
        if self.file.is_none() {
            if let Some(dir) = self.path.parent() {
                crate::paths::create_private_dir(dir)?;
            }
            let mut options = OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let file = options.open(&self.path)?;
            self.size = file.metadata()?.len();
            self.file = Some(file);
        }
        Ok(self.file.as_mut().expect("just opened"))
    }

    fn rotated(&self, n: u32) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".{n}"));
        name.into()
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file = None;
        if self.keep == 0 {
            return std::fs::remove_file(&self.path);
        }
        let _ = std::fs::remove_file(self.rotated(self.keep));
        for n in (1..self.keep).rev() {
            let from = self.rotated(n);
            if from.exists() {
                std::fs::rename(&from, self.rotated(n + 1))?;
            }
        }
        std::fs::rename(&self.path, self.rotated(1))
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size > 0 && self.size + buf.len() as u64 > self.max_bytes {
            self.open()?;
            self.rotate()?;
        }
        let n = self.open()?.write(buf)?;
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

enum Message {
    Line {
        file: Arc<PathBuf>,
        text: String,
    },
    /// Sent by [`LogWriterThread`] when dropped: write what is queued, then exit, even if
    /// some [`LogWriter`] clones are still alive.
    Stop,
}

/// Writes lines to many rotating files on a background thread. Sending never blocks: when
/// the queue is full, lines are dropped and counted.
#[derive(Clone)]
pub struct LogWriter {
    tx: mpsc::SyncSender<Message>,
    dropped: Arc<AtomicU64>,
}

const QUEUE: usize = 10_000;

impl LogWriter {
    pub fn start(max_bytes: u64, keep: u32) -> io::Result<(Self, LogWriterThread)> {
        let (tx, rx) = mpsc::sync_channel::<Message>(QUEUE);
        let thread = std::thread::Builder::new()
            .name("gravekeeper-logs".into())
            .spawn(move || {
                let mut files: HashMap<Arc<PathBuf>, RotatingFile> = HashMap::new();
                let mut failed: std::collections::HashSet<Arc<PathBuf>> = Default::default();
                let mut stop = false;
                while !stop && let Ok(first) = rx.recv() {
                    for message in std::iter::once(first).chain(rx.try_iter()) {
                        let Message::Line { file: path, text } = message else {
                            stop = true;
                            continue;
                        };
                        let file = files.entry(Arc::clone(&path)).or_insert_with(|| {
                            RotatingFile::new(path.as_path(), max_bytes, keep)
                        });
                        if let Err(err) = file.write_all(text.as_bytes()) {
                            // Report each broken file once instead of once per line.
                            if failed.insert(Arc::clone(&path)) {
                                tracing::warn!(path = %path.display(), %err, "cannot write log file");
                            }
                        }
                    }
                    for file in files.values_mut() {
                        let _ = file.flush();
                    }
                }
            })?;
        Ok((
            Self {
                tx: tx.clone(),
                dropped: Arc::new(AtomicU64::new(0)),
            },
            LogWriterThread {
                stop: tx,
                thread: Some(thread),
            },
        ))
    }

    pub fn write(&self, file: &Arc<PathBuf>, text: String) {
        let line = Message::Line {
            file: Arc::clone(file),
            text,
        };
        if self.tx.try_send(line).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Lines dropped because the writer could not keep up.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Owns the writer thread: dropping it writes out the queue and joins the thread.
pub struct LogWriterThread {
    stop: mpsc::SyncSender<Message>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for LogWriterThread {
    fn drop(&mut self) {
        let _ = self.stop.send(Message::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Copies `file` into `dir` with a timestamp and keeps only the newest `keep` copies.
pub fn backup(file: &Path, dir: &Path, keep: usize) -> io::Result<Option<PathBuf>> {
    if !file.exists() {
        return Ok(None);
    }
    crate::paths::create_private_dir(dir)?;
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("backup");
    let ext = file.extension().and_then(|s| s.to_str()).unwrap_or("bak");
    let stamp = jiff::Timestamp::now()
        .strftime("%Y%m%d-%H%M%S%.3f")
        .to_string();
    let target = dir.join(format!("{stem}-{stamp}.{ext}"));
    std::fs::copy(file, &target)?;

    let prefix = format!("{stem}-");
    let mut backups: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix))
        })
        .collect();
    // The timestamp format sorts chronologically.
    backups.sort();
    let excess = backups.len().saturating_sub(keep);
    for old in &backups[..excess] {
        std::fs::remove_file(old)?;
    }
    Ok(Some(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(p: impl AsRef<Path>) -> String {
        std::fs::read_to_string(p).unwrap_or_default()
    }

    #[test]
    fn rotates_and_keeps_a_limited_number_of_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/app.log");
        let mut file = RotatingFile::new(&path, 10, 2);
        for line in ["aaaa\n", "bbbb\n", "cccc\n", "dddd\n", "eeee\n"] {
            file.write_all(line.as_bytes()).unwrap();
        }
        file.flush().unwrap();
        assert_eq!(read(&path), "eeee\n");
        assert_eq!(read(file.rotated(1)), "cccc\ndddd\n");
        assert_eq!(read(file.rotated(2)), "aaaa\nbbbb\n");
        assert!(!file.rotated(3).exists());
    }

    #[test]
    fn appends_to_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.log");
        std::fs::write(&path, "old\n").unwrap();
        let mut file = RotatingFile::new(&path, 1000, 1);
        file.write_all(b"new\n").unwrap();
        assert_eq!(read(&path), "old\nnew\n");
    }

    #[test]
    fn writer_thread_writes_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("w.log"));
        let (writer, thread) = LogWriter::start(1 << 20, 1).unwrap();
        writer.write(&path, "one\n".into());
        writer.write(&path, "two\n".into());
        // Stops even though a writer clone is still alive.
        drop(thread);
        writer.write(&path, "late\n".into());
        assert_eq!(read(path.as_path()), "one\ntwo\n");
    }

    #[test]
    fn backups_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        std::fs::write(&file, "x").unwrap();
        let backups = dir.path().join("backups");
        for _ in 0..4 {
            backup(&file, &backups, 2).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(std::fs::read_dir(&backups).unwrap().count(), 2);
        assert!(
            backup(&dir.path().join("none"), &backups, 2)
                .unwrap()
                .is_none()
        );
    }
}
