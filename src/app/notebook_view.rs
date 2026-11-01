//! Native notebook cells, bounded outputs and workspace-owned Jupyter kernels.
//!
//! Cell identities and source/output versions, rather than positions, own kernel results and
//! render caches. Structured edits serialize through ordinary protected source saves; kernel
//! cleanup follows project/interpreter/tab lifecycle and never affects persistent project
//! terminals.
mod shutdowns;
use super::*;
use crate::notebook::{
    self, Cell, Notebook,
    kernel::{self, Kernel, Run},
};
use serde_json::{Value, json};
pub(super) use shutdowns::Shutdowns;

enum Picture {
    Pending,
    Ready(egui::TextureHandle, Arc<crate::images::Lease>),
    Error(String),
}
#[derive(Clone)]
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
struct PictureResult {
    target: Target,
    result: Result<crate::images::Decoded, String>,
}
struct PictureJob {
    target: Target,
    encoded: Option<(String, String)>,
    path: String,
    cancel: Arc<AtomicBool>,
}
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
    pub(in crate::app) fn source_mode(&self) -> bool {
        self.source_mode
    }
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
    fn active_index(&self) -> Option<usize> {
        self.book.cells.iter().position(|c| c.uid == self.active)
    }
    pub(in crate::app) fn uses_slurm(&self) -> bool {
        self.execution_target.is_some()
    }
    pub(in crate::app) fn lifetime_pending(&self) -> bool {
        self.kernel.as_ref().is_some_and(Kernel::pending)
    }
    pub(in crate::app) fn native_id(&self) -> Option<&str> {
        self.kernel.as_ref().and_then(Kernel::native_id)
    }
    fn set_execution_target(&mut self, target: Option<crate::slurm::Target>) {
        if self.execution_target != target {
            self.stop("Execution target changed");
            self.execution_target = target;
            self.status = "Execution target changed · Run a cell to start a new kernel".into();
            self.error.clear();
        }
    }
    fn stop(&mut self, reason: &str) {
        if let Some(kernel) = self.kernel.take() {
            self.shutdowns.stop(kernel);
            self.status = reason.into();
        }
        self.display_ids.clear();
        self.pending_clear.clear();
    }
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
    fn source(&mut self) -> bool {
        self.source_mode
    }
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
    fn drop_displays(&mut self, uid: u64) {
        self.display_ids.retain(|_, targets| {
            targets.retain(|(owner, _, _)| *owner != uid);
            !targets.is_empty()
        });
    }
    fn invalidate_pictures(&mut self, uid: u64) {
        self.pictures.retain(|(owner, _, _), _| *owner != uid);
    }
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
    pub(super) fn notebook_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        let index = self.active;
        let Some(mut view) = self
            .documents
            .get_mut(index)
            .and_then(|d| d.notebook.take())
        else {
            return false;
        };
        if self.python_environments().is_none() && self.python_environment_error().is_empty() {
            self.refresh_environment(false, ctx);
        }
        view.pictures(ctx);
        let palette = self.themes.active().palette;
        let font_size = self.font_size;
        let mut action = None;
        let mut jobs = Vec::new();
        let mut changed = false;
        let mut link = None;
        let focus = if view.source_mode {
            false
        } else {
            self.take_editor_focus(ui, ctx).is_some()
        };
        if focus {
            view.focus = Some(view.active);
        }
        let id = self.documents[index].id;
        let path = self.documents[index]
            .path
            .clone()
            .unwrap_or_else(|| self.join_root("untitled.ipynb"));
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(12, 5))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(palette.tag("JUPYTER"));
                    if ui.selectable_label(!view.source_mode, "Cells").clicked() && view.source_mode
                    {
                        action = Some(Action::Source);
                    }
                    if ui
                        .selectable_label(view.source_mode, "JSON source")
                        .on_hover_text(
                            "Stops this notebook's kernel before editing its JSON source",
                        )
                        .clicked()
                        && !view.source_mode
                    {
                        action = Some(Action::Source);
                    }
                    if view.source_mode {
                        return;
                    }
                    let busy = view.kernel.as_ref().is_some_and(Kernel::busy);
                    if ui
                        .add_enabled(!busy, IconButton::new(Icon::Play, "Run cell"))
                        .clicked()
                    {
                        action = Some(Action::Run(false));
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Run all"))
                        .clicked()
                    {
                        action = Some(Action::Run(true));
                    }
                    if ui
                        .add_enabled(view.kernel.is_some(), egui::Button::new("Interrupt"))
                        .clicked()
                    {
                        action = Some(Action::Interrupt);
                    }
                    if ui
                        .add_enabled(view.kernel.is_some(), egui::Button::new("Shut down"))
                        .clicked()
                    {
                        action = Some(Action::Shutdown);
                    }
                    if ui.button("+ Code").clicked() {
                        action = Some(Action::Add("code", view.book.cells.len()));
                    }
                    if ui.button("+ Markdown").clicked() {
                        action = Some(Action::Add("markdown", view.book.cells.len()));
                    }
                    if ui.button("Clear outputs").clicked() {
                        action = Some(Action::Clear(None));
                    }
                });
                if !view.source_mode {
                    ui.horizontal_wrapped(|ui| {
                        let on = view.kernel.as_ref().map_or(view.persistent, Kernel::persistent);
                        if ui.add_enabled(
                            view.kernel.as_ref().is_none_or(|k| !k.pending()),
                            egui::Button::new(if on { "Persist kernel: on" } else { "Persist kernel: off" }),
                        ).on_hover_text("Keep the kernel, queued cells and outputs on the project host after closing this notebook, exiting RODE or losing SSH. Reattach from Sessions with the same project and sandbox.").clicked() {
                            if !on && self.documents[index].path.is_none() {
                                view.error = "Save this notebook before enabling persistence".into();
                            } else if let Some(kernel) = &mut view.kernel {
                                match kernel.set_persistent(!on) {
                                    Ok(()) => view.persistent = !on,
                                    Err(error) => view.error = error.to_string(),
                                }
                            } else {
                                view.persistent = !on;
                            }
                        }
                        if ui.small_button("Running kernels…").clicked() {
                            self.open_sessions(ctx);
                        }
                        if view.kernel.is_none() && ui.small_button("Reattach kernel").clicked() {
                            if let Some(path) = &self.documents[index].path {
                                self.find_notebook_runtime(path.clone(), ctx);
                            } else {
                                view.error = "Save this notebook before reattaching a kernel".into();
                            }
                        }
                        if view.kernel.as_ref().is_some_and(Kernel::persistent)
                            && ui.add_enabled(
                                view.kernel.as_ref().is_none_or(|k| !k.pending()),
                                egui::Button::new("Detach kernel").small(),
                            ).clicked()
                        {
                            view.detach("Kernel detached · Reattach from Sessions");
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Execution target");
                        let mut selected = view.execution_target.clone();
                        let (queue, _) = self.slurm_programs();
                        let runner = self.workbench.slurm_allocation().runner.clone();
                        egui::ComboBox::from_id_salt(("notebook-execution", id))
                            .selected_text(
                                selected.as_ref()
                                    .map(|t| format!("Slurm job {}", t.id))
                                    .unwrap_or_else(|| "Project host".into()),
                            )
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut selected, None, "Project host");
                                if let Some(snapshot) = &self.sessions.slurm {
                                    for job in &snapshot.jobs {
                                        if job.state == "RUNNING"
                                            && job.id.bytes().all(|b| b.is_ascii_digit())
                                        {
                                            let target = crate::slurm::Target {
                                                id: job.id.clone(),
                                                name: job.name.clone(),
                                                queue: queue.clone(),
                                                runner: runner.clone(),
                                            };
                                            ui.selectable_value(
                                                &mut selected, Some(target),
                                                format!("Job {} · {} · {}", job.id, job.nodes, job.name),
                                            );
                                        }
                                    }
                                }
                            }).response.on_hover_text(
                                "Changing execution target stops this kernel and clears its live variables. Run a cell to start a new kernel. Slurm kernels share the selected allocation's resources."
                            );
                        view.set_execution_target(selected);
                        if ui.add_enabled(
                            !self.sessions.slurm_busy,
                            egui::Button::new("Refresh Slurm targets"),
                        ).clicked() {
                            self.refresh_slurm(None, ctx);
                        }
                        if ui.small_button("Allocate…").clicked() {
                            self.open_slurm(ctx);
                        }
                        if let Some(kernel) = &view.kernel {
                            ui.small(&kernel.execution);
                        }
                    });
                    if !self.sessions.slurm_error.is_empty() {
                        ui.colored_label(palette.muted, &self.sessions.slurm_error);
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Python environment");
                        let previous = self.terminal_python_selection();
                        ui.push_id(("notebook-python", id), |ui| self.python_environment_picker(ui, ctx));
                        if previous != self.terminal_python_selection() {
                            view.detach("Notebook environment changed");
                            view.kernel_name = "python3".into();
                            action = Some(Action::Kernel);
                        }
                        if ui.add_enabled(!self.python_environment_busy(), egui::Button::new("Refresh kernels")).clicked() {
                            self.refresh_environment(false, ctx);
                        }
                        if self.python_environment_busy() { ui.spinner(); }
                    });
                    if !self.python_environment_error().is_empty() {
                        ui.colored_label(palette.error, self.python_environment_error());
                    }
                    if let Some(env) = self.python_environments() {
                        ui.small(&env.executable);
                        if !env.kernel_error.is_empty() { ui.colored_label(palette.muted, &env.kernel_error); }
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Kernel");
                        let previous = view.kernel_name.clone();
                        egui::ComboBox::from_id_salt(("notebook-kernel", id))
                            .width(300.)
                            .selected_text(if view.kernel_name == "python3" {
                                "Python · selected environment".into()
                            } else {
                                self.python_environments()
                                    .and_then(|env| env.kernels.iter().find(|k| k.name == view.kernel_name))
                                    .map(|k| k.display_name.clone())
                                    .unwrap_or_else(|| view.kernel_name.clone())
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut view.kernel_name, "python3".into(), "Python · selected environment");
                                if let Some(env) = self.python_environments() {
                                    for kernel in &env.kernels {
                                        if kernel.name != "python3" {
                                            ui.selectable_value(&mut view.kernel_name, kernel.name.clone(), format!("{} ({})", kernel.display_name, kernel.name));
                                        }
                                    }
                                }
                            });
                        if previous != view.kernel_name { action = Some(Action::Kernel); }
                        ui.label(
                            palette.tag(
                                view.kernel
                                    .as_ref()
                                    .map(|k| k.status.as_str())
                                    .unwrap_or(&view.status),
                            ),
                        );
                        if let Some(kernel) = &view.kernel {
                            ui.small(&kernel.python);
                        }
                    });
                    if self.show_find {
                        ui.horizontal(|ui| {
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.find)
                                    .hint_text("Find in cell sources…")
                                    .desired_width(220.),
                            );
                            if self.find_focus {
                                response.request_focus();
                                self.find_focus = false;
                            }
                            if (ui.small_button("Next").clicked()
                                || (response.has_focus() || response.lost_focus())
                                    && ui.input(|i| i.key_pressed(Key::Enter)))
                                && !self.find.is_empty()
                            {
                                let start = view.active_index().unwrap_or(0);
                                if let Some((at, range)) = (0..view.book.cells.len())
                                    .map(|n| (start + n + 1) % view.book.cells.len())
                                    .find_map(|at| {
                                        find_matches(
                                            &view.book.cells[at].source,
                                            &self.find,
                                            self.case_sensitive,
                                        )
                                        .first()
                                        .copied()
                                        .map(|r| (at, r))
                                    })
                                {
                                    let cell = &view.book.cells[at];
                                    let a = cell.source[..range.0].chars().count();
                                    let b = cell.source[..range.1].chars().count();
                                    view.active = cell.uid;
                                    view.selection = Some((cell.uid, a, b));
                                    view.focus = Some(cell.uid);
                                    view.reveal = true;
                                    view.cells.entry(cell.uid).or_default().previewing = false;
                                }
                            }
                            if ui.small_button("Close").clicked() {
                                self.show_find = false;
                            }
                        });
                    }
                    if !view.error.is_empty() {
                        ui.colored_label(palette.error, &view.error);
                    }
                    if !view.status.is_empty() && view.kernel.is_some() {
                        ui.small(&view.status);
                    }
                }
            });
        if !view.source_mode {
            let mut visible = HashSet::new();
            let scroll_offset = if view.reveal {
                view.reveal = false;
                view.active_index().map(|at| {
                    view.book.cells[..at]
                        .iter()
                        .map(|c| view.cells.get(&c.uid).map(|c| c.height).unwrap_or(150.))
                        .sum::<f32>()
                })
            } else {
                None
            };
            let mut scroll = egui::ScrollArea::vertical()
                .id_salt(("notebook", id))
                .auto_shrink([false, false]);
            if let Some(y) = scroll_offset {
                scroll = scroll.vertical_scroll_offset(y).animated(false);
            }
            let themes = &self.themes;
            let focus_cell = view.focus;
            let selection = view.selection;
            scroll.show_viewport(ui,|ui,viewport|{
                ui.spacing_mut().item_spacing.y=0.;let mut y=0.;
                let cell_count=view.book.cells.len();
                for at in 0..cell_count {
                    let uid=view.book.cells[at].uid;let height=view.cells.entry(uid).or_default().height;
                    if y+height<viewport.top()-60. || y>viewport.bottom()+60. {ui.add_space(height);y+=height;continue;}
                    visible.insert(uid);let top=ui.cursor().top();let active=view.active==uid;
                    let cell=&mut view.book.cells[at];let state=view.cells.get_mut(&uid).unwrap();
                    let frame=egui::Frame::new().fill(if active {palette.raised} else {palette.panel}).stroke(Stroke::new(1_f32,if active {palette.accent} else {palette.border})).inner_margin(8.).outer_margin(egui::Margin::symmetric(10,5));
                    frame.show(ui,|ui|{ui.set_min_width(ui.available_width());
                        ui.push_id(("notebook-cell",id,uid),|ui|{
                            ui.horizontal_wrapped(|ui|{
                                if ui.selectable_label(active,format!("Cell {}",at+1)).clicked(){view.active=uid;view.focus=Some(uid);}
                                let mut kind=cell.kind().to_owned();egui::ComboBox::from_id_salt("kind").width(96.).selected_text(&kind).show_ui(ui,|ui|{for value in ["code","markdown","raw"]{ui.selectable_value(&mut kind,value.into(),value);}});
                                if kind!=cell.kind(){action=Some(Action::Kind(at,kind));}
                                if cell.kind()=="code" {
                                    let running=view.kernel.as_ref().is_some_and(|k|k.running_uid()==Some(uid));
                                    ui.label(palette.tag(if running {"[*]".into()} else {format!("[{}]",cell.data["execution_count"].as_u64().map(|n|n.to_string()).unwrap_or_default())}));
                                    if ui.add_enabled(!view.kernel.as_ref().is_some_and(Kernel::busy),icons::IconButton::icon(Icon::Play,"Run cell")).clicked(){view.active=uid;action=Some(Action::Run(false));}
                                } else if cell.kind()=="markdown" && ui.small_button(if state.previewing {"Edit"} else {"Preview"}).clicked(){state.previewing = !state.previewing;view.active=uid;view.focus=Some(uid);}
                                if ui.add_enabled(at>0,egui::Button::new("Up").small()).clicked(){action=Some(Action::Move(at,-1));}
                                if ui.add_enabled(at+1<cell_count,egui::Button::new("Down").small()).clicked(){action=Some(Action::Move(at,1));}
                                if ui.small_button("Delete").clicked(){action=Some(Action::Delete(at));}
                                if cell.kind()=="code"&&ui.small_button("Clear").clicked(){action=Some(Action::Clear(Some(at)));}
                            });
                            if cell.kind()=="markdown"&&state.previewing {
                                let preview=state.preview.get_or_insert_with(||markdown::Preview::new(view.images.clone()));
                                let mut embedded=HashMap::new();let mut attachment_data=HashMap::new();
                                if let Some(attachments)=cell.data["attachments"].as_object() {for (name,bundle) in attachments {if let Some((data,path))=image_bundle(bundle) {let key=format!("attachment:{name}");embedded.insert(key.clone(),key.clone());attachment_data.insert(key,(data,path));}}}
                                let requests=preview.prepare_embedded(cell.version,&path,&cell.source,&embedded);let token=preview.token;let cancel=preview.cancelled();
                                for (uri,target) in requests {jobs.push(PictureJob {target:Target::Markdown {uid,token,uri},encoded:attachment_data.get(&target).cloned(),path:target,cancel:cancel.clone()});}
                                link=preview.inline_ui(ui,font_size,palette).or(link.take());
                            } else {
                                let before=cell.source.clone();let lang=if cell.kind()=="code" {view.book.data["metadata"]["language_info"]["name"].as_str().unwrap_or("python")} else if cell.kind()=="markdown" {"Markdown"} else {"text"};
                                let lang=crate::languages::all().iter().find(|l|l.id.eq_ignore_ascii_case(lang)).map(|l|l.syntax.as_str()).unwrap_or(lang);
                                let mut layouter=|ui:&egui::Ui,text:&dyn egui::TextBuffer,wrap:f32|{let mut job=if text.as_str().len()<128*1024 {themes.highlight(ui,text.as_str(),lang,font_size)} else {egui::text::LayoutJob::simple(text.as_str().into(),FontId::monospace(font_size),palette.text,wrap)};job.wrap.max_width=wrap;ui.fonts_mut(|f|f.layout_job(job))};
                                let edit_id=egui::Id::new(("notebook-source",id,uid));
                                if let Some((owner,a,b))=selection && owner==uid {let mut state=egui::TextEdit::load_state(ctx,edit_id).unwrap_or_default();state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(a),egui::text::CCursor::new(b))));state.store(ctx,edit_id);view.selection=None;}
                                if ctx.memory(|m|m.has_focus(edit_id)) && ui.input_mut(|i|i.consume_key(egui::Modifiers::SHIFT,Key::Enter)) {action=Some(if cell.kind()=="markdown" {Action::Preview(at)} else {Action::Run(false)});}
                                egui::ScrollArea::vertical().id_salt("cell-source-scroll").max_height(320.).show(ui,|ui|{
                                    let response=egui::TextEdit::multiline(&mut cell.source).id(edit_id).code_editor().font(FontId::monospace(font_size)).desired_width(f32::INFINITY).desired_rows(before.lines().count().clamp(3,18)).char_limit(notebook::MAX_SOURCE).layouter(&mut layouter).show(ui);
                                    if focus_cell==Some(uid) {response.response.request_focus();view.focus=None;}
                                    if response.response.has_focus(){view.active=uid;}
                                });
                                if cell.source!=before {
                                    if cell.source.len()>notebook::MAX_SOURCE {cell.source=before;view.error="Cell source exceeds 256 KiB.".into();} else {let source=cell.source.clone();cell.set_source(source);changed=true;}
                                }
                            }
                            if cell.kind()=="code" {
                                for (output_index,output) in cell.outputs().iter().take(notebook::MAX_OUTPUTS).enumerate() {
                                    ui.add_space(6.);
                                    if output["data"].get("image/png").is_some() || output["data"].get("image/jpeg").is_some() {
                                        let key=(uid,cell.outputs_version,output_index);
                                        if !view.pictures.contains_key(&key)&&view.pictures.len()<16 {
                                            let Some((data,image_path))=image_bundle(&output["data"]) else {ui.label("Image output exceeds its preview limit or has invalid data; the original is preserved.");continue;};
                                            view.pictures.insert(key,Picture::Pending);jobs.push(PictureJob {target:Target::Output {uid,version:cell.outputs_version,index:output_index},encoded:Some((data,image_path)),path:String::new(),cancel:Arc::new(AtomicBool::new(false))});
                                        }
                                        match view.pictures.get(&key) {Some(Picture::Ready(texture,_lease))=>{ui.add(egui::Image::new(texture).fit_to_original_size(1.).max_width(ui.available_width()).max_height(600.));},Some(Picture::Error(error))=>{ui.colored_label(palette.error,error);},Some(Picture::Pending)=>{ui.label("Loading image output…");},None=>{ui.label("Only 16 output images are previewed at once; all data is preserved.");}}
                                    } else {
                                        let text=output_text(output);let color=if output["output_type"]=="error"||output["name"]=="stderr" {palette.error} else {palette.text};
                                        ui.add(egui::Label::new(RichText::new(text).monospace().color(color)).wrap().selectable(true));
                                    }
                                }
                                if cell.outputs().len()>notebook::MAX_OUTPUTS {ui.small("Only the first 256 saved outputs are displayed; the rest are preserved.");}
                            }
                        });
                    });
                    state.height=(ui.cursor().top()-top).max(60.);y+=state.height;
                }
            });
            for (uid, state) in &mut view.cells {
                if !visible.contains(uid) {
                    state.preview = None;
                }
            }
            view.pictures.retain(|(uid, version, _), _| {
                visible.contains(uid)
                    && view
                        .book
                        .cells
                        .iter()
                        .any(|c| c.uid == *uid && c.outputs_version == *version)
            });
            if let Some(selected_action) = action.take() {
                match selected_action {
                    Action::Add(kind, at) => {
                        if view.book.cells.len() < notebook::MAX_CELLS {
                            let cell = Cell::new(kind);
                            view.active = cell.uid;
                            view.focus = Some(cell.uid);
                            view.cells.entry(cell.uid).or_default().previewing = false;
                            view.book.cells.insert(at, cell);
                            view.reveal = true;
                            changed = true;
                        } else {
                            view.error = "Notebook is limited to 1024 cells.".into();
                        }
                    }
                    Action::Delete(at) => {
                        let uid = view.book.cells.remove(at).uid;
                        view.cells.remove(&uid);
                        view.invalidate_pictures(uid);
                        view.active = view
                            .book
                            .cells
                            .get(at)
                            .or_else(|| view.book.cells.last())
                            .map(|c| c.uid)
                            .unwrap_or(0);
                        changed = true;
                    }
                    Action::Move(at, delta) => {
                        let to = at.saturating_add_signed(delta);
                        view.book.cells.swap(at, to);
                        changed = true;
                    }
                    Action::Kind(at, kind) => {
                        view.book.cells[at].change_kind(&kind);
                        changed = true;
                    }
                    Action::Clear(at) => {
                        let cell = at.and_then(|at| view.book.cells.get(at)).map(|cell| {
                            cell.data["id"]
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| format!("uid:{}", cell.uid))
                        });
                        if let Some(kernel) = &mut view.kernel
                            && let Err(error) = kernel.clear_outputs(cell)
                        {
                            view.error = error.to_string();
                        }
                        for (index, cell) in view.book.cells.iter_mut().enumerate() {
                            if at.is_none() || at == Some(index) {
                                cell.clear_outputs();
                            }
                        }
                        view.pictures.clear();
                        view.display_ids.clear();
                        changed = true;
                    }
                    Action::Kernel => match view.book.set_kernel(&view.kernel_name) {
                        Ok(()) => {
                            view.stop("Kernel changed · Run to start");
                            changed = true;
                        }
                        Err(e) => view.error = e.to_string(),
                    },
                    Action::Interrupt => {
                        if let Some(kernel) = &mut view.kernel
                            && let Err(e) = kernel.interrupt()
                        {
                            view.error = e.to_string();
                        }
                    }
                    Action::Shutdown => view.stop("Kernel shut down · Run to restart"),
                    Action::Preview(at) => {
                        view.cells
                            .entry(view.book.cells[at].uid)
                            .or_default()
                            .previewing = true;
                    }
                    other => action = Some(other),
                }
            }
            if changed {
                view.sync(&mut self.documents[index]);
            }
        }
        let source = view.source_mode;
        self.notebook_picture_jobs(&view, jobs, ctx);
        self.documents[index].notebook = Some(view);
        match action {
            Some(Action::Run(all)) => self.run_notebook(all, ctx),
            Some(Action::Source) => self.toggle_notebook_source(ctx),
            _ => {}
        }
        if let Some(link) = link {
            if link.starts_with("https://")
                || link.starts_with("http://")
                || link.starts_with("mailto:")
            {
                ctx.open_url(egui::OpenUrl::new_tab(link));
            } else {
                self.notify("Use the explorer to open local notebook links.", false);
            }
        }
        !source
    }
}
fn image_bundle(bundle: &Value) -> Option<(String, String)> {
    for (mime, path) in [("image/png", "output.png"), ("image/jpeg", "output.jpg")] {
        if let Ok(data) = notebook::multiline(&bundle[mime])
            && data.len() <= 2 * 1024 * 1024
        {
            return Some((data, path.into()));
        }
    }
    None
}
fn output_text(output: &Value) -> String {
    let text=match output["output_type"].as_str().unwrap_or("") {
        "stream"=>notebook::multiline(&output["text"]).unwrap_or_default(),
        "error"=>output["traceback"].as_array().map(|lines|lines.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).filter(|s|!s.is_empty()).unwrap_or_else(||format!("{}: {}",output["ename"].as_str().unwrap_or("Error"),output["evalue"].as_str().unwrap_or(""))),
        _=>notebook::multiline(&output["data"]["text/plain"]).or_else(|_|notebook::multiline(&output["data"]["text/markdown"])).unwrap_or_else(|_|"[Rich output has no supported text/PNG/JPEG representation; its data is preserved.]".into()),
    };
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else if !c.is_control() || "\n\t\r".contains(c) {
            result.push(c);
        }
        if result.len() >= 64 * 1024 {
            result.push_str(
                "\n[Output display limited to 64 KiB; full output remains in the notebook.]",
            );
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn python_notebooks_default_to_the_environment_without_rewriting_saved_metadata() {
        for (language, saved, expected) in [
            ("python", "old-venv", "python3"),
            ("julia", "julia-1.12", "julia-1.12"),
        ] {
            let mut book = Notebook::default();
            book.set_kernel(saved).unwrap();
            book.data["metadata"]["language_info"] = serde_json::json!({"name": language});
            let original = book.serialize().unwrap();
            let view = View::new(
                book,
                Arc::new(markdown::Images::default()),
                Shutdowns::default(),
            );
            assert_eq!(view.kernel_name, expected);
            assert_eq!(view.book.serialize().unwrap(), original);
            assert!(view.kernel.is_none());
        }
    }
    #[test]
    fn a_disconnected_notebook_cannot_start_a_local_kernel_or_read_external_images() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_notebook();
        app.documents[0].path = Some("/remote/work.ipynb".into());
        app.workspace = Some(crate::app::tests::remote_workspace());
        app.detach_ssh(&ctx);
        app.run_notebook(false, &ctx);
        assert!(app.documents[0].notebook.as_ref().unwrap().kernel.is_none());
        assert!(
            app.notice
                .as_ref()
                .unwrap()
                .0
                .contains("disconnected notebook")
        );
        let view = app.documents[0].notebook.as_ref().unwrap();
        app.notebook_picture_jobs(
            view,
            vec![PictureJob {
                target: Target::Markdown {
                    uid: 1,
                    token: 1,
                    uri: "outside.png".into(),
                },
                encoded: None,
                path: "/remote/outside.png".into(),
                cancel: Arc::new(AtomicBool::new(false)),
            }],
            &ctx,
        );
        let result = view.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            result
                .result
                .err()
                .unwrap()
                .contains("Reconnect or Save As")
        );
    }

    #[test]
    fn clearing_outputs_forgets_display_targets_only_for_that_cell() {
        let mut view = View::new(
            Notebook::default(),
            Arc::new(markdown::Images::default()),
            Shutdowns::default(),
        );
        let uid = view.book.cells[0].uid;
        view.display_ids.insert("clear".into(), vec![(uid, 0, 0)]);
        view.display_ids
            .insert("other".into(), vec![(uid + 100, 0, 0)]);
        view.drop_displays(uid);
        assert!(!view.display_ids.contains_key("clear"));
        assert!(view.display_ids.contains_key("other"));
    }
    #[test]
    fn kernel_startup_failure_preserves_saved_outputs_and_a_clean_buffer() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join(".venv/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let python = bin.join("python");
        std::fs::write(&python,"#!/bin/sh\nprintf '%s\\n' '{\"kind\":\"error\",\"message\":\"missing optional packages\"}'\n").unwrap();
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_notebook();
        let book = Notebook::parse(include_str!("../../tests/fixtures/notebook.ipynb")).unwrap();
        let text = book.serialize().unwrap();
        app.documents[0].text = text.clone();
        app.documents[0].saved_text = text.clone();
        app.documents[0].modified = false;
        app.documents[0].notebook = Some(View::new(
            book,
            app.markdown_images.clone(),
            app.notebook_shutdowns.clone(),
        ));
        app.workspace = Some(Workspace {
            root: root.path().to_string_lossy().into(),
            remote: None,
            ready: true,
            connected: true,
            plan: sandbox::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap(),
            sandbox_options: Default::default(),
        });
        app.run_notebook(true, &ctx);
        let start = Instant::now();
        while app.documents[0].notebook.as_ref().unwrap().error.is_empty() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
            app.poll_notebooks(&ctx);
        }
        assert_eq!(app.documents[0].text, text);
        assert!(!app.documents[0].dirty());
        assert_eq!(
            app.documents[0].notebook.as_ref().unwrap().book.cells[2].outputs()[0]["text"],
            json!(["42\n"])
        );
    }
    #[test]
    fn missing_slurm_target_preserves_outputs_and_never_starts_a_host_kernel() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let queue = root.path().join("queue");
        std::fs::write(&queue, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&queue, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_notebook();
        let book = Notebook::parse(include_str!("../../tests/fixtures/notebook.ipynb")).unwrap();
        let text = book.serialize().unwrap();
        app.documents[0].text = text.clone();
        app.documents[0].saved_text = text.clone();
        let mut view = View::new(
            book,
            app.markdown_images.clone(),
            app.notebook_shutdowns.clone(),
        );
        app.documents[0].modified = false;
        let target = crate::slurm::Target {
            id: "4242".into(),
            name: "expired".into(),
            queue: queue.to_string_lossy().into(),
            runner: "/bin/false".into(),
        };
        view.set_execution_target(Some(target.clone()));
        app.documents[0].notebook = Some(view);
        app.workspace = Some(Workspace {
            root: root.path().to_string_lossy().into(),
            remote: None,
            ready: true,
            connected: true,
            plan: sandbox::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap(),
            sandbox_options: Default::default(),
        });
        app.run_notebook(true, &ctx);
        let start = Instant::now();
        while app.documents[0].notebook.as_ref().unwrap().error.is_empty() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
            app.poll_notebooks(&ctx);
        }
        let view = app.documents[0].notebook.as_mut().unwrap();
        assert!(view.kernel.is_none());
        assert_eq!(view.execution_target, Some(target));
        assert!(
            view.error.contains("no longer your running job"),
            "{}",
            view.error
        );
        view.set_execution_target(None);
        assert!(view.error.is_empty());
        assert!(!view.uses_slurm());
        assert_eq!(app.documents[0].text, text);
        assert!(!app.documents[0].dirty());
    }

    #[test]
    fn enter_in_notebook_find_reveals_and_selects_a_matching_cell() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_notebook();
        let book = Notebook::parse(include_str!("../../tests/fixtures/notebook.ipynb")).unwrap();
        let expected = book.cells[1].uid;
        app.documents[0].text = book.serialize().unwrap();
        app.documents[0].notebook = Some(View::new(
            book,
            app.markdown_images.clone(),
            app.notebook_shutdowns.clone(),
        ));
        app.show_find = true;
        app.find = "counter = 40".into();
        app.find_focus = true;
        let frame = |app: &mut EditorApp, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200., 800.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    app.editor(ctx);
                },
            )
        };
        frame(&mut app, vec![]);
        frame(
            &mut app,
            vec![egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert_eq!(app.documents[0].notebook.as_ref().unwrap().active, expected);
    }
}
