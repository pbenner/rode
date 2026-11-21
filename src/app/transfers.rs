//! Two independent directory browsers and one serialized transfer queue. Browsing
//! never changes workspace selection. Every job captures host, policy and vault
//! work ownership; late listing/results cannot populate a different connection.
use super::*;
mod browser;
mod drag;
mod operations;
mod queue;
#[cfg(test)]
mod tests;
mod ui;
use crate::transfers::{
    Endpoint,
    copy::{self, Action as ConflictAction, Decision, Job, Progress},
};
use rode_host_protocol::transfer::Request as TransferRequest;
use std::sync::atomic::AtomicUsize;
use std::{collections::VecDeque, sync::Mutex};

/// Direction is part of a browser/drag intent, not an unchecked array index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Side {
    Local,
    Remote,
}
impl Side {
    const ALL: [Self; 2] = [Self::Local, Self::Remote];
    fn other(self) -> Self {
        match self {
            Self::Local => Self::Remote,
            Self::Remote => Self::Local,
        }
    }
    fn index(self) -> usize {
        match self {
            Self::Local => 0,
            Self::Remote => 1,
        }
    }
}
impl std::fmt::Display for Side {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Keep existing native widget/probe identities independent of this model change.
        self.index().fmt(formatter)
    }
}

/// Only a Side can select per-endpoint state; the pair retains local/remote order.
#[derive(Default)]
struct Sides<T>([T; 2]);
impl<T> Sides<T> {
    fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }
    fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.0.iter_mut()
    }
}
impl<T> std::ops::Index<Side> for Sides<T> {
    type Output = T;
    fn index(&self, side: Side) -> &T {
        &self.0[side.index()]
    }
}
impl<T> std::ops::IndexMut<Side> for Sides<T> {
    fn index_mut(&mut self, side: Side) -> &mut T {
        &mut self.0[side.index()]
    }
}

struct Pending(Arc<AtomicUsize>);
struct Listing(Arc<AtomicBool>);
impl Drop for Listing {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl Pending {
    fn new(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self(count.clone())
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Default)]
struct Pane {
    browser: file_browser::State,
    query: String,
    selected: HashSet<String>,
    list: navigation::List,
    /// Presentation stays with this pane across browsing and connection changes.
    view: file_browser::View,
    hidden: bool,
    reveal: Option<String>,
    new_folder: String,
    creating: bool,
}
impl Pane {
    fn ready(&self) -> bool {
        !self.browser.loading
            && self.browser.error.is_none()
            && !self.browser.directory.is_empty()
            && self.browser.directory == self.browser.location
    }
}
struct Task {
    job: Job,
    label: String,
    _work: vaults::Work,
}
struct Running {
    id: u64,
    generation: u64,
    job: Job,
    label: String,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<Progress>>,
}
enum Event {
    Listed {
        generation: u64,
        side: Side,
        token: u64,
        path: String,
        result: Result<Box<files::Response>, String>,
    },
    Finished {
        id: u64,
        result: Result<(), String>,
    },
    Mutated {
        id: u64,
        completed: Vec<operations::Completed>,
    },
}
struct Finished {
    job: Job,
    label: String,
    result: String,
    failed: bool,
}
/// Transient browser/queue state. Only explicit user actions launch workers.
pub(super) struct State {
    panes: Sides<Pane>,
    generation: u64,
    next: u64,
    send: mpsc::Sender<Event>,
    receive: mpsc::Receiver<Event>,
    listings: Sides<Arc<AtomicBool>>,
    listing_active: Sides<Arc<AtomicBool>>,
    queue: VecDeque<Task>,
    running: Option<Running>,
    history: VecDeque<Finished>,
    protected: Arc<Mutex<Vec<String>>>,
    error: Option<String>,
    rename: String,
    remaining: bool,
    pending: Arc<AtomicUsize>,
    refresh: Option<String>,
    file_form: Option<operations::Form>,
    file_operation: Option<operations::Running>,
}
impl Default for State {
    fn default() -> Self {
        let (send, receive) = mpsc::channel();
        Self {
            panes: Default::default(),
            generation: 0,
            next: 0,
            send,
            receive,
            listings: Default::default(),
            listing_active: Default::default(),
            queue: VecDeque::new(),
            running: None,
            history: VecDeque::new(),
            protected: Arc::default(),
            error: None,
            rename: String::new(),
            remaining: false,
            pending: Arc::default(),
            refresh: None,
            file_form: None,
            file_operation: None,
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.reset();
    }
}
impl State {
    /// Apply the same bounded queue admission to new copies and explicit retries.
    fn enqueue(&mut self, task: Task) -> bool {
        let paths = &task.job.paths;
        let path_bytes = paths.iter().map(String::len).sum::<usize>()
            + self
                .queue
                .iter()
                .flat_map(|task| &task.job.paths)
                .map(String::len)
                .sum::<usize>();
        if self.queue.len() >= 32 || paths.len() > 10_000 || path_bytes > 16 * 1024 * 1024 {
            self.error =
                Some("The transfer queue is full. Wait for queued copies to finish.".into());
            return false;
        }
        self.queue.push_back(task);
        self.error = None;
        true
    }

    /// Normal window exit waits for cancelled stream and listing workers to release their stages.
    pub fn pending(&self) -> bool {
        self.pending.load(Ordering::Acquire) > 0
    }
    /// Cancel without waiting on network/filesystem I/O on the GUI thread. Keep
    /// the running slot until completion so rapid switches never spawn copies concurrently.
    pub fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.queue.clear();
        self.history.clear();
        self.refresh = None;
        self.error = None;
        self.rename.clear();
        self.remaining = false;
        self.file_form = None;
        if let Some(operation) = &self.file_operation {
            operation.cancel.store(true, Ordering::Release);
        }
        if let Some(running) = &self.running {
            running.cancel.store(true, Ordering::Release);
        }
        for side in Side::ALL {
            let pane = &mut self.panes[side];
            self.listings[side].store(true, Ordering::Release);
            self.listings[side] = Arc::default();
            pane.browser.loading = false;
            pane.selected.clear();
        }
        self.panes[Side::Remote] = Pane {
            view: self.panes[Side::Remote].view,
            ..Default::default()
        };
    }
}

impl EditorApp {
    fn transfer_endpoint(&self, side: Side) -> Option<Endpoint> {
        if side == Side::Local {
            return Some(Endpoint::local());
        }
        self.transfer_host_endpoint()
    }
    fn transfer_list(
        &mut self,
        side: Side,
        path: String,
        create: Option<String>,
        ctx: &egui::Context,
    ) {
        let Some(endpoint) = self.transfer_endpoint(side) else {
            return;
        };
        if self.transfers.panes[side].browser.loading {
            return;
        }
        if self.transfers.listing_active[side].swap(true, Ordering::AcqRel) {
            self.transfers.panes[side].browser.needs_load = true;
            return;
        }
        let listing = Listing(self.transfers.listing_active[side].clone());
        let state = &mut self.transfers;
        state.listings[side].store(true, Ordering::Release);
        let cancel = Arc::new(AtomicBool::new(false));
        state.listings[side] = cancel.clone();
        let pane = &mut state.panes[side];
        pane.browser.token = pane.browser.token.wrapping_add(1);
        pane.browser.location = path.clone();
        pane.browser.requested = path.clone();
        pane.browser.loading = true;
        pane.browser.error = None;
        let token = pane.browser.token;
        let generation = state.generation;
        let send = state.send.clone();
        let ctx = ctx.clone();
        let pending = Pending::new(&state.pending);
        let work = self.vaults.work();
        std::thread::spawn(move || {
            let _work = work;
            let pending = pending;
            let result = (|| -> anyhow::Result<files::Response> {
                let mut client = endpoint.open(cancel)?;
                if let Some(path) = create {
                    client.request(TransferRequest::Directory { path })?;
                }
                let reply = client.request(TransferRequest::List { path: path.clone() })?;
                let entries = reply
                    .entries
                    .into_iter()
                    .map(|entry| Entry {
                        path: std::path::Path::new(&reply.path)
                            .join(&entry.name)
                            .to_string_lossy()
                            .into(),
                        name: entry.name,
                        dir: entry.directory,
                    })
                    .collect();
                Ok(files::Response {
                    path: reply.path,
                    entries,
                    ..Default::default()
                })
            })()
            .map_err(|e| format!("{e:#}"));
            drop(pending);
            drop(listing);
            let _ = send.send(Event::Listed {
                generation,
                side,
                token,
                path,
                result: result.map(Box::new),
            });
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        });
    }
    fn queue_transfer(&mut self, side: Side, paths: Vec<String>, ctx: &egui::Context) {
        let directory = self.transfers.panes[side.other()].browser.directory.clone();
        self.queue_transfer_to(side, paths, directory, ctx);
    }
    /// Buttons and drops enter the same bounded queue, with destination authority
    /// derived from a ready listing rather than from an arbitrary drag payload.
    fn queue_transfer_to(
        &mut self,
        side: Side,
        paths: Vec<String>,
        directory: String,
        ctx: &egui::Context,
    ) {
        if paths.is_empty()
            || !self.panel_enabled(Panel::Transfers)
            || self.transfers.file_operation.is_some()
        {
            return;
        }
        let target = &self.transfers.panes[side.other()];
        if !target.ready() || (side == Side::Remote && !self.transfers.panes[side].ready()) {
            self.transfers.error =
                Some("Finish browsing both folders before starting a copy.".into());
            return;
        }
        if target.browser.directory != directory
            && !target
                .browser
                .entries
                .iter()
                .any(|entry| entry.dir && entry.path == directory)
        {
            self.transfers.error =
                Some("The destination folder changed. Refresh and try again.".into());
            return;
        }
        let mut paths = paths;
        paths.sort();
        paths.dedup();
        let mut roots: Vec<String> = Vec::new();
        let mut ancestors = HashSet::<&str>::new();
        for path in &paths {
            if !std::path::Path::new(path)
                .ancestors()
                .skip(1)
                .any(|parent| {
                    parent
                        .to_str()
                        .is_some_and(|parent| ancestors.contains(parent))
                })
            {
                ancestors.insert(path);
                roots.push(path.clone());
            }
        }
        let paths = roots;
        let target = side.other();
        if directory.is_empty() {
            self.transfers.error = Some("Choose a destination folder first.".into());
            return;
        }
        let Some(from) = self.transfer_endpoint(side) else {
            return;
        };
        let Some(to) = self.transfer_endpoint(target) else {
            return;
        };
        let label = format!(
            "{} · {} selected",
            if side == Side::Local {
                "Upload"
            } else {
                "Download"
            },
            paths.len()
        );
        let job = Job {
            from,
            to,
            paths,
            directory,
            protected: self.transfers.protected.clone(),
        };
        self.transfers.enqueue(Task {
            job,
            label,
            _work: self.vaults.work(),
        });
        self.transfer_tick(ctx);
    }
    /// Refresh dirty-destination guards and drain replies even when the tab is hidden or disabled.
    pub(super) fn transfer_tick(&mut self, ctx: &egui::Context) {
        if self.dialog != Dialog::TransferFiles {
            self.transfers.file_form = None;
        }
        let mut mutations = Vec::new();
        if self.transfers.running.is_some() || !self.transfers.queue.is_empty() {
            *self.transfers.protected.lock().unwrap() = self
                .documents
                .iter()
                .filter(|doc| doc.dirty())
                .filter_map(|doc| doc.path.clone())
                .collect();
        }
        let state = &mut self.transfers;
        while let Ok(event) = state.receive.try_recv() {
            match event {
                Event::Listed {
                    generation,
                    side,
                    token,
                    path,
                    result,
                } if generation == state.generation && token == state.panes[side].browser.token => {
                    let pane = &mut state.panes[side];
                    pane.browser.loading = false;
                    if pane.browser.matches(token, &path) {
                        match result {
                            Ok(reply) => {
                                pane.browser.loaded(*reply);
                                pane.selected.clear();
                                if let Some(path) = pane.reveal.take().filter(|path| {
                                    &pane.browser.directory == path
                                        || pane
                                            .browser
                                            .entries
                                            .iter()
                                            .any(|entry| &entry.path == path)
                                }) {
                                    pane.selected.insert(path);
                                }
                                pane.list = Default::default();
                            }
                            Err(error) => pane.browser.error = Some(error),
                        }
                    }
                }
                Event::Finished { id, result }
                    if state
                        .running
                        .as_ref()
                        .is_some_and(|running| running.id == id) =>
                {
                    let running = state.running.take().unwrap();
                    // Reset keeps this slot until cleanup finishes. Its result
                    // belongs to the previous connection and must not restore
                    // history or conflict state in the new workspace.
                    if running.generation != state.generation {
                        continue;
                    }
                    let progress = running.progress.lock().unwrap();
                    let cancelled = running.cancel.load(Ordering::Acquire);
                    let failed = result.is_err() && !cancelled;
                    let message = match result {
                        Ok(()) => format!(
                            "{} files copied · {} skipped · {}",
                            progress.files,
                            progress.skipped,
                            layout::bytes(progress.bytes)
                        ),
                        Err(_) if cancelled => {
                            format!("Cancelled · {} files completed", progress.files)
                        }
                        Err(error) => error,
                    };
                    let refresh = running
                        .job
                        .to
                        .remote()
                        .map(|_| running.job.directory.clone());
                    state.history.push_front(Finished {
                        job: running.job,
                        label: running.label,
                        result: message,
                        failed,
                    });
                    state.history.truncate(12);
                    if running.generation == state.generation {
                        state.refresh = refresh;
                        for pane in state.panes.iter_mut() {
                            pane.browser.needs_load = true;
                        }
                    }
                    state.rename.clear();
                    state.remaining = false;
                }
                Event::Mutated { id, completed }
                    if state
                        .file_operation
                        .as_ref()
                        .is_some_and(|operation| operation.id == id) =>
                {
                    let operation = state.file_operation.take().unwrap();
                    mutations.push((operation, completed));
                }
                _ => {}
            }
        }
        if state.running.is_none()
            && state.file_operation.is_none()
            && let Some(task) = state.queue.pop_front()
        {
            state.next = state.next.wrapping_add(1);
            let id = state.next;
            let cancel = Arc::new(AtomicBool::new(false));
            let progress = Arc::new(Mutex::new(Progress::default()));
            let worker_cancel = cancel.clone();
            let worker_progress = progress.clone();
            let job = task.job.clone();
            let send = state.send.clone();
            let ctx = ctx.clone();
            state.running = Some(Running {
                id,
                generation: state.generation,
                job: task.job,
                label: task.label,
                cancel,
                progress,
            });
            let pending = Pending::new(&state.pending);
            std::thread::spawn(move || {
                let _work = task._work;
                let result =
                    copy::run(job, worker_cancel, worker_progress).map_err(|e| format!("{e:#}"));
                drop(pending);
                let _ = send.send(Event::Finished { id, result });
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            });
        }
        if state.running.is_some() || state.file_operation.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if let Some(directory) = state.refresh.take() {
            let root = self
                .workspace
                .as_ref()
                .map(|workspace| workspace.root.clone());
            let mut paths = vec![directory];
            if let Some(root) = root {
                paths.push(root);
            }
            paths.sort();
            paths.dedup();
            for path in paths {
                if self.explorer_state.directories.contains_key(&path)
                    && !self.explorer_state.loading.contains(&path)
                {
                    self.list(path, ctx);
                }
            }
        }
        for (operation, completed) in mutations {
            self.finish_transfer_files(operation, completed, ctx);
        }
    }
    /// Explorer entry points reuse this view and leave the project/chat/terminal roots intact.
    pub(super) fn open_transfers(
        &mut self,
        download: Option<Entry>,
        upload_to: Option<String>,
        ctx: &egui::Context,
    ) {
        if !self.panel_enabled(Panel::Transfers) {
            return;
        }
        if download.is_some() || upload_to.is_some() {
            self.bind_project_transfers();
        }
        self.reveal_panel(Panel::Transfers, ctx);
        if let Some(directory) = upload_to {
            self.transfers.panes[Side::Remote].browser.needs_load = true;
            self.transfers.panes[Side::Remote].browser.location = directory;
        }
        if let Some(entry) = download {
            let directory = if entry.dir {
                entry.path.clone()
            } else {
                std::path::Path::new(&entry.path)
                    .parent()
                    .unwrap_or(std::path::Path::new("/"))
                    .to_string_lossy()
                    .into_owned()
            };
            self.transfers.panes[Side::Remote].browser.location = directory;
            self.transfers.panes[Side::Remote].browser.needs_load = true;
            self.transfers.panes[Side::Remote].reveal = Some(entry.path);
            self.transfers.error = None;
        }
    }
}
