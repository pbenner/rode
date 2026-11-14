//! Cached directory projection and acknowledged path changes; no host I/O or editor access.
use super::keyboard::Selection;
use super::*;

/// Project-scoped cache, row projection, mutation identity and native selection.
pub(in crate::app) struct State {
    pub(super) form: Option<Form>,
    pub(super) input: String,
    pub(in crate::app) busy: bool,
    /// Directory replies captured before a mutation must not restore stale rows.
    pub(in crate::app) epoch: u64,
    pub(in crate::app) directories: HashMap<String, Vec<Entry>>,
    pub(in crate::app) tree_rows: Vec<TreeRow>,
    pub(in crate::app) tree_dirty: bool,
    pub(in crate::app) expanded: HashSet<String>,
    pub(in crate::app) loading: HashSet<String>,
    pub(in crate::app) selection: Selection,
}

impl Default for State {
    fn default() -> Self {
        Self {
            form: None,
            input: String::new(),
            busy: false,
            epoch: 0,
            directories: HashMap::new(),
            tree_rows: Vec::new(),
            tree_dirty: true,
            expanded: HashSet::new(),
            loading: HashSet::new(),
            selection: Selection::default(),
        }
    }
}

/// Acknowledged file-mutation effects for the application adapter: binding updates, refreshes,
/// optional open and a lifecycle error.
#[derive(Default)]
pub(super) struct Effects {
    pub(super) rename: Option<(Entry, String)>,
    pub(super) open: Option<String>,
    pub(super) refresh: Vec<String>,
    pub(super) status: String,
    pub(super) error: Option<String>,
}

#[derive(Clone)]
/// One flattened explorer entry with its visual indentation depth.
pub(in crate::app) struct TreeRow {
    pub(super) entry: Entry,
    pub(super) depth: usize,
}

/// Flatten expanded cached directories in display order while retaining each entry's indentation depth.
pub(super) fn flatten_tree(
    directories: &HashMap<String, Vec<Entry>>,
    expanded: &HashSet<String>,
    root: &str,
    depth: usize,
    hidden: bool,
    rows: &mut Vec<TreeRow>,
) {
    if depth > 24 {
        return;
    }
    if let Some(entries) = directories.get(root) {
        for entry in entries {
            if !hidden && entry.name.starts_with('.') {
                continue;
            }
            rows.push(TreeRow {
                entry: entry.clone(),
                depth,
            });
            if entry.dir && expanded.contains(&entry.path) {
                flatten_tree(directories, expanded, &entry.path, depth + 1, hidden, rows);
            }
        }
    }
}

impl State {
    /// Discard the project’s cached directories, expansion, selection and in-flight UI markers on
    /// context replacement.
    pub(in crate::app) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Register one listing for the current mutation epoch, deduplicating workers.
    pub(in crate::app) fn begin_list(&mut self, path: &str) -> Option<u64> {
        (!self.busy && self.loading.insert(path.to_owned())).then_some(self.epoch)
    }

    /// Cache an admitted listing under both requested and canonical paths and invalidate the
    /// flattened row projection.
    pub(in crate::app) fn loaded(&mut self, path: String, response: Response) {
        self.directories.insert(path, response.entries.clone());
        self.directories.insert(response.path, response.entries);
        self.tree_dirty = true;
    }

    /// Fence older directory replies with a new mutation epoch, clear loading markers and retain
    /// busy state until the mutation is acknowledged.
    pub(super) fn begin_mutation(&mut self) {
        self.form = None;
        self.epoch = self.epoch.wrapping_add(1);
        self.loading.clear();
        self.busy = true;
    }

    /// Rebuild flattened visible rows only when dirty, using cached directories and the shared
    /// expansion/hidden-file choices.
    pub(super) fn project(&mut self, root: &str, hidden: bool) {
        if self.tree_dirty {
            self.tree_rows.clear();
            flatten_tree(
                &self.directories,
                &self.expanded,
                root,
                0,
                hidden,
                &mut self.tree_rows,
            );
            self.tree_dirty = false;
        }
    }

    /// Mutate cached paths only after host acknowledgement; partial deletes still invalidate descendants.
    pub(super) fn finish(
        &mut self,
        change: Change,
        result: Result<Response, String>,
        root: Option<&str>,
    ) -> Effects {
        self.busy = false;
        self.epoch = self.epoch.wrapping_add(1);
        self.loading.clear();
        let mut effects = Effects::default();
        let succeeded = result.is_ok();
        let parent = if matches!(change.kind, Kind::File | Kind::Folder) {
            Some(change.entry.path.clone())
        } else {
            std::path::Path::new(&change.entry.path)
                .parent()
                .map(|path| path.to_string_lossy().into_owned())
        };
        match result {
            Ok(response) => {
                if change.kind == Kind::Rename {
                    rename_directories(&mut self.directories, &change.entry, &response.path);
                    effects.rename = Some((change.entry.clone(), response.path.clone()));
                    self.expanded = self
                        .expanded
                        .iter()
                        .map(|path| {
                            renamed(path, &change.entry, &response.path)
                                .unwrap_or_else(|| path.clone())
                        })
                        .collect();
                    if let Some(path) = &self.selection.path {
                        self.selection.path = Some(
                            renamed(path, &change.entry, &response.path)
                                .unwrap_or_else(|| path.clone()),
                        );
                    }
                } else if change.kind == Kind::Delete {
                    self.expanded.retain(|path| !within(path, &change.entry));
                    if self
                        .selection
                        .path
                        .as_ref()
                        .is_some_and(|path| within(path, &change.entry))
                    {
                        self.selection.path = None;
                    }
                } else if change.kind == Kind::File {
                    effects.open = Some(response.path.clone());
                }
                effects.status = format!("{} complete", change.kind.title());
            }
            Err(error) => {
                effects.status = format!("{} failed", change.kind.title());
                effects.error = Some(error);
            }
        }
        if change.kind == Kind::Delete {
            // Deletion can fail partway through a folder. Refresh its remaining
            // expanded nodes even on error, while keeping unrelated caches.
            self.directories
                .retain(|path, _| !within(path, &change.entry));
            if succeeded {
                for entries in self.directories.values_mut() {
                    entries.retain(|entry| !within(&entry.path, &change.entry));
                }
            }
        }
        self.tree_dirty = true;
        let mut paths: Vec<_> = self
            .expanded
            .iter()
            .filter(|path| !self.directories.contains_key(*path))
            .cloned()
            .collect();
        if let Some(parent) = parent {
            paths.push(parent);
        }
        if let Some(root) = root
            && !self.directories.contains_key(root)
        {
            paths.push(root.to_owned());
        }
        paths.sort();
        paths.dedup();
        effects.refresh = paths;
        effects
    }
}

/// Whether a path is the entry itself or a component-boundary descendant of a directory entry.
pub(super) fn within(path: &str, entry: &Entry) -> bool {
    path == entry.path || (entry.dir && std::path::Path::new(path).starts_with(&entry.path))
}

/// Rewrite an entry/descendant path after an acknowledged rename, preserving its relative suffix.
/// Unrelated paths return `None`.
pub(super) fn renamed(path: &str, entry: &Entry, destination: &str) -> Option<String> {
    if !within(path, entry) {
        return None;
    }
    let suffix = std::path::Path::new(path).strip_prefix(&entry.path).ok()?;
    if suffix.as_os_str().is_empty() {
        return Some(destination.into());
    }
    Some(
        std::path::Path::new(destination)
            .join(suffix)
            .to_string_lossy()
            .into(),
    )
}

/// A rename changes paths, not directory contents. Keep unrelated listings and
/// remap loaded descendants rather than launching a worker for every open node.
pub(super) fn rename_directories(
    directories: &mut HashMap<String, Vec<Entry>>,
    source: &Entry,
    destination: &str,
) {
    let target = Entry {
        path: destination.into(),
        name: ssh::display_name(destination).into(),
        dir: source.dir,
    };
    for (path, mut entries) in std::mem::take(directories) {
        // A previously deleted destination can still have a stale cached listing.
        if within(&path, &target) && !within(&path, source) {
            continue;
        }
        let path = renamed(&path, source, destination).unwrap_or(path);
        // A stale destination row must not acquire the same widget ID as the
        // renamed source while the parent listing is being refreshed.
        entries.retain(|entry| !within(&entry.path, &target) || within(&entry.path, source));
        let mut changed_name = false;
        for entry in &mut entries {
            if let Some(path) = renamed(&entry.path, source, destination) {
                if entry.path == source.path {
                    entry.name.clone_from(&target.name);
                    changed_name = true;
                }
                entry.path = path;
            }
        }
        if changed_name {
            entries.sort_by_key(|entry| (!entry.dir, entry.name.to_lowercase()));
        }
        directories.insert(path, entries);
    }
}
