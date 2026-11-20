//! Bounded recursive copy planning and explicit conflict decisions. No operation
//! deletes a source or silently retries a possibly completed remote write.
use super::{Endpoint, transport::Client};
#[cfg(test)]
mod tests;
use anyhow::{Context, Result, bail, ensure};
use rode_host_protocol::transfer::{Entry, Request};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// A queued copy's source, destination and authority cannot be edited by browsing.
#[derive(Clone)]
pub(crate) struct Job {
    pub from: Endpoint,
    pub to: Endpoint,
    pub paths: Vec<String>,
    pub directory: String,
    /// Refreshed by the UI; dirty document contents themselves never leave the editor.
    pub protected: Arc<Mutex<Vec<String>>>,
}
/// User decision for one collision; Rename is a literal leaf, empty means choose a free name.
#[derive(Clone, Debug)]
pub(crate) enum Action {
    Skip,
    Replace,
    Rename(String),
}
/// The user can apply a collision choice to the rest of this batch.
pub(crate) struct Decision {
    pub action: Action,
    pub remaining: bool,
}
#[derive(Clone)]
pub(crate) struct Conflict {
    pub path: String,
    pub regular: bool,
    pub answer: mpsc::SyncSender<Decision>,
}
#[derive(Clone)]
pub(crate) struct Progress {
    pub current: String,
    pub bytes: u64,
    pub total: u64,
    pub files: usize,
    pub skipped: usize,
    pub scanning: bool,
    pub conflict: Option<Conflict>,
    pub started: Instant,
}
impl Default for Progress {
    fn default() -> Self {
        Self {
            current: String::new(),
            bytes: 0,
            total: 0,
            files: 0,
            skipped: 0,
            scanning: true,
            conflict: None,
            started: Instant::now(),
        }
    }
}
struct Item {
    path: String,
    parent: Option<String>,
    entry: Entry,
}

fn checked(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Acquire), "Transfer cancelled");
    Ok(())
}
fn leaf(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 255
            && name != "."
            && name != ".."
            && !name.contains(['/', '\0']),
        "Enter one valid file name"
    );
    Ok(())
}
fn join(directory: &str, name: &str) -> Result<String> {
    leaf(name)?;
    Ok(Path::new(directory).join(name).to_string_lossy().into())
}

/// Leave room for the copy suffix without splitting UTF-8 or exceeding NAME_MAX.
/// Preserve ordinary file extensions; a huge extension is treated as part of the name.
fn copy_name(name: &str, number: usize, directory: bool) -> String {
    let original = Path::new(name);
    let suffix = format!(" copy {number}");
    let extension = if directory {
        None
    } else {
        original.extension().and_then(|v| v.to_str())
    };
    let extension = extension.map(|v| format!(".{v}")).unwrap_or_default();
    let (stem, extension) = if extension.len() + suffix.len() < 255 {
        (
            if directory {
                name
            } else {
                original
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .unwrap_or(name)
            },
            extension,
        )
    } else {
        (name, String::new())
    };
    let mut end = stem.len().min(255 - extension.len() - suffix.len());
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}{extension}", &stem[..end])
}
fn collect(
    client: &mut Client,
    path: String,
    parent: Option<String>,
    depth: usize,
    items: &mut Vec<Item>,
    budget: &mut usize,
    cancel: &AtomicBool,
) -> Result<()> {
    checked(cancel)?;
    ensure!(
        depth <= 64 && items.len() < 100_000,
        "Transfer selection is too large or deeply nested; choose a smaller folder"
    );
    let entry = client
        .request(Request::Stat { path: path.clone() })?
        .entry
        .context("Source disappeared")?;
    ensure!(
        entry.directory || entry.regular,
        "Symbolic links and special files are not copied: {path}"
    );
    leaf(&entry.name)?;
    *budget = budget
        .checked_add(path.len() + parent.as_ref().map_or(0, String::len) + entry.name.len() + 128)
        .context("Selection overflow")?;
    ensure!(
        *budget <= 64 * 1024 * 1024,
        "Transfer selection exceeds the directory planning budget"
    );
    let directory = entry.directory;
    items.push(Item {
        path: path.clone(),
        parent,
        entry,
    });
    if directory {
        for entry in client
            .request(Request::List { path: path.clone() })?
            .entries
        {
            collect(
                client,
                join(&path, &entry.name)?,
                Some(path.clone()),
                depth + 1,
                items,
                budget,
                cancel,
            )?;
        }
    }
    Ok(())
}

struct Copier {
    source: Client,
    destination: Client,
    progress: Arc<Mutex<Progress>>,
    cancel: Arc<AtomicBool>,
    policy: Option<Action>,
    protected: Arc<Mutex<Vec<String>>>,
}
impl Copier {
    fn protect(&self, path: &str) -> Result<()> {
        ensure!(
            !self
                .protected
                .lock()
                .unwrap()
                .iter()
                .any(|dirty| Path::new(dirty) == Path::new(path)),
            "Destination has unsaved edits in the editor: {path}"
        );
        Ok(())
    }
    fn decide(&mut self, path: &str, regular: bool) -> Result<Action> {
        if let Some(action) = &self.policy {
            return Ok(action.clone());
        }
        let (send, receive) = mpsc::sync_channel(1);
        self.progress.lock().unwrap().conflict = Some(Conflict {
            path: path.into(),
            regular,
            answer: send,
        });
        let mut heartbeat = Instant::now();
        let decision = loop {
            checked(&self.cancel)?;
            match receive.recv_timeout(Duration::from_millis(100)) {
                Ok(decision) => break decision,
                Err(mpsc::RecvTimeoutError::Disconnected) => bail!("Conflict dialog closed"),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if heartbeat.elapsed() >= Duration::from_secs(10) {
                self.source.request(Request::Ping)?;
                self.destination.request(Request::Ping)?;
                heartbeat = Instant::now();
            }
        };
        self.progress.lock().unwrap().conflict = None;
        if decision.remaining {
            self.policy = Some(match &decision.action {
                Action::Rename(_) => Action::Rename(String::new()),
                action => action.clone(),
            });
        }
        Ok(decision.action)
    }
    fn target(
        &mut self,
        path: String,
        directory: bool,
    ) -> Result<Option<(String, Option<String>)>> {
        let Some(existing) = self
            .destination
            .request(Request::Stat { path: path.clone() })?
            .entry
        else {
            return Ok(Some((path, None)));
        };
        if directory && existing.directory {
            return Ok(Some((path, None)));
        }
        let action = self.decide(&path, existing.regular && !directory)?;
        match action {
            Action::Skip => Ok(None),
            Action::Replace => {
                ensure!(
                    existing.regular && !directory,
                    "Only a regular file can be replaced; skip or rename this entry"
                );
                self.protect(&path)?;
                Ok(Some((path, Some(existing.version))))
            }
            Action::Rename(name) => {
                let parent = Path::new(&path)
                    .parent()
                    .context("Missing parent")?
                    .to_string_lossy();
                if !name.is_empty() {
                    let path = join(&parent, &name)?;
                    ensure!(
                        self.destination
                            .request(Request::Stat { path: path.clone() })?
                            .entry
                            .is_none(),
                        "Renamed destination already exists"
                    );
                    return Ok(Some((path, None)));
                }
                for number in 1..=1000 {
                    let path = join(&parent, &copy_name(&existing.name, number, directory))?;
                    if self
                        .destination
                        .request(Request::Stat { path: path.clone() })?
                        .entry
                        .is_none()
                    {
                        return Ok(Some((path, None)));
                    }
                }
                bail!("Cannot find a free destination name")
            }
        }
    }
    fn file(&mut self, item: &Item, target: String, expected: Option<String>) -> Result<()> {
        self.protect(&target)?;
        self.source.open_source(
            item.path.clone(),
            item.entry.version.clone(),
            item.entry.size,
        )?;
        self.destination.request(Request::OpenWrite {
            path: target.clone(),
            expected,
        })?;
        let mut bytes = 0u64;
        loop {
            checked(&self.cancel)?;
            let block = self.source.request(Request::Read)?;
            if block.data.is_empty() {
                ensure!(
                    bytes == item.entry.size,
                    "Source length changed during transfer"
                );
                checked(&self.cancel)?;
                self.protect(&target)?;
                // An acknowledged commit is final, even if Cancel arrives immediately after it.
                self.destination.request(Request::Commit {
                    size: bytes,
                    digest: block.digest,
                    executable: item.entry.executable,
                })?;
                break;
            }
            use base64::{Engine, engine::general_purpose::STANDARD};
            ensure!(
                block.data.len() <= rode_host_protocol::transfer::BLOCK.div_ceil(3) * 4,
                "Oversized source block"
            );
            // The decoded stream bound is also enforced by the destination.
            let decoded = STANDARD.decode(&block.data)?;
            ensure!(
                decoded.len() <= rode_host_protocol::transfer::BLOCK,
                "Oversized source block"
            );
            bytes = bytes
                .checked_add(decoded.len() as u64)
                .context("Transfer size overflow")?;
            ensure!(bytes <= item.entry.size, "Source grew during transfer");
            self.destination
                .request(Request::Write { data: block.data })?;
            self.progress.lock().unwrap().bytes += decoded.len() as u64;
        }
        self.progress.lock().unwrap().files += 1;
        Ok(())
    }
}

/// Copy a complete selected batch. Failures preserve already completed files;
/// only the current unpublished file is abandoned. Retrying is always explicit.
pub(crate) fn run(job: Job, cancel: Arc<AtomicBool>, progress: Arc<Mutex<Progress>>) -> Result<()> {
    // A large directory scan can outlast the remote stream's idle lease. Open
    // the destination only after planning, before any destination mutation.
    let mut source = job.from.open(cancel.clone())?;
    let mut items = Vec::new();
    let mut budget = 0;
    for path in job.paths {
        collect(&mut source, path, None, 0, &mut items, &mut budget, &cancel)?;
    }
    let total = items
        .iter()
        .try_fold(0u64, |size, item| size.checked_add(item.entry.size))
        .context("Transfer size overflow")?;
    checked(&cancel)?;
    let mut copier = Copier {
        source,
        destination: job.to.open(cancel.clone())?,
        progress: progress.clone(),
        cancel: cancel.clone(),
        policy: None,
        protected: job.protected,
    };
    let result = (|| {
        {
            let mut progress = progress.lock().unwrap();
            progress.total = total;
            progress.scanning = false;
        }
        let mut directories: HashMap<String, Option<String>> = HashMap::new();
        let mut heartbeat = Instant::now();
        for item in items {
            checked(&cancel)?;
            // Directory-only work and skipped entries may otherwise leave the
            // source stream idle for a long time before the next file.
            if heartbeat.elapsed() >= Duration::from_secs(10) {
                copier.source.request(Request::Ping)?;
                copier.destination.request(Request::Ping)?;
                heartbeat = Instant::now();
            }
            let parent = if let Some(parent) = &item.parent {
                directories
                    .get(parent)
                    .context("Missing transfer directory")?
                    .clone()
            } else {
                Some(job.directory.clone())
            };
            let Some(parent) = parent else {
                if item.entry.directory {
                    directories.insert(item.path, None);
                } else {
                    let mut progress = progress.lock().unwrap();
                    progress.skipped += 1;
                    progress.total -= item.entry.size;
                }
                continue;
            };
            let target = join(&parent, &item.entry.name)?;
            progress.lock().unwrap().current = target.clone();
            let Some((target, expected)) = copier.target(target, item.entry.directory)? else {
                if item.entry.directory {
                    directories.insert(item.path, None);
                } else {
                    let mut progress = progress.lock().unwrap();
                    progress.skipped += 1;
                    progress.total -= item.entry.size;
                }
                continue;
            };
            if item.entry.directory {
                copier.destination.request(Request::Directory {
                    path: target.clone(),
                })?;
                directories.insert(item.path, Some(target));
            } else {
                copier.file(&item, target, expected)?;
            }
        }
        Ok(())
    })();
    // Abort does not publish. A disconnected peer still drops its session stage at EOF.
    if result.is_err() {
        let _ = copier.destination.request(Request::Abort);
    }
    progress.lock().unwrap().conflict = None;
    result
}
