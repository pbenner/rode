//! Private atomic session records and generation-scoped owner locks. Confirmed
//! takeover replaces the generation under the record lock; old writers are
//! rejected even when their GUI and advisory lock survive indefinitely.
use super::model::*;
use anyhow::{Context, Result, ensure};
use rode_host_core::{
    locking::{self, PrivateFile},
    persistence,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::{self, File},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt},
    },
    path::{Path, PathBuf},
};

pub(super) struct Store {
    root: PathBuf,
    summaries: RefCell<HashMap<String, (FileStamp, Summary)>>,
}
#[derive(PartialEq, Eq)]
struct FileStamp {
    inode: u64,
    size: u64,
    modified: (i64, i64),
    mode: u32,
    uid: u32,
}
pub(super) struct Catalogue {
    pub rows: Vec<Row>,
    pub warnings: Vec<String>,
}
pub(super) struct Lease {
    pub key: Key,
    _file: File,
}
#[derive(Serialize, serde::Deserialize)]
struct Focus {
    generation: String,
}
impl Store {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)?;
        let metadata = fs::symlink_metadata(&root)?;
        ensure!(
            metadata.is_dir()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o077 == 0,
            "Workspace sessions need a private directory owned by this user"
        );
        Ok(Self {
            root,
            summaries: RefCell::new(HashMap::new()),
        })
    }
    fn path(&self, id: &str) -> Result<PathBuf> {
        ensure!(valid_id(id), "Invalid workspace session ID");
        Ok(self.root.join(format!("{id}.json")))
    }
    fn owner_path(&self, key: &Key) -> Result<PathBuf> {
        ensure!(
            valid_id(&key.id) && valid_id(&key.generation),
            "Invalid workspace owner generation"
        );
        Ok(self
            .root
            .join(format!("{}-{}.owner", key.id, key.generation)))
    }
    fn read<T: DeserializeOwned>(&self, path: &Path) -> Result<T> {
        let file = locking::open_private(path, PrivateFile::Existing)?;
        let mut bytes = Vec::new();
        file.take(persistence::LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= persistence::LIMIT,
            "Workspace session exceeds 8 MiB"
        );
        serde_json::from_slice(&bytes).with_context(|| format!("Cannot read {}", path.display()))
    }
    fn record(&self, id: &str) -> Result<Record> {
        let record: Record = self.read(&self.path(id)?)?;
        ensure!(
            record.format == FORMAT && record.id == id && valid_id(&record.owner.generation),
            "Unsupported or invalid workspace session"
        );
        valid_name(&record.name)?;
        record.snapshot.validate()?;
        Ok(record)
    }
    fn publish(&self, record: &Record) -> Result<()> {
        record.snapshot.validate()?;
        valid_name(&record.name)?;
        persistence::replace(&self.path(&record.id)?, &serde_json::to_vec(record)?, true)
    }
    fn lease(&self, key: Key) -> Result<Lease> {
        let file = locking::open_private(&self.owner_path(&key)?, PrivateFile::New)?;
        locking::lock(&file)?;
        Ok(Lease { key, _file: file })
    }
    fn active(&self, key: &Key) -> Result<bool> {
        let file = match locking::open_private(&self.owner_path(key)?, PrivateFile::Existing) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Ok(true)
        } else {
            Err(error.into())
        }
    }
    /// Metadata caching avoids repeatedly decoding draft payloads while a window
    /// watches for ownership changes. Every mutation still reloads under the lock.
    fn summary(&self, id: &str) -> Result<Summary> {
        let path = self.path(id)?;
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            metadata.is_file()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o077 == 0,
            "Workspace record must be private and regular"
        );
        let stamp = FileStamp {
            inode: metadata.ino(),
            size: metadata.size(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            mode: metadata.mode(),
            uid: metadata.uid(),
        };
        if let Some((cached, summary)) = self.summaries.borrow().get(id)
            && cached == &stamp
        {
            return Ok(summary.clone());
        }
        let summary: Summary = self.read(&path)?;
        ensure!(
            summary.format == FORMAT && summary.id == id && valid_id(&summary.owner.generation),
            "Unsupported workspace session {}",
            path.display()
        );
        valid_name(&summary.name)?;
        summary.snapshot.location.validate()?;
        self.summaries
            .borrow_mut()
            .insert(id.into(), (stamp, summary.clone()));
        Ok(summary)
    }
    pub fn list(&self) -> Result<Catalogue> {
        let mut rows = Vec::new();
        let mut warnings = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().is_none_or(|x| x != "json") || !valid_id(id) {
                continue;
            }
            let result = self.summary(id).and_then(|summary| {
                Ok(Row {
                    active: self.active(&summary.key())?,
                    summary,
                })
            });
            match result {
                Ok(row) => rows.push(row),
                Err(error) => warnings.push(format!("{}: {error:#}", path.display())),
            }
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.summary.updated));
        if rows.len() > 128 {
            rows.truncate(128);
            warnings.push("Only the 128 most recent workspace sessions are shown.".into());
        }
        Ok(Catalogue { rows, warnings })
    }
    pub fn create(&self, snapshot: Snapshot) -> Result<(Record, Lease)> {
        snapshot.validate()?;
        ensure!(
            self.list()?.rows.len() < 128,
            "At most 128 workspace sessions may be saved"
        );
        let id = rode_session::new_id()?;
        let generation = rode_session::new_id()?;
        let lease = self.lease(Key {
            id: id.clone(),
            generation: generation.clone(),
        })?;
        let mut name = crate::ssh::display_name(snapshot.location.path())
            .chars()
            .filter(|c| !c.is_control())
            .take(60)
            .collect::<String>();
        while name.len() > 120 {
            name.pop();
        }
        let record = Record {
            format: FORMAT,
            id,
            name: if name.trim().is_empty() {
                "Workspace".into()
            } else {
                name
            },
            revision: 1,
            updated: now(),
            owner: Owner {
                generation,
                desktop: crate::desktop::owner(),
            },
            snapshot,
        };
        persistence::transaction(&self.path(&record.id)?, |exists| {
            ensure!(!exists, "Workspace identity collision");
            self.publish(&record)
        })?;
        Ok((record, lease))
    }
    pub fn claim(&self, key: &Key, takeover: bool) -> Result<(Record, Lease)> {
        persistence::transaction(&self.path(&key.id)?, |_| {
            let mut record = self.record(&key.id)?;
            ensure!(
                record.owner.generation == key.generation,
                "Workspace ownership changed. Refresh the list and confirm again."
            );
            ensure!(
                takeover || !self.active(key)?,
                "This workspace is open in another window. Switch to it or confirm Take over."
            );
            let generation = rode_session::new_id()?;
            let lease = self.lease(Key {
                id: key.id.clone(),
                generation: generation.clone(),
            })?;
            record.owner = Owner {
                generation,
                desktop: crate::desktop::owner(),
            };
            record.revision += 1;
            record.updated = now();
            self.publish(&record)?;
            let _ = fs::remove_file(self.owner_path(key)?);
            Ok((record, lease))
        })
    }
    pub fn save(&self, lease: &Lease, snapshot: Snapshot) -> Result<Record> {
        persistence::transaction(&self.path(&lease.key.id)?, |_| {
            let mut record = self.record(&lease.key.id)?;
            ensure!(
                record.key() == lease.key,
                "Workspace session moved to another window"
            );
            ensure!(
                snapshot.location.identity() == record.snapshot.location.identity(),
                "A saved workspace cannot change its project or host implicitly"
            );
            record.snapshot = snapshot;
            record.updated = now();
            record.revision += 1;
            self.publish(&record)?;
            Ok(record)
        })
    }
    pub fn rename(&self, key: &Key, name: String) -> Result<()> {
        valid_name(&name)?;
        persistence::transaction(&self.path(&key.id)?, |_| {
            let mut record = self.record(&key.id)?;
            ensure!(
                record.key() == *key,
                "Workspace ownership changed; refresh before renaming"
            );
            record.name = name;
            record.revision += 1;
            self.publish(&record)
        })
    }
    pub fn remove(&self, key: &Key, revision: u64) -> Result<()> {
        persistence::transaction(&self.path(&key.id)?, |_| {
            let record = self.record(&key.id)?;
            ensure!(
                record.key() == *key && record.revision == revision,
                "Workspace changed; refresh before removing it"
            );
            ensure!(
                !self.active(key)?,
                "Close this workspace session before removing its saved state"
            );
            fs::remove_file(self.path(&key.id)?)?;
            self.summaries.borrow_mut().remove(&key.id);
            let _ = fs::remove_file(self.owner_path(key)?);
            Ok(())
        })
    }
    pub fn focus(&self, key: &Key) -> Result<()> {
        let record = self.record(&key.id)?;
        ensure!(
            record.key() == *key && self.active(key)?,
            "Workspace window is no longer available"
        );
        let desktop = crate::desktop::owner();
        ensure!(
            record.owner.desktop.host == desktop.host
                && record.owner.desktop.display == desktop.display,
            "This workspace belongs to a different desktop; use Take over"
        );
        persistence::replace(
            &self.root.join(format!("{}.focus", key.id)),
            &serde_json::to_vec(&Focus {
                generation: key.generation.clone(),
            })?,
            true,
        )
    }
    pub fn requested_focus(&self, lease: &Lease) -> Result<bool> {
        let path = self.root.join(format!("{}.focus", lease.key.id));
        if !path.exists() {
            return Ok(false);
        }
        let request: Focus = self.read(&path)?;
        if request.generation != lease.key.generation {
            return Ok(false);
        }
        fs::remove_file(path)?;
        Ok(true)
    }
    pub fn owns(&self, lease: &Lease) -> Result<bool> {
        Ok(self.summary(&lease.key.id)?.key() == lease.key)
    }
}

#[cfg(test)]
pub(super) mod tests;
