//! Cooperating processes merge preference edits under a stable advisory lock.
//! eframe uses a private snapshot: its whole-file writer never touches shared state.
//!
//! # Design decisions
//!
//! Each window remembers its own baseline, so writing that whole snapshot would
//! undo unrelated edits made by another window. We instead record explicit field
//! changes and apply them to the latest file while holding a transaction lock.
//! This gives independent settings a useful merge rule without introducing a
//! database or a general conflict-resolution engine. Concurrent explicit edits
//! to the same scalar/array are resolved by transaction order.
//!
//! The stable adjacent lock survives atomic replacement: locking only the old
//! configuration inode would stop coordinating writers after a rename. The
//! current destination is also locked to cooperate with source-file writers.
//! Locks serialize the merge; atomic publication prevents partial file contents.
//! Neither mechanism substitutes for the other.
//!
//! Preference writes run on a worker because a filesystem lock or sync may stall.
//! Failed work pauses until an explicit retry, avoiding repeated I/O on an idle
//! frame. This means [`eframe::Storage::flush`] queues work, not a durability wait.
//! Eframe gets a private snapshot because its whole-file persistence cannot
//! express these merge rules; [`NativeState`] bridges layout restoration without
//! letting the framework replace shared settings.
use anyhow::{Context, Result, ensure};
use eframe::Storage;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Maximum serialized configuration size in bytes.
pub const LIMIT: usize = 8 * 1024 * 1024;
type Values = BTreeMap<String, String>;
static SERIAL: AtomicU64 = AtomicU64::new(0);

/// Open before checking file type without waiting for a FIFO peer. Regular-file
/// reads/writes retain their usual behavior; symlinks remain supported.
pub(crate) fn open_regular(path: &Path, writable: bool) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .write(writable)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Only regular files can be accessed.",
        ));
    }
    Ok(file)
}

/// Read a bounded regular file; a missing path returns `None`, while oversized or nonregular
/// files fail. This reads bytes without parsing their configuration format.
pub fn read(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
    let file = match open_regular(path, false) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("Cannot read {}", path.display())),
    };
    ensure!(
        file.metadata()?.is_file(),
        "{} is not a regular file",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "{} exceeds its size limit",
        path.display()
    );
    Ok(Some(bytes))
}

/// The file remains locked until its handle is dropped. Never unlink lock files.
pub(crate) fn lock(file: &File) -> Result<()> {
    use std::os::fd::AsRawFd;
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock
            && error.kind() != std::io::ErrorKind::Interrupted
        {
            return Err(error).context("Cannot lock the file safely");
        }
        ensure!(
            Instant::now() < until,
            "File is busy in another process; changes were not saved. Try again."
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn private_file(path: &Path, exclusive: bool) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    if exclusive {
        options.create_new(true);
    } else {
        options.create(true);
    }
    Ok(options.open(path)?)
}

/// Serialize a read/modify/replace transaction under the stable configuration lock and
/// current destination inode lock. The callback receives whether the file existed after
/// locking; it must reload the latest data, parse/merge it, and call [`replace`] if needed.
///
/// This function does not write or roll back callback side effects. Parse before publishing
/// so malformed files remain intact. Lock waits may block; call from a worker. Handles are
/// released on return, while the stable adjacent lock file remains for other processes.
pub fn transaction<T>(path: &Path, update: impl FnOnce(bool) -> Result<T>) -> Result<T> {
    let parent = path.parent().context("Configuration path has no parent")?;
    fs::create_dir_all(parent)?;
    let file = private_file(
        &path.with_file_name(format!(
            "{}.lock",
            path.file_name()
                .context("Missing filename")?
                .to_string_lossy()
        )),
        false,
    )?;
    lock(&file)?;
    let existing = lock_existing(path)?;
    update(existing.is_some())
}

/// Follow inode replacements after waiting, so all RODE file writers cooperate.
pub(crate) fn lock_existing(path: &Path) -> Result<Option<File>> {
    use std::os::unix::fs::MetadataExt;
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let mut file = match open_regular(path, false) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            file.metadata()?.is_file(),
            "Only regular files can be written."
        );
        if let Err(error) = lock(&file) {
            // Linux local filesystems permit read-only flock handles. Some
            // network filesystems require a writable descriptor for EX locks.
            if error
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::raw_os_error)
                == Some(libc::EBADF)
            {
                file = open_regular(path, true)?;
                lock(&file)?;
            } else {
                return Err(error);
            }
        }
        let opened = file.metadata()?;
        // Waiting on an inode that has since been replaced does not protect the
        // current path. Reopen before doing a revision check or merge on it.
        if fs::metadata(path)
            .is_ok_and(|current| current.dev() == opened.dev() && current.ino() == opened.ino())
        {
            return Ok(Some(file));
        }
        ensure!(
            Instant::now() < until,
            "File keeps changing in another process; changes were not saved."
        );
    }
}

/// Atomically publish configuration bytes; `overwrite = false` refuses an existing destination.
/// Call inside [`transaction`] when merging shared state. Parent-directory sync makes
/// publication durable where supported.
pub fn replace(path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    ensure!(
        bytes.len() <= LIMIT,
        "Configuration exceeds 8 MiB; changes were not saved."
    );
    let temporary = path.with_file_name(format!(
        ".rode-config-{}-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    // Only remove a temporary file after successfully creating it ourselves.
    let mut file = private_file(&temporary, true)?;
    let result = (|| {
        lock(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if overwrite {
            fs::rename(&temporary, path)?;
        } else {
            // A prior existence check would race another creator. Linking the
            // completed inode publishes only if the destination is still absent.
            fs::hard_link(&temporary, path).context(
                "Configuration appeared in another process; changes were not saved. Try again.",
            )?;
            fs::remove_file(&temporary)?;
        }
        File::open(path.parent().context("Missing configuration directory")?)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// One JSON-path edit, including optional baseline data for conditional record eviction.
#[derive(Clone)]
pub struct Edit {
    path: Vec<String>,
    value: Option<Value>,
    previous: Option<Value>,
}

/// Object fields merge independently; arrays and scalars are single settings.
///
/// # Design
///
/// Object keys identify settings across windows. Array positions do not identify
/// the same logical item after concurrent insertions, so arrays are deliberately
/// replaced as a unit. This keeps the merge rule predictable without guessing at
/// list identity or silently combining incompatible ordered settings.
pub fn changes(before: &Value, after: &Value) -> Vec<Edit> {
    fn visit(before: &Value, after: &Value, path: Vec<String>, edits: &mut Vec<Edit>) {
        if before == after {
            return;
        }
        if let Some(next) = after.as_object() {
            let old = before.as_object();
            for (key, value) in next {
                let mut child = path.clone();
                child.push(key.clone());
                visit(
                    old.and_then(|m| m.get(key)).unwrap_or(&Value::Null),
                    value,
                    child,
                    edits,
                );
            }
            if let Some(old) = old {
                for key in old.keys() {
                    if !next.contains_key(key) {
                        let mut child = path.clone();
                        child.push(key.clone());
                        edits.push(Edit {
                            path: child,
                            value: None,
                            previous: None,
                        });
                    }
                }
            }
        } else {
            edits.push(Edit {
                path,
                value: Some(after.clone()),
                previous: None,
            });
        }
    }
    let mut edits = vec![];
    visit(before, after, vec![], &mut edits);
    edits
}

/// Apply edits in order, creating object parents as needed and respecting conditional
/// deletions.
pub fn apply(value: &mut Value, edits: &[Edit]) {
    for edit in edits {
        let Some((last, parents)) = edit.path.split_last() else {
            if let Some(next) = &edit.value {
                *value = next.clone();
            }
            continue;
        };
        if edit.value.is_none() {
            let mut cursor = Some(&mut *value);
            for key in parents {
                cursor = cursor.and_then(|v| v.as_object_mut()?.get_mut(key));
            }
            if let Some(map) = cursor.and_then(Value::as_object_mut)
                && edit
                    .previous
                    .as_ref()
                    .is_none_or(|old| map.get(last) == Some(old))
            {
                map.remove(last);
            }
            continue;
        }
        let mut cursor = &mut *value;
        for key in parents {
            if !cursor.is_object() {
                *cursor = serde_json::json!({});
            }
            cursor = cursor
                .as_object_mut()
                .unwrap()
                .entry(key)
                .or_insert(Value::Null);
        }
        if !cursor.is_object() {
            *cursor = serde_json::json!({});
        }
        let map = cursor.as_object_mut().unwrap();
        if let Some(next) = &edit.value {
            map.insert(last.clone(), next.clone());
        }
    }
}

fn structured(key: &str) -> bool {
    matches!(
        key,
        "project_preferences" | "agent_preferences" | "sandbox_defaults" | "extension_preferences"
    )
}

/// Normalize known defaults only for comparison, then overlay original fields. Merely opening
/// a settings form must not turn omitted defaults or unknown future fields into explicit
/// overwrite/deletion edits.
/// Preserving the original fields also lets older and newer editor versions share
/// storage without an older form erasing settings it cannot represent.
fn comparison_base(key: &str, raw: &Value, next: &Value) -> Result<Value> {
    let mut seed = raw.clone();
    if key != "sandbox_defaults" {
        for project in next.as_object().context("Invalid preference map")?.keys() {
            seed.as_object_mut()
                .context("Invalid preference map")?
                .entry(project)
                .or_insert(serde_json::json!({}));
        }
    }
    let mut normalized = match key {
        "extension_preferences" => {
            serde_json::to_value(crate::extensions::preferences(&seed.to_string())?)?
        }
        "project_preferences" => crate::app::project_preference_defaults(seed)?,
        "agent_preferences" => {
            serde_json::to_value(serde_json::from_value::<crate::agent::Preferences>(seed)?)?
        }
        "sandbox_defaults" => {
            serde_json::to_value(serde_json::from_value::<crate::sandbox::Options>(seed)?)?
        }
        _ => raw.clone(),
    };
    fn overlay(base: &mut Value, raw: &Value) {
        if let (Some(target), Some(source)) = (base.as_object_mut(), raw.as_object()) {
            for (key, value) in source {
                overlay(target.entry(key).or_insert(Value::Null), value);
            }
        } else {
            *base = raw.clone();
        }
    }
    overlay(&mut normalized, raw);
    Ok(normalized)
}

fn edit_size(edit: &Edit) -> usize {
    edit.path.iter().map(String::len).sum::<usize>()
        + 64
        + edit.value.as_ref().map_or(0, |v| v.to_string().len())
        + edit.previous.as_ref().map_or(0, |v| v.to_string().len())
}
fn decode(values: &Values) -> Result<Value> {
    let mut result = serde_json::Map::new();
    for (key, text) in values {
        result.insert(
            key.clone(),
            if structured(key) {
                match key.as_str() {
                    "extension_preferences" => { crate::extensions::preferences(text)?; }
                    "project_preferences" => crate::app::validate_project_preferences(text)?,
                    "agent_preferences" => {
                        let agents: crate::agent::Preferences = serde_json::from_str(text)?;
                        ensure!(agents.len()<=128 && text.len()<=256*1024,"Agent preferences exceed their size limit; existing file was preserved.");
                    }
                    "sandbox_defaults" => {
                        let _: crate::sandbox::Options = serde_json::from_str(text)?;
                    }
                    _ => {}
                }
                let value: Value = serde_json::from_str(text).with_context(|| {
                    format!("Invalid {key}; existing preferences were preserved")
                })?;
                ensure!(value.is_object(), "Invalid {key}; expected an object");
                value
            } else {
                Value::String(text.clone())
            },
        );
    }
    Ok(Value::Object(result))
}
fn load(path: &Path) -> Result<Values> {
    read(path, LIMIT)?
        .map(|bytes| {
            ron::de::from_bytes(&bytes).with_context(|| {
                format!(
                    "Invalid {}; existing preferences were preserved",
                    path.display()
                )
            })
        })
        .transpose()
        .map(Option::unwrap_or_default)
}
/// Load the latest shared file inside its transaction and apply only queued paths. Never
/// serialize this window's stale whole-file snapshot over concurrent edits.
fn write_edits(path: &Path, edits: &[Edit]) -> Result<()> {
    transaction(path, |existed| {
        let mut latest = decode(&load(path)?)?;
        apply(&mut latest, edits);
        let mut values = Values::new();
        for (key, value) in latest.as_object().context("Invalid preference map")? {
            let text = if structured(key) {
                serde_json::to_string(value)?
            } else {
                value
                    .as_str()
                    .context("Invalid preference value")?
                    .to_owned()
            };
            if key == "agent_preferences" {
                ensure!(
                    text.len() <= 256 * 1024 && value.as_object().is_some_and(|m| m.len() <= 128),
                    "Agent preference limit reached; changes were not saved."
                );
            }
            values.insert(key.clone(), text);
        }
        replace(
            path,
            ron::ser::to_string_pretty(&values, Default::default())?.as_bytes(),
            existed,
        )
    })
}

/// Writer-owned queue protected by the condition-variable mutex. A failed write is retained
/// and paused; an explicit flush retries, avoiding an idle error/write loop.
#[derive(Default)]
struct Pending {
    edits: Vec<Edit>,
    error: Option<String>,
    paused: bool,
    closed: bool,
}
struct Writer {
    state: Mutex<Pending>,
    ready: Condvar,
    wake: Arc<dyn Fn() + Send + Sync>,
}

/// Per-window preference snapshot with a background, field-merging shared-file writer.
/// `Storage::flush` queues work; dropping attempts a final flush and joins the worker.
/// Failed writes stay queued until an explicit retry or the window exits.
pub struct Preferences {
    local: Values,
    edits: Vec<Edit>,
    writer: Arc<Writer>,
    worker: Option<JoinHandle<()>>,
}
impl Preferences {
    /// Load preferences and start the writer. Malformed input is preserved and reported by
    /// [`Self::take_error`]; `wake` must be thread-safe and only schedule UI work. Initial
    /// loading may block, so call during startup.
    pub fn open(path: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let path = path.canonicalize().unwrap_or(path);
        let (local, error) = match load(&path).and_then(|v| {
            decode(&v)?;
            Ok(v)
        }) {
            Ok(values) => (values, None),
            Err(e) => (Values::new(), Some(format!("{e:#}"))),
        };
        let writer = Arc::new(Writer {
            state: Mutex::new(Pending {
                error,
                ..Default::default()
            }),
            ready: Condvar::new(),
            wake,
        });
        let shared = writer.clone();
        let worker = std::thread::spawn(move || {
            loop {
                let mut pending = shared.state.lock().unwrap();
                while (pending.edits.is_empty() || pending.paused) && !pending.closed {
                    pending = shared.ready.wait(pending).unwrap();
                }
                if pending.edits.is_empty() && pending.closed {
                    break;
                }
                let edits = std::mem::take(&mut pending.edits);
                let closing = pending.closed;
                drop(pending);
                if let Err(error) = write_edits(&path, &edits) {
                    let mut pending = shared.state.lock().unwrap();
                    let mut failed = edits;
                    failed.append(&mut pending.edits);
                    pending.edits = failed;
                    let message = format!("Preferences were not saved: {error:#}");
                    eprintln!("{message}");
                    pending.error = Some(message);
                    pending.paused = true;
                    drop(pending);
                    (shared.wake)();
                    if closing {
                        break;
                    }
                }
            }
        });
        Self {
            local,
            edits: vec![],
            writer,
            worker: Some(worker),
        }
    }
    /// Take the latest loading/writing error without discarding queued edits.
    pub fn take_error(&self) -> Option<String> {
        self.writer.state.lock().unwrap().error.take()
    }
    fn gui_snapshot(&self) -> Result<String> {
        let gui: BTreeMap<_, _> = self
            .local
            .iter()
            .filter(|(key, _)| matches!(key.as_str(), "window" | "egui"))
            .collect();
        Ok(ron::ser::to_string_pretty(&gui, Default::default())?)
    }
}
impl Storage for Preferences {
    fn get_string(&self, key: &str) -> Option<String> {
        self.local.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, text: String) {
        if self.local.get(key) == Some(&text) && !matches!(key, "color_theme" | "syntax_theme") {
            return;
        }
        if structured(key) {
            let old = self
                .local
                .get(key)
                .and_then(|v| serde_json::from_str(v).ok())
                .unwrap_or(serde_json::json!({}));
            let Ok(next) = serde_json::from_str(&text) else {
                return;
            };
            let before = match comparison_base(key, &old, &next) {
                Ok(value) => value,
                Err(error) => {
                    self.writer.state.lock().unwrap().error =
                        Some(format!("Invalid preferences: {error:#}"));
                    (self.writer.wake)();
                    return;
                }
            };
            let mut edits = vec![];
            for mut edit in changes(&before, &next) {
                // Struct fields unknown to this version must survive. Only map
                // entry eviction and explicit sandbox-override removal delete keys.
                if edit.value.is_none() {
                    let removable = match key {
                        "agent_preferences" => {
                            edit.path.len() == 1
                                || (edit.path.len() == 3 && edit.path[1] == "acp_options")
                        }
                        "extension_preferences" => {
                            edit.path.len() == 1
                                || (edit.path.len() == 3 && edit.path[1] == "settings")
                        }
                        "project_preferences" => {
                            edit.path.len() == 1
                                || (edit.path.len() == 4
                                    && edit.path[1] == "sandbox"
                                    && edit.path[2] == "overrides")
                        }
                        "sandbox_defaults" => edit.path.len() == 2 && edit.path[0] == "overrides",
                        _ => false,
                    };
                    if !removable {
                        continue;
                    }
                    // Evict only the record this window actually saw. Another
                    // window may have changed it since it fell out of this map;
                    // an unconditional deletion would erase that newer edit.
                    // Compare unknown fields too, not just our typed defaults.
                    edit.previous = if edit.path.len() == 1 && key != "sandbox_defaults" {
                        old.get(&edit.path[0]).cloned()
                    } else {
                        None
                    };
                }
                edits.push(edit);
            }
            let mut local = old;
            apply(&mut local, &edits);
            for edit in &mut edits {
                edit.path.insert(0, key.into());
            }
            let pending_bytes = self
                .writer
                .state
                .lock()
                .unwrap()
                .edits
                .iter()
                .map(edit_size)
                .sum::<usize>();
            if pending_bytes
                + self.edits.iter().map(edit_size).sum::<usize>()
                + edits.iter().map(edit_size).sum::<usize>()
                > 16 * 1024 * 1024
            {
                self.writer.state.lock().unwrap().error=Some("Too many unsaved preference changes. Fix storage and edit the setting again to retry.".into());
                (self.writer.wake)();
                return;
            }
            self.edits.extend(edits);
            self.local.insert(key.into(), local.to_string());
            return;
        } else {
            self.edits.push(Edit {
                path: vec![key.into()],
                value: Some(Value::String(text.clone())),
                previous: None,
            });
        }
        self.local.insert(key.into(), text);
    }
    fn flush(&mut self) {
        if self.edits.is_empty() {
            return;
        }
        let mut pending = self.writer.state.lock().unwrap();
        pending.edits.append(&mut self.edits);
        // Bound queued work even if storage is slow or temporarily unwritable.
        if !pending.edits.iter().any(|edit| edit.value.is_none()) {
            let mut compact: BTreeMap<Vec<String>, Edit> = BTreeMap::new();
            for edit in pending.edits.drain(..) {
                compact.retain(|path, _| !path.starts_with(&edit.path));
                compact.insert(edit.path.clone(), edit);
            }
            pending.edits = compact.into_values().collect();
        } else {
            // Preserve chronological prerequisites of conditional evictions,
            // while still replacing obsolete whole-GUI snapshots and scalars.
            let mut last = BTreeMap::new();
            for (index, edit) in pending.edits.iter().enumerate() {
                if edit.path.len() == 1 && edit.value.is_some() {
                    last.insert(edit.path.clone(), index);
                }
            }
            pending.edits = std::mem::take(&mut pending.edits)
                .into_iter()
                .enumerate()
                .filter_map(|(index, edit)| {
                    if edit.path.len() != 1
                        || edit.value.is_none()
                        || last.get(&edit.path) == Some(&index)
                    {
                        Some(edit)
                    } else {
                        None
                    }
                })
                .collect();
        }
        pending.paused = false;
        drop(pending);
        self.writer.ready.notify_one();
    }
}
impl Drop for Preferences {
    fn drop(&mut self) {
        self.flush();
        self.writer.state.lock().unwrap().closed = true;
        self.writer.ready.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// eframe's writer is isolated per process, but still restores the last GUI layout.
pub struct NativeState {
    directory: PathBuf,
    /// Private eframe persistence path; never substitute the shared preferences path.
    pub path: PathBuf,
}
impl NativeState {
    /// Seed a private, owner-only GUI snapshot from persisted layout state. Dropping the guard
    /// removes only its own directory.
    pub fn new(preferences: &Preferences) -> Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let directory = std::env::temp_dir().join(format!(
            "rode-state-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let state = Self {
            path: directory.join("app.ron"),
            directory,
        };
        replace(&state.path, preferences.gui_snapshot()?.as_bytes(), false)?;
        Ok(state)
    }
}
impl Drop for NativeState {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
