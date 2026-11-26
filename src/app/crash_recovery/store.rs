//! Private, coalescing crash checkpoints. A live advisory lock prevents another
//! desktop from offering this window's drafts; claiming a stale snapshot retains
//! that same lock until the user restores or discards it.
use anyhow::{Context, Result, ensure};
use rode_host_core::locking::{self, PrivateFile};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt},
    },
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

/// Checkpoint size is bounded independently of the open document's size.
const LIMIT: usize = rode_host_core::persistence::LIMIT;

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::app) struct Entry {
    /// Informational original name/path; never authorizes a save after recovery.
    pub name: String,
    pub text: String,
    pub notebook: bool,
}

pub(super) struct Candidate {
    path: PathBuf,
    _lock: File,
    pub entries: Vec<Entry>,
    pub restored: bool,
    pub failure: Option<String>,
}
impl Candidate {
    /// Explicit removal under the claimed lock; failed removals remain recoverable.
    pub fn discard(&self) -> Result<()> {
        fs::remove_file(&self.path).context("Cannot remove the recovery checkpoint")
    }
}
#[derive(Default)]
struct Pending {
    next: Option<(u64, Vec<Entry>)>,
    sent: u64,
    saved: u64,
    error: Option<String>,
    reported: bool,
    closing: bool,
    discard: bool,
    closed: bool,
}
#[derive(Default)]
pub(super) struct Shared {
    pending: Mutex<Pending>,
    changed: Condvar,
}
impl Shared {
    /// A bounded durability barrier, usable without any egui or document locks.
    pub fn flush(&self, timeout: Duration) -> bool {
        let state = self.pending.lock().unwrap();
        let target = state.sent;
        let (state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |s| {
                s.saved < target && s.error.is_none() && !s.closed
            })
            .unwrap();
        state.saved >= target
    }
}
pub(super) struct Store {
    pub shared: Arc<Shared>,
}
impl Store {
    pub fn open(root: &Path) -> Result<(Self, Vec<Candidate>)> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)?;
        let metadata = fs::symlink_metadata(root)?;
        ensure!(
            metadata.is_dir()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o077 == 0,
            "Recovery storage must be a private directory owned by this user"
        );
        let mut candidates = Vec::new();
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            let Some(name) = path.file_stem().and_then(|n| n.to_str()) else {
                continue;
            };
            if path.extension().is_none_or(|e| e != "json")
                || !name.starts_with("s-")
                || name.len() != 34
                || !name[2..].bytes().all(|b| b.is_ascii_hexdigit())
            {
                continue;
            }
            let lock = locking::open_private(&path.with_extension("lock"), PrivateFile::Lock)?;
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::WouldBlock {
                    continue;
                }
                return Err(error.into());
            }
            let read = (|| -> Result<Vec<Entry>> {
                let file = match locking::open_private(&path, PrivateFile::Existing) {
                    Ok(file) => file,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                    Err(e) => return Err(e.into()),
                };
                let mut bytes = Vec::new();
                file.take(LIMIT as u64 + 1).read_to_end(&mut bytes)?;
                ensure!(bytes.len() <= LIMIT, "Recovery checkpoint exceeds 8 MiB");
                serde_json::from_slice(&bytes)
                    .with_context(|| format!("Cannot read recovery checkpoint {}", path.display()))
            })();
            let (entries, failure) = match read {
                Ok(entries) => (entries, None),
                Err(e) => (Vec::new(), Some(format!("{e:#}"))),
            };
            if !entries.is_empty() || failure.is_some() {
                candidates.push(Candidate {
                    path,
                    _lock: lock,
                    entries,
                    restored: false,
                    failure,
                });
            }
        }
        let path = root.join(format!("{}.json", rode_session::new_id()?));
        let lock = locking::open_private(&path.with_extension("lock"), PrivateFile::Lock)?;
        locking::lock(&lock)?;
        let shared = Arc::new(Shared::default());
        let writing = shared.clone();
        std::thread::Builder::new().name("draft-checkpoints".into()).spawn(move || {
            let _lock = lock;
            loop {
                let mut state = writing.pending.lock().unwrap();
                while state.next.is_none() && !state.closing {
                    state = writing.changed.wait(state).unwrap();
                }
                if state.closing && (state.discard || state.next.is_none()) {
                    let discard = state.discard;
                    drop(state);
                    if discard { let _ = fs::remove_file(&path); }
                    let mut state = writing.pending.lock().unwrap();
                    state.closed = true;
                    writing.changed.notify_all();
                    return;
                }
                let (version, entries) = state.next.take().unwrap();
                drop(state);
                let result = (|| -> Result<()> {
                    if entries.is_empty() {
                        match fs::remove_file(&path) {
                            Ok(()) => {},
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                            Err(e) => return Err(e.into()),
                        }
                    } else {
                        let bytes = serde_json::to_vec(&entries)?;
                        ensure!(bytes.len() <= LIMIT, "Unsaved work exceeds the 8 MiB desktop recovery limit; save it explicitly");
                        rode_host_core::persistence::replace(&path, &bytes, true)?;
                    }
                    Ok(())
                })();
                let mut state = writing.pending.lock().unwrap();
                match result {
                    Ok(()) => { state.saved = version; state.error = None; },
                    Err(e) => { state.error = Some(format!("{e:#}")); state.reported = false; },
                }
                writing.changed.notify_all();
            }
        })?;
        Ok((Self { shared }, candidates))
    }
    /// Latest-only queue: slow storage never creates an unbounded series of drafts.
    pub fn submit(&self, entries: Vec<Entry>) {
        let mut state = self.shared.pending.lock().unwrap();
        state.sent += 1;
        state.error = None;
        state.next = Some((state.sent, entries));
        self.shared.changed.notify_all();
    }
    /// Only a confirmed clean application exit may remove this instance's drafts.
    /// Drop during a panic or failed event loop preserves its checkpoint.
    pub fn clear_on_exit(&self) {
        self.shared.pending.lock().unwrap().discard = true;
    }
    pub fn error(&self) -> Option<String> {
        let mut state = self.shared.pending.lock().unwrap();
        if state.reported {
            return None;
        }
        state.reported = true;
        state.error.clone()
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        let mut state = self.shared.pending.lock().unwrap();
        state.closing = true;
        self.shared.changed.notify_all();
        // Bound normal-exit cleanup too: disconnected network storage must not
        // prevent the frontend from releasing its agent leases.
        let _ = self
            .shared
            .changed
            .wait_timeout_while(state, Duration::from_secs(1), |s| !s.closed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: &str) -> Entry {
        Entry {
            name: "/remote/unsaved.txt".into(),
            text: text.into(),
            notebook: false,
        }
    }

    #[test]
    fn active_and_claimed_checkpoints_are_exclusive_and_changes_coalesce() {
        let root = tempfile::tempdir().unwrap();
        let (store, old) = Store::open(root.path()).unwrap();
        assert!(old.is_empty());
        for version in 0..20 {
            store.submit(vec![entry(&version.to_string())]);
        }
        assert!(store.shared.flush(Duration::from_secs(3)));
        let (other, old) = Store::open(root.path()).unwrap();
        assert!(
            old.is_empty(),
            "A live window's checkpoint was offered for recovery"
        );
        drop(other);
        // Simulate process death by releasing its OS lock while leaving exactly
        // the durable file a new process will discover.
        let path = fs::read_dir(root.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|x| x == "json"))
            .unwrap();
        let crash = root
            .path()
            .join(format!("{}.json", rode_session::new_id().unwrap()));
        fs::copy(path, &crash).unwrap();
        let (recovering, old) = Store::open(root.path()).unwrap();
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].entries[0].text, "19");
        let (_third, unavailable) = Store::open(root.path()).unwrap();
        assert!(
            unavailable.is_empty(),
            "Two windows claimed the same recovery file"
        );
        old[0].discard().unwrap();
        drop(old);
        drop(recovering);
        store.clear_on_exit();
        drop(store);
        assert!(!crash.exists());
        assert!(
            fs::read_dir(root.path()).unwrap().all(|e| e
                .unwrap()
                .path()
                .extension()
                .is_none_or(|x| x != "json")),
            "Clean shutdown retained stale drafts"
        );
    }

    #[test]
    fn a_failed_checkpoint_keeps_the_last_durable_copy_and_reports_the_error() {
        let root = tempfile::tempdir().unwrap();
        let (store, _) = Store::open(root.path()).unwrap();
        store.submit(vec![entry("last safe version")]);
        assert!(store.shared.flush(Duration::from_secs(3)));
        store.submit(vec![entry(&"x".repeat(LIMIT + 1))]);
        assert!(!store.shared.flush(Duration::from_secs(3)));
        assert!(store.error().unwrap().contains("8 MiB"));
        assert!(store.error().is_none());
        let path = fs::read_dir(root.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|x| x == "json"))
            .unwrap();
        let entries: Vec<Entry> = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(entries[0].text, "last safe version");
    }
    #[test]
    fn interrupted_shutdown_keeps_pending_drafts_and_corrupt_files_do_not_disable_new_checkpoints()
    {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let broken = root
            .path()
            .join(format!("{}.json", rode_session::new_id().unwrap()));
        locking::open_private(&broken, PrivateFile::New)
            .unwrap()
            .write_all(b"partial json")
            .unwrap();
        let (store, candidates) = Store::open(root.path()).unwrap();
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].failure.is_some());
        store.submit(vec![entry("latest draft")]);
        drop(store); // Unconfirmed exit must preserve even an already queued write.
        drop(candidates);
        let (next, candidates) = Store::open(root.path()).unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(
            candidates
                .iter()
                .any(|c| c.entries.first().is_some_and(|e| e.text == "latest draft"))
        );
        assert_eq!(fs::read(&broken).unwrap(), b"partial json");
        for candidate in candidates {
            candidate.discard().unwrap();
        }
        next.clear_on_exit();
    }
}
