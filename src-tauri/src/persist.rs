//! Debounced, atomic JSON persistence.
//!
//! One helper, used by both things that have to survive a relaunch: the
//! loudness cache of SPEC §8 and the `settings.json` of §12. The rules it
//! exists to enforce are the ones a player gets wrong at 3 a.m.:
//!
//! * **A command never blocks on the disk.** [`AtomicWriter::queue`] only swaps
//!   a `String` into a mutex and pokes an unbounded channel; the write happens
//!   on a dedicated thread.
//! * **Never write from the audio or decode thread.** Nothing here is called
//!   from either — the queue is the only entry point, and it does no IO.
//! * **Debounced.** A burst of edits (dragging an EQ node, decoding 200 files)
//!   collapses into one write at most every [`DEBOUNCE`]. The debounce is a
//!   *deadline*, not a sliding window: a continuous stream of edits still gets
//!   persisted every two seconds instead of never.
//! * **Atomic.** Write `*.tmp`, fsync, rename. A power cut can lose the last
//!   two seconds of settings; it can never leave half a JSON file behind, which
//!   is what turns "your EQ curve is gone" into "Onyx will not start".

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, unbounded, RecvTimeoutError, Sender};
use parking_lot::Mutex;

/// Minimum gap between two writes of the same file (SPEC §8).
pub const DEBOUNCE: Duration = Duration::from_secs(2);
/// How long [`AtomicWriter::flush_blocking`] waits for the writer thread before
/// giving up and writing inline. Generous: this only runs at shutdown.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

enum Msg {
    /// New payload waiting in `pending`.
    Wake,
    /// Write now and acknowledge (shutdown).
    Flush(Sender<()>),
}

/// A file that is written at most every [`DEBOUNCE`], always atomically.
///
/// `path` is optional because `app_config_dir()` / `app_cache_dir()` can fail
/// on a locked-down system. Onyx still has to run, so a writer with no path
/// silently accepts payloads and drops them — the in-memory state stays correct
/// and only persistence is lost.
pub struct AtomicWriter {
    path: Option<PathBuf>,
    pending: Arc<Mutex<Option<String>>>,
    tx: Option<Sender<Msg>>,
}

impl AtomicWriter {
    /// Spawn the writer thread. `name` only shows up in thread names and logs.
    pub fn spawn(path: Option<PathBuf>, name: &str, debounce: Duration) -> AtomicWriter {
        let pending = Arc::new(Mutex::new(None::<String>));
        let Some(path) = path else {
            log::warn!(
                "no writable directory for {name}; it will work this session but will not be \
                 remembered after a restart"
            );
            return AtomicWriter {
                path: None,
                pending,
                tx: None,
            };
        };
        let (tx, rx) = unbounded::<Msg>();
        let worker_path = path.clone();
        let worker_pending = Arc::clone(&pending);
        let spawned = std::thread::Builder::new()
            .name(format!("onyx-write-{name}"))
            .spawn(move || run(rx, worker_path, worker_pending, debounce));
        match spawned {
            Ok(_) => AtomicWriter {
                path: Some(path),
                pending,
                tx: Some(tx),
            },
            Err(e) => {
                // Without the thread, `queue` would silently accumulate; fall
                // back to writing inline from the caller instead of lying.
                log::warn!(
                    "could not spawn the {name} writer thread ({e}); falling back to writing it \
                     inline, which may briefly stall the caller"
                );
                AtomicWriter {
                    path: Some(path),
                    pending,
                    tx: None,
                }
            }
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Hand over the complete new file contents. Never blocks, never touches
    /// the disk on the calling thread (unless the writer thread is missing).
    pub fn queue(&self, text: String) {
        *self.pending.lock() = Some(text);
        match &self.tx {
            Some(tx) => {
                if tx.send(Msg::Wake).is_err() {
                    log::error!(
                        "the persistence writer stopped; changes are no longer being saved"
                    );
                }
            }
            // No thread: either no path (drop it) or the spawn failed (write it).
            None => {
                if self.path.is_some() {
                    self.write_now();
                } else {
                    self.pending.lock().take();
                }
            }
        }
    }

    /// Write any pending payload now and wait for it. Called once, at exit.
    pub fn flush_blocking(&self) {
        if self.path.is_none() {
            return;
        }
        if let Some(tx) = &self.tx {
            let (ack, done) = bounded::<()>(1);
            if tx.send(Msg::Flush(ack)).is_ok() && done.recv_timeout(FLUSH_TIMEOUT).is_ok() {
                return;
            }
        }
        // Thread gone or too slow: better a synchronous write at shutdown than
        // losing the user's settings.
        self.write_now();
    }

    fn write_now(&self) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let Some(text) = self.pending.lock().take() else {
            return;
        };
        if let Err(e) = write_atomic(path, &text) {
            log::error!(
                "could not write {}: {e} - those changes are lost",
                path.display()
            );
        }
    }
}

fn run(
    rx: crossbeam_channel::Receiver<Msg>,
    path: PathBuf,
    pending: Arc<Mutex<Option<String>>>,
    debounce: Duration,
) {
    let write = |ack: Option<Sender<()>>| {
        // The lock is held only for the `take`, never across the IO.
        let payload = pending.lock().take();
        if let Some(text) = payload {
            if let Err(e) = write_atomic(&path, &text) {
                log::error!(
                    "could not write {}: {e} - those changes are lost",
                    path.display()
                );
            }
        }
        if let Some(ack) = ack {
            let _ = ack.send(());
        }
    };

    while let Ok(first) = rx.recv() {
        match first {
            Msg::Flush(ack) => {
                write(Some(ack));
                continue;
            }
            Msg::Wake => {}
        }
        // Coalesce everything that arrives inside the debounce window. The
        // deadline is fixed at the *first* edit so a continuous drag cannot
        // postpone the write for ever.
        let deadline = Instant::now() + debounce;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(Msg::Wake) => continue,
                Ok(Msg::Flush(ack)) => {
                    write(Some(ack));
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {
                    write(None);
                    break;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    write(None);
                    return;
                }
            }
        }
    }
    // Channel closed (app shutting down): do not lose the last payload.
    write(None);
}

/// Write `text` to `path` so that a reader only ever sees the old or the new
/// contents: temp file in the same directory, fsync, rename.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("onyx"));
    name.push(".tmp");
    let tmp = path.with_file_name(name);

    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        // Rename is only atomic with respect to the *directory entry*; without
        // the fsync the bytes may not be on disk yet after a crash.
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        // Never leave the temp file lying around to be mistaken for state.
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Read a file that is allowed not to exist. `Ok(None)` = "nothing saved yet",
/// `Err` = "there is something there and we could not read it", which the
/// callers turn into a warning rather than a failure.
pub fn read_optional(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Seconds since the Unix epoch, or 0 if the clock is before 1970.
pub fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("onyx-persist-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn write_atomic_creates_the_directory_and_leaves_no_temp_file() {
        let dir = tmp_dir("atomic");
        let path = dir.join("nested").join("settings.json");
        write_atomic(&path, "{\"a\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");
        let siblings: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(siblings, ["settings.json"], "a .tmp file survived");

        // A second write replaces the contents rather than appending.
        write_atomic(&path, "{}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn queue_does_not_write_immediately_but_flush_does() {
        let dir = tmp_dir("debounce");
        let path = dir.join("cache.json");
        let w = AtomicWriter::spawn(Some(path.clone()), "test", Duration::from_secs(30));
        w.queue("first".into());
        w.queue("second".into());
        // Still debounced: nothing on disk yet.
        std::thread::sleep(Duration::from_millis(120));
        assert!(!path.exists(), "queue() wrote synchronously");
        // Only the newest payload is ever written.
        w.flush_blocking();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_debounce_deadline_fires_without_a_flush() {
        let dir = tmp_dir("deadline");
        let path = dir.join("cache.json");
        let w = AtomicWriter::spawn(Some(path.clone()), "test", Duration::from_millis(60));
        w.queue("payload".into());
        // A continuous stream of edits must not postpone the write for ever.
        for i in 0..40 {
            w.queue(format!("payload {i}"));
            std::thread::sleep(Duration::from_millis(5));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while !path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(path.exists(), "the debounce deadline never fired");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_writer_with_no_path_is_a_silent_no_op() {
        let w = AtomicWriter::spawn(None, "test", DEBOUNCE);
        w.queue("dropped".into());
        w.flush_blocking();
        assert!(w.path().is_none());
        assert!(w.pending.lock().is_none(), "payload leaked");
    }

    #[test]
    fn read_optional_distinguishes_missing_from_broken() {
        let dir = tmp_dir("read");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nope.json");
        assert!(read_optional(&path).unwrap().is_none());
        std::fs::write(&path, "hello").unwrap();
        assert_eq!(read_optional(&path).unwrap().as_deref(), Some("hello"));
        // A directory where a file was expected is a real error, not "missing".
        assert!(read_optional(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
