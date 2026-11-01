//! Native notebook cells, bounded outputs and workspace-owned Jupyter kernels.
//!
//! Cell identities and source/output versions, rather than positions, own kernel results and
//! render caches. Structured edits serialize through ordinary protected source saves; kernel
//! cleanup follows project/interpreter/tab lifecycle and never affects persistent project
//! terminals.
mod shutdowns;
mod source;
mod ui;
use super::*;
use crate::notebook::{
    self, Cell, Notebook,
    kernel::{self, Kernel, Run},
};
use serde_json::{Value, json};
pub(super) use shutdowns::Shutdowns;

/// Decoded output-image state; a ready texture retains its shared decoded-memory lease.
enum Picture {
    Pending,
    Ready(egui::TextureHandle, Arc<crate::images::Lease>),
    Error(String),
}
#[derive(Clone)]
/// Captured cell/output or Markdown request identity used to reject stale image decodes.
enum Target {
    Output {
        uid: u64,
        version: u64,
        index: usize,
    },
    Markdown {
        uid: u64,
        token: u64,
        uri: String,
    },
}
/// Worker result addressed to a stable cell/output identity instead of its current position.
struct PictureResult {
    target: Target,
    result: Result<crate::images::Decoded, String>,
}
/// Captured image input and cancellation handle for off-thread decoding or host retrieval.
struct PictureJob {
    target: Target,
    encoded: Option<(String, String)>,
    path: String,
    cancel: Arc<AtomicBool>,
}
/// Per-cell rendering measurements and optional Markdown preview, keyed by stable cell UID.
struct CellUi {
    height: f32,
    previewing: bool,
    preview: Option<markdown::Preview>,
}
impl Default for CellUi {
    fn default() -> Self {
        Self {
            height: 150.,
            previewing: true,
            preview: None,
        }
    }
}

/// One notebook's structured model, kernel attachment and versioned render caches.
/// The shared shutdown registry outlives this view when a kernel still needs host cleanup.
pub(super) struct View {
    pub book: Notebook,
    source_mode: bool,
    active: u64,
    focus: Option<u64>,
    selection: Option<(u64, usize, usize)>,
    kernel: Option<Kernel>,
    shutdowns: Shutdowns,
    execution_target: Option<crate::slurm::Target>,
    persistent: bool,
    environment: String,
    kernel_name: String,
    status: String,
    error: String,
    cells: HashMap<u64, CellUi>,
    pictures: HashMap<(u64, u64, usize), Picture>,
    display_ids: HashMap<String, Vec<(u64, u64, usize)>>,
    pending_clear: HashMap<u64, u64>,
    images: Arc<markdown::Images>,
    tx: mpsc::Sender<PictureResult>,
    rx: mpsc::Receiver<PictureResult>,
    reveal: bool,
}
impl View {
    /// Report whether the ordinary text editor currently owns the raw notebook JSON.
    pub(in crate::app) fn source_mode(&self) -> bool {
        self.source_mode
    }
    /// Create a structured notebook view and focus its first cell; execution starts only on Run.
    pub fn new(book: Notebook, images: Arc<markdown::Images>, shutdowns: Shutdowns) -> Self {
        let active = book.cells.first().map(|c| c.uid).unwrap_or(0);
        let kernel_name = if book.language() == "python" {
            "python3".into()
        } else {
            book.kernel()
        };
        let (tx, rx) = mpsc::channel();
        Self {
            book,
            active,
            focus: Some(active),
            selection: None,
            source_mode: false,
            kernel: None,
            shutdowns,
            execution_target: None,
            persistent: false,
            environment: String::new(),
            kernel_name,
            status: "Kernel stopped · Run a cell to start".into(),
            error: String::new(),
            cells: HashMap::new(),
            pictures: HashMap::new(),
            display_ids: HashMap::new(),
            pending_clear: HashMap::new(),
            images,
            tx,
            rx,
            reveal: false,
        }
    }
    /// Resolve the stable active cell UID to its current position after insertions or reordering.
    fn active_index(&self) -> Option<usize> {
        self.book.cells.iter().position(|c| c.uid == self.active)
    }
    /// Report a selected compute allocation; this does not imply a kernel is running.
    pub(in crate::app) fn uses_slurm(&self) -> bool {
        self.execution_target.is_some()
    }
    /// Report an unacknowledged kernel lifetime operation that normal exit must drain.
    pub(in crate::app) fn lifetime_pending(&self) -> bool {
        self.kernel.as_ref().is_some_and(Kernel::pending)
    }
    /// Expose the host runtime identity once kernel creation or attachment has established it.
    pub(in crate::app) fn native_id(&self) -> Option<&str> {
        self.kernel.as_ref().and_then(Kernel::native_id)
    }
    /// Stop the current kernel before changing its captured local/Slurm execution target.
    fn set_execution_target(&mut self, target: Option<crate::slurm::Target>) {
        if self.execution_target != target {
            self.stop("Execution target changed");
            self.execution_target = target;
            self.status = "Execution target changed · Run a cell to start a new kernel".into();
            self.error.clear();
        }
    }
    /// Transfer kernel shutdown ownership to the window registry and clear display-update bindings.
    fn stop(&mut self, reason: &str) {
        if let Some(kernel) = self.kernel.take() {
            self.shutdowns.stop(kernel);
            self.status = reason.into();
        }
        self.display_ids.clear();
        self.pending_clear.clear();
    }
    /// Release this attachment, retaining persistent host computation; nonpersistent kernels
    /// are handed to the shutdown registry until termination is acknowledged.
    fn detach(&mut self, reason: &str) {
        if let Some(kernel) = self.kernel.take() {
            if !kernel.persistent() {
                self.shutdowns.stop(kernel);
            }
            self.status = reason.into();
        }
        self.display_ids.clear();
        self.pending_clear.clear();
    }
    /// Return the current raw-JSON editing mode for execution guards.
    fn source(&mut self) -> bool {
        self.source_mode
    }
    /// Serialize structured edits into the document and advance its source version only on change.
    /// On a size/serialization failure, restore the last parseable source and stop its kernel.
    fn sync(&mut self, doc: &mut Document) -> bool {
        match self.book.serialize() {
            Ok(text) => {
                if text != doc.text {
                    doc.text = text;
                    doc.modified = doc.text != doc.saved_text;
                    doc.edit_version = doc.edit_version.wrapping_add(1);
                    doc.lines = doc.text.bytes().filter(|b| *b == b'\n').count() + 1;
                }
                true
            }
            Err(error) => {
                self.error = format!("{error:#}");
                if let Ok(book) = Notebook::parse(&doc.text) {
                    self.book = book;
                    self.cells.clear();
                    self.pictures.clear();
                    self.stop("Notebook change exceeded limits");
                }
                false
            }
        }
    }
    /// Forget display-ID update destinations belonging to a removed or reset cell.
    fn drop_displays(&mut self, uid: u64) {
        self.display_ids.retain(|_, targets| {
            targets.retain(|(owner, _, _)| *owner != uid);
            !targets.is_empty()
        });
    }
    /// Release cached output textures for one cell after its output identity changes.
    fn invalidate_pictures(&mut self, uid: u64) {
        self.pictures.retain(|(owner, _, _), _| *owner != uid);
    }
    /// Drain decoded-image replies and accept only destinations whose cell/output tokens still match.
    fn pictures(&mut self, ctx: &egui::Context) {
        while let Ok(PictureResult { target, result }) = self.rx.try_recv() {
            match target {
                Target::Output {
                    uid,
                    version,
                    index,
                } => {
                    if !self
                        .book
                        .cells
                        .iter()
                        .any(|c| c.uid == uid && c.outputs_version == version)
                    {
                        continue;
                    }
                    let key = (uid, version, index);
                    if !self.pictures.contains_key(&key) {
                        continue;
                    }
                    let value = match result {
                        Ok(decoded) => Picture::Ready(
                            ctx.load_texture(
                                format!("notebook-{uid}-{version}-{index}"),
                                egui::ImageData::Color(decoded.pixels),
                                Default::default(),
                            ),
                            decoded.lease,
                        ),
                        Err(error) => Picture::Error(error),
                    };
                    self.pictures.insert(key, value);
                }
                Target::Markdown { uid, token, uri } => {
                    if let Some(preview) = self
                        .cells
                        .get_mut(&uid)
                        .and_then(|c| c.preview.as_mut())
                        .filter(|p| p.token == token)
                    {
                        preview.complete(uri, result, ctx);
                    }
                }
            }
        }
    }
}
impl Drop for View {
    fn drop(&mut self) {
        self.detach("Notebook closed");
    }
}

enum Action {
    Add(&'static str, usize),
    Delete(usize),
    Move(usize, isize),
    Kind(usize, String),
    Clear(Option<usize>),
    Run(bool),
    Interrupt,
    Shutdown,
    Kernel,
    Source,
    Preview(usize),
}

impl EditorApp {
    /// Attach an existing host kernel to an open notebook using its original project policy.
    /// Retain the host's captured interpreter and detach the previous view according to persistence.
    pub(in crate::app) fn attach_notebook_runtime(
        &mut self,
        index: usize,
        info: rode_runtime_core::service::Info,
        ctx: &egui::Context,
    ) -> anyhow::Result<()> {
        let workspace = self
            .workspace
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Open the runtime's original project"))?;
        let target = info
            .target
            .clone()
            .map(serde_json::from_value)
            .transpose()?;
        // The host keeps its captured interpreter. Track the current selector
        // separately so only a subsequent UI selection change detaches this view.
        let environment = self.terminal_python_selection();
        let persistent = info.persistent;
        let repaint = ctx.clone();
        let kernel = Kernel::attach(
            workspace.plan.clone(),
            workspace.remote.clone(),
            info,
            Arc::new(move || repaint.request_repaint()),
        )?;
        let view = self.documents[index]
            .notebook
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Open the notebook in Cells view before attaching"))?;
        view.detach("Switching kernel attachment");
        view.environment = environment;
        view.persistent = persistent;
        view.execution_target = target;
        view.kernel = Some(kernel);
        view.error.clear();
        self.active = index;
        self.focus_editor(ctx);
        Ok(())
    }
    /// Create an unsaved metadata-preserving notebook document without starting Jupyter.
    pub(super) fn new_notebook(&mut self) {
        if !self.feature(crate::extensions::Feature::Notebooks) {
            self.notify("Enable Jupyter notebooks in View → Extensions.", false);
            return;
        }
        self.new_file();
        let book = Notebook::default();
        let doc = &mut self.documents[self.active];
        doc.text = book.serialize().expect("small new notebook");
        doc.lines = doc.text.lines().count();
        doc.notebook = Some(View::new(
            book,
            self.markdown_images.clone(),
            self.notebook_shutdowns.clone(),
        ));
    }
    /// Dispose notebook attachments when context changes. Encrypted-project locking
    /// explicitly stops even persistent kernels; other changes honor their persistence.
    pub(super) fn stop_notebook_kernels(&mut self, reason: &str) {
        for doc in &mut self.documents {
            if let Some(view) = &mut doc.notebook {
                if reason == "Locking encrypted project" {
                    view.stop(reason);
                } else {
                    view.detach(reason);
                }
                for cell in view.cells.values_mut() {
                    if let Some(preview) = &mut cell.preview {
                        preview.invalidate();
                    }
                }
            }
        }
    }
    /// Stop execution before raw-JSON editing, or parse a captured source snapshot
    /// on a worker when returning to the structured Cells view.
    pub(super) fn toggle_notebook_source(&mut self, ctx: &egui::Context) {
        let Some(doc) = self.documents.get_mut(self.active) else {
            return;
        };
        if let Some(view) = &mut doc.notebook
            && !view.source_mode
        {
            view.stop("Kernel stopped for JSON editing");
            view.source_mode = true;
            self.focus_editor(ctx);
            return;
        }
        if !self.feature(crate::extensions::Feature::Notebooks) {
            return;
        }
        let doc = &self.documents[self.active];
        let id = doc.id;
        let version = doc.edit_version;
        let text = doc.text.clone();
        let generation = self.generation;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let result = Notebook::parse(&text).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::NotebookParsed {
                generation,
                id,
                version,
                result,
            });
            ctx.request_repaint();
        });
    }
    /// Install a parsed notebook only for the still-current document version;
    /// parse failures preserve the user's original JSON source.
    pub(super) fn notebook_parsed(
        &mut self,
        id: u64,
        version: u64,
        result: Result<Notebook, String>,
        ctx: &egui::Context,
    ) {
        if !self.feature(crate::extensions::Feature::Notebooks) {
            return;
        }
        let Some(doc) = self
            .documents
            .iter_mut()
            .find(|d| d.id == id && d.edit_version == version)
        else {
            return;
        };
        match result {
            Ok(book) => {
                doc.notebook = Some(View::new(
                    book,
                    self.markdown_images.clone(),
                    self.notebook_shutdowns.clone(),
                ));
                self.focus_editor(ctx);
            }
            Err(error) => self.notify(
                format!("Notebook: {error}. JSON source is preserved."),
                true,
            ),
        }
    }
    /// Validate host, source mode and execution target, then lazily start or reuse a kernel
    /// and queue the active cell or all code cells with their captured source versions.
    pub(super) fn run_notebook(&mut self, all: bool, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Notebooks) {
            return;
        }
        if self.preparing
            || self
                .workspace
                .as_ref()
                .is_some_and(|w| !w.ready || !w.connected)
        {
            self.notify("Wait for the project host and sandbox to be ready.", false);
            return;
        }
        let python = self.terminal_python_selection();
        let Some(doc) = self.documents.get_mut(self.active) else {
            return;
        };
        if doc.recovery.is_some() {
            self.notify(
                "Reconnect or Save As this disconnected notebook before running it.",
                true,
            );
            return;
        }
        let Some(view) = &mut doc.notebook else {
            self.notify("Open a Jupyter notebook first.", false);
            return;
        };
        if view.source() {
            self.notify(
                "Switch from JSON source back to Cells before running.",
                false,
            );
            return;
        }
        let index = view.active_index();
        {
            let mut changed = false;
            let mut identities = std::collections::HashSet::new();
            for cell in &mut view.book.cells {
                let id = cell.data["id"].as_str().unwrap_or_default();
                if id.is_empty() || !identities.insert(id.to_owned()) {
                    let mut replacement = format!("rode-{}-{}", doc.id, cell.uid);
                    while !identities.insert(replacement.clone()) {
                        replacement.push('x');
                    }
                    cell.data["id"] = json!(replacement);
                    cell.version += 1;
                    changed = true;
                }
            }
            if changed {
                match view.book.serialize() {
                    Ok(text) => {
                        doc.text = text;
                        doc.modified = true;
                        doc.edit_version += 1;
                    }
                    Err(error) => {
                        view.error = error.to_string();
                        return;
                    }
                }
            }
        }
        if !all
            && let Some(index) = index
            && view.book.cells[index].kind() != "code"
        {
            view.cells.entry(view.active).or_default().previewing = true;
            return;
        }
        let runs: Vec<_> = view
            .book
            .cells
            .iter()
            .enumerate()
            .filter(|(i, c)| c.kind() == "code" && (all || Some(*i) == index))
            .map(|(_, c)| Run {
                cell: c.data["id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("uid:{}", c.uid)),
                uid: c.uid,
                version: c.version,
                code: c.source.clone(),
            })
            .collect();
        if runs.is_empty() {
            return;
        }
        if view.kernel.as_ref().is_some_and(Kernel::busy) {
            view.error = "A notebook run is already in progress.".into();
            return;
        }
        if view.kernel.is_none() {
            let (plan, remote) = self
                .workspace
                .as_ref()
                .map(|w| (w.plan.clone(), w.remote.clone()))
                .unwrap_or_else(|| {
                    let root = doc
                        .path
                        .as_ref()
                        .and_then(|p| {
                            PathBuf::from(p)
                                .parent()
                                .map(|p| p.to_string_lossy().into_owned())
                        })
                        .unwrap_or_else(|| {
                            std::env::current_dir()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into()
                        });
                    (
                        sandbox::Plan {
                            root,
                            ..Default::default()
                        },
                        None,
                    )
                });
            let context = ctx.clone();
            if view.persistent && doc.path.is_none() {
                view.error = "Save this notebook before enabling persistent execution".into();
                return;
            }
            let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || context.request_repaint());
            let started = Kernel::start_managed(
                plan,
                remote,
                python.clone(),
                view.kernel_name.clone(),
                view.execution_target.clone(),
                doc.path.clone().unwrap_or_default(),
                view.persistent,
                wake,
            );
            match started {
                Ok(kernel) => {
                    view.kernel = Some(kernel);
                    view.environment = python;
                }
                Err(error) => {
                    view.error = format!("{error:#}");
                    return;
                }
            }
        }
        match view.kernel.as_mut().unwrap().run(runs.clone()) {
            Ok(()) => {
                for run in runs {
                    view.pending_clear.insert(run.uid, run.version);
                }
                view.error.clear();
            }
            Err(error) => view.error = format!("{error:#}"),
        }
    }
    /// Drain kernel events and window-owned shutdowns without blocking. Apply outputs
    /// only to matching cell versions, serialize accepted changes and report cleanup failures.
    pub(super) fn poll_notebooks(&mut self, ctx: &egui::Context) {
        for error in self.notebook_shutdowns.poll() {
            self.abort_session_quit(error, ctx);
        }
        if self.notebook_shutdowns.pending() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let python = self.terminal_python_selection();
        let enabled = self.feature(crate::extensions::Feature::Notebooks);
        let offline = self.workspace.as_ref().is_some_and(|w| !w.connected);
        let mut failure = None;
        for doc in &mut self.documents {
            let Some(mut view) = doc.notebook.take() else {
                continue;
            };
            let previous_error = view.error.clone();
            if !enabled || offline || (view.kernel.is_some() && view.environment != python) {
                view.detach(if offline {
                    "SSH disconnected"
                } else {
                    "Notebook environment changed"
                });
            }
            if let Some(kernel) = &mut view.kernel {
                if let Some(path) = &doc.path {
                    kernel.bind_document(path);
                }
                let versions: Vec<_> = view
                    .book
                    .cells
                    .iter()
                    .map(|cell| (cell.uid, cell.version))
                    .collect();
                if kernel.needs_bind(&versions) {
                    kernel.bind(
                        view.book
                            .cells
                            .iter()
                            .map(|cell| Run {
                                uid: cell.uid,
                                version: cell.version,
                                cell: cell.data["id"]
                                    .as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| format!("uid:{}", cell.uid)),
                                code: cell.source.clone(),
                            })
                            .collect(),
                    );
                }
                kernel.retain_queued(|run| {
                    view.book
                        .cells
                        .iter()
                        .any(|c| c.uid == run.uid && c.version == run.version)
                });
            }
            let events = view.kernel.as_mut().map(Kernel::poll).unwrap_or_default();
            let mut changed = false;
            for event in events {
                let result = match event {
                    kernel::Event::Ready(info) => {
                        if let Some(info) = info.as_object() {
                            if !view.book.data["metadata"]["language_info"].is_object() {
                                view.book.data["metadata"]["language_info"] = json!({});
                            }
                            for (key, value) in info {
                                view.book.data["metadata"]["language_info"][key] = value.clone();
                            }
                            changed = true;
                        }
                        Ok(false)
                    }
                    kernel::Event::Output {
                        run,
                        output,
                        update,
                        display_id,
                    } => {
                        if !view
                            .book
                            .cells
                            .iter()
                            .any(|c| c.uid == run.uid && c.version == run.version)
                        {
                            continue;
                        }
                        if update {
                            let targets = display_id
                                .as_ref()
                                .and_then(|id| view.display_ids.get(id))
                                .cloned()
                                .unwrap_or_default();
                            for (uid, version, index) in targets {
                                if let Some(cell) = view
                                    .book
                                    .cells
                                    .iter_mut()
                                    .find(|c| c.uid == uid && c.version == version)
                                    && let Some(old) = cell.data["outputs"]
                                        .as_array_mut()
                                        .and_then(|o| o.get_mut(index))
                                {
                                    *old = output.clone();
                                    cell.outputs_version = cell.outputs_version.wrapping_add(1);
                                    view.invalidate_pictures(uid);
                                    changed = true;
                                }
                            }
                            Ok(false)
                        } else {
                            let mut index = view
                                .book
                                .cells
                                .iter()
                                .find(|c| c.uid == run.uid)
                                .map(|c| c.outputs().len())
                                .unwrap_or(0);
                            let clear = view.pending_clear.get(&run.uid) == Some(&run.version);
                            if clear {
                                view.pending_clear.remove(&run.uid);
                                view.drop_displays(run.uid);
                                index = 0;
                            }
                            let result = view.book.apply_output(
                                run.uid,
                                run.version,
                                Some(output),
                                None,
                                clear,
                            );
                            if matches!(&result, Ok(true))
                                && let Some(id) = display_id
                            {
                                view.display_ids.entry(id).or_default().push((
                                    run.uid,
                                    run.version,
                                    index,
                                ));
                            }
                            view.invalidate_pictures(run.uid);
                            result
                        }
                    }
                    kernel::Event::Clear(run) => {
                        if !view
                            .book
                            .cells
                            .iter()
                            .any(|c| c.uid == run.uid && c.version == run.version)
                        {
                            continue;
                        }
                        view.drop_displays(run.uid);
                        view.pending_clear.remove(&run.uid);
                        view.invalidate_pictures(run.uid);
                        view.book
                            .apply_output(run.uid, run.version, None, None, true)
                    }
                    kernel::Event::Count(run, count) => {
                        let clear = view.pending_clear.get(&run.uid) == Some(&run.version);
                        if clear {
                            view.pending_clear.remove(&run.uid);
                            view.invalidate_pictures(run.uid);
                            view.drop_displays(run.uid);
                        }
                        view.book
                            .apply_output(run.uid, run.version, None, Some(count), clear)
                    }
                    kernel::Event::Done(run, status) => {
                        view.status = if view
                            .book
                            .cells
                            .iter()
                            .any(|c| c.uid == run.uid && c.version == run.version)
                        {
                            format!("Cell finished: {status}")
                        } else {
                            "Cell changed during execution; stale outputs were skipped".into()
                        };
                        Ok(false)
                    }
                    kernel::Event::Error(error) => {
                        view.error = error;
                        view.detach("Kernel view ended · Reattach persistent work from Sessions");
                        Ok(false)
                    }
                };
                match result {
                    Ok(value) => changed |= value,
                    Err(error) => {
                        view.error = format!("{error:#}");
                        view.stop("Kernel stopped after an output limit");
                    }
                }
            }
            if changed {
                view.sync(doc);
            }
            if view.kernel.as_ref().is_some_and(Kernel::busy) {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            if !view.error.is_empty() && view.error != previous_error {
                failure = Some(view.error.clone());
            }
            doc.notebook = Some(view);
        }
        if self.session_exit_pending()
            && let Some(error) = failure
        {
            self.abort_session_quit(error, ctx);
        }
    }
    fn notebook_picture_jobs(&self, view: &View, jobs: Vec<PictureJob>, ctx: &egui::Context) {
        if jobs.is_empty() {
            return;
        }
        let plan = self.workspace.as_ref().map(|w| w.plan.clone());
        let remote = self.workspace.as_ref().and_then(|w| w.remote.clone());
        let disconnected = self.documents[self.active].recovery.is_some();
        let budget = self.image_budget.clone();
        let tx = view.tx.clone();
        let ctx = ctx.clone();
        let texture_limit = ctx.input(|i| i.max_texture_side);
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            for job in jobs {
                if job.cancel.load(Ordering::Acquire) {
                    continue;
                }
                let _io = budget.io.lock().unwrap_or_else(|e| e.into_inner());
                let result = (|| -> anyhow::Result<crate::images::Decoded> {
                    let (encoded, path) = if let Some(pair) = job.encoded {
                        pair
                    } else {
                        anyhow::ensure!(
                            !disconnected,
                            "Reconnect or Save As this notebook to load external images."
                        );
                        let request = Request::ReadImage {
                            path: job.path.clone(),
                        };
                        let response = match &remote {
                            Some(r) => r.request_in(&request, plan.as_ref()),
                            None => match &plan {
                                Some(p) => p.local_request(&request),
                                None => files::local(&request),
                            },
                        }?;
                        (
                            response
                                .image
                                .ok_or_else(|| anyhow::anyhow!("No notebook image data"))?,
                            job.path,
                        )
                    };
                    crate::images::decode(&encoded, &path, &budget, texture_limit)
                })()
                .map_err(|e| format!("{e:#}"));
                if !job.cancel.load(Ordering::Acquire) {
                    let _ = tx.send(PictureResult {
                        target: job.target,
                        result,
                    });
                    ctx.request_repaint();
                }
            }
        });
    }
}
#[cfg(test)]
mod tests;
