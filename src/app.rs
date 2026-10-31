//! Native UI coordinator for documents, workspace identity and owned services.
//!
//! Input and layout stay on the egui thread. Dispatch captures the host, actual
//! sandbox plan and generation before starting workers; `poll` rejects results
//! from obsolete contexts and checks buffer versions before applying edits.
//! Domain modules own their panel/view state, while common save/project/exit
//! guards keep dirty buffers and independent terminal lifetimes intact.
//!
//! # Design decisions
//!
//! One UI-thread owner avoids sharing mutable editor buffers with workers.
//! Workers return snapshots instead: cancellation alone cannot retract a result
//! already queued, so generations and source versions remain the authority for
//! accepting it. Stable document IDs let tab order change without changing which
//! buffer owns undo state, selections or a pending save.
//!
//! Candidate projects are prepared before replacing the current environment.
//! A slow SSH login or unavailable sandbox must not discard the working project
//! or edits made while probing. Common guarded actions keep menu, palette and
//! native-close behavior consistent without separate discard/exit rules.

use crate::{
    files::{self, Entry, Request, Response},
    sandbox,
    ssh::{self, Profile, Remote},
    terminal::Terminal,
    theme,
};
mod agents;
mod binary_view;
mod chrome;
mod commands;
mod connections;
mod debugging;
mod extensions;
mod file_browser;
mod icons;
mod image_view;
mod latex_tools;
mod lifecycle;
mod markdown;
mod marp_view;
mod navigation;
mod notebook_view;
mod pdf_view;
mod sessions;
mod terminal_colors;
mod vaults;
mod workbench;
use eframe::egui::{self, Align, FontId, Key, Layout, RichText, Sense, Stroke};
use icons::{Icon, IconButton};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use workbench::{Panel, ToolJob, Workbench};

pub(crate) fn validate_project_preferences(text: &str) -> Result<(), serde_json::Error> {
    serde_json::from_str::<std::collections::BTreeMap<String, workbench::Preferences>>(text)
        .map(|_| ())
}

pub(crate) fn project_preference_defaults(
    value: serde_json::Value,
) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::from_value::<std::collections::BTreeMap<String, workbench::Preferences>>(value)
        .and_then(serde_json::to_value)
}

/// Validated command-line/startup choices consumed when creating the native editor.
#[derive(Default)]
pub struct Startup {
    /// Optional initial local project directory.
    pub local: Option<PathBuf>,
    /// Optional SSH connection/profile, taking the remote startup route.
    pub remote: Option<Profile>,
    /// Optional interface-theme identifier override.
    pub theme: Option<String>,
    /// Optional independent syntax-theme identifier override.
    pub syntax_theme: Option<String>,
    /// Request sandboxing for the initial project, subject to host capability probing.
    pub sandbox: bool,
}

#[derive(Clone)]
struct TreeRow {
    entry: Entry,
    depth: usize,
}

/// Execution context selected by this window. A connected transport is not a ready project:
/// file/tool operations wait for preparation to establish the actual plan.
struct Workspace {
    root: String,
    remote: Option<Remote>,
    connected: bool,
    ready: bool,
    plan: sandbox::Plan,
    sandbox_options: sandbox::Options,
}

/// One stable tab identity and editable source/save baseline, with optional specialized
/// document state. Only bound paths authorize project-host file operations; recovery paths are
/// display/reconnect metadata.
pub struct Document {
    markdown: Option<markdown::Preview>,
    marp: Option<marp_view::View>,
    image: Option<image_view::ImageView>,
    pdf: Option<pdf_view::PdfView>,
    hex: Option<binary_view::HexView>,
    notebook: Option<notebook_view::View>,
    /// Stable runtime tab identity, preserved when reordering tabs.
    pub id: u64,
    /// Writable/readable path bound to the active project host, or `None` for unsaved/recovery
    /// buffers.
    pub path: Option<String>,
    recovery: Option<connections::Recovery>,
    /// Current source text; structured/preview documents use their dedicated state where
    /// applicable.
    pub text: String,
    /// Exact source snapshot acknowledged by the last successful text save.
    pub saved_text: String,
    /// Content digest read/saved on the bound host, used for conflict detection.
    pub revision: Option<String>,
    /// Whether a source/binary save request is in flight.
    pub saving: bool,
    modified: bool,
    edit_version: u64,
    last_cursor: usize,
    /// Displayed one-based source line and character column.
    pub cursor: (usize, usize),
    selection: Option<(usize, usize)>,
    lines: usize,
}

impl Document {
    fn is_preview(&self) -> bool {
        self.image.is_some() || self.pdf.is_some() || self.hex.is_some() || self.notebook.is_some()
    }
    fn preview_loading(&self) -> bool {
        self.image.as_ref().is_some_and(|image| image.loading)
            || self.pdf.as_ref().is_some_and(|pdf| pdf.image.loading)
    }
    /// Return the cached edit state maintained by ordinary source/structured editing paths.
    pub fn dirty(&self) -> bool {
        self.modified
    }
    /// Acknowledge the submitted text snapshot, updating path/revision while preserving newer
    /// unsaved edits.
    pub fn acknowledge_save(&mut self, path: String, text: String, revision: String) {
        self.path = Some(path);
        self.recovery = None;
        self.saved_text = text;
        self.revision = Some(revision);
        self.saving = false;
        self.modified = self.text != self.saved_text;
    }
    /// Recovery paths describe retained tabs; only `path` authorizes file I/O.
    fn display_path(&self) -> Option<&str> {
        self.path
            .as_deref()
            .or_else(|| self.recovery.as_ref().map(|r| r.path.as_str()))
    }
    fn name(&self) -> String {
        self.display_path()
            .map(ssh::display_name)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Untitled-{}", self.id))
    }
}

/// Captured completion intent for file workers. Prepare/project-folder discovery uses raw
/// host scope; ordinary lists, file pickers and source operations use the active plan.
enum Job {
    Prepare {
        remote: bool,
        options: sandbox::Options,
        keep_buffers: bool,
        token: u64,
        documents_at_start: Vec<(u64, u64)>,
        restore_saved: Option<bool>,
    },
    Browse {
        remote: bool,
        token: u64,
        path: String,
    },
    BrowseFiles {
        token: u64,
        path: String,
    },
    List(String),
    Open(String),
    SaveBinary {
        id: u64,
        bytes: Arc<Vec<u8>>,
        _lease: Arc<crate::images::Lease>,
    },
    Save {
        id: u64,
        text: String,
    },
    Reload {
        id: u64,
        version: u64,
    },
}
/// Worker-to-UI results. Generation identifies the host/project; token identifies a
/// supersedable request, and document/version identifies the submitted source snapshot.
enum Event {
    Latex {
        generation: u64,
        token: u64,
        preview: bool,
        versions: Vec<(u64, u64)>,
        result: Box<Result<crate::latex::Reply, String>>,
    },
    Marp {
        generation: u64,
        id: u64,
        version: u64,
        token: u64,
        exporting: bool,
        result: Box<Result<marp_view::Completed, String>>,
    },
    MarpPage {
        generation: u64,
        id: u64,
        version: u64,
        token: u64,
        page: usize,
        result: Result<crate::images::Decoded, String>,
    },
    NotebookParsed {
        generation: u64,
        id: u64,
        version: u64,
        result: Result<crate::notebook::Notebook, String>,
    },
    PdfPage {
        generation: u64,
        id: u64,
        version: u64,
        token: u64,
        page: usize,
        result: Result<crate::images::Decoded, String>,
    },
    MarkdownImage {
        generation: u64,
        id: u64,
        token: u64,
        uri: String,
        result: Result<crate::images::Decoded, String>,
    },
    ProfileSaved {
        result: Result<Vec<Profile>, String>,
        submitted: Box<Profile>,
        form: u64,
    },
    Tool {
        generation: u64,
        job: Box<ToolJob>,
        result: Box<Result<crate::project::Response, String>>,
    },
    Done {
        generation: u64,
        job: Job,
        result: Box<Result<Response, String>>,
    },
    Session {
        generation: u64,
        job: sessions::Job,
        result: Result<crate::project::Response, String>,
    },
    Connected {
        generation: u64,
        result: Result<(), String>,
    },
}
#[derive(Clone)]
/// Operations routed through common dirty-buffer/encrypted-project guards before changing
/// ownership or discarding state.
enum Action {
    PreparedProject {
        response: Box<Response>,
        remote: bool,
        options: sandbox::Options,
    },
    Project {
        path: String,
        remote: bool,
        options: sandbox::Options,
    },
    Remote(Profile),
    CloseTab(u64),
    Quit,
    LockEncrypted,
}
#[derive(PartialEq)]
enum Dialog {
    None,
    Connect,
    Folder,
    Open,
    Save(u64),
    Commands,
    Sessions,
    Slurm,
    AgentSessions,
    TerminalExit,
    Shortcuts,
    Extensions,
    Encrypted,
    HexOpen,
    MarpExport(u64, crate::marp::Format),
}

/// Native editor coordinator. Owns tabs/workspace/services on the GUI thread; workers return
/// generation-tagged events, and context changes invalidate old host work before it can mutate
/// current state.
pub struct EditorApp {
    vaults: vaults::State,
    latex: latex_tools::State,
    markdown_images: Arc<markdown::Images>,
    marp_gate: Arc<std::sync::Mutex<()>>,
    image_budget: Arc<crate::images::Budget>,
    pdf_cancel: Arc<AtomicBool>,
    persistence: Option<crate::persistence::Preferences>,
    font_save_pending: bool,
    agents: agents::Agents,
    tabs: chrome::Tabs,
    navigation: navigation::State,
    workbench: Workbench,
    debugger: debugging::State,
    extensions: extensions::Extensions,
    workspace: Option<Workspace>,
    documents: Vec<Document>,
    active: usize,
    next_id: u64,
    generation: u64,
    directories: HashMap<String, Vec<Entry>>,
    tree_rows: Vec<TreeRow>,
    tree_dirty: bool,
    expanded: HashSet<String>,
    loading: HashSet<String>,
    opening: HashSet<String>,
    terminal: Option<Terminal>,
    sessions: sessions::Sessions,
    ssh_master: Option<Terminal>,
    preparing: bool,
    project_token: u64,
    file_browser: file_browser::State,
    folder_remote: bool,
    folder_entries: Vec<Entry>,
    folder_loading: bool,
    folder_needs_load: bool,
    folder_token: u64,
    folder_error: Option<String>,
    folder_policy_path: String,
    sandbox_defaults: sandbox::Options,
    startup_restore: Option<bool>,
    folder_sandbox: sandbox::Options,
    last_window_title: String,
    last_ssh: Option<Profile>,
    show_terminal: bool,
    show_explorer: bool,
    show_hidden: bool,
    dialog: Dialog,
    dialog_focus: bool,
    dialog_viewport: egui::ViewportId,
    ui_viewport: egui::ViewportId,
    dialog_input: String,
    marp_overwrite: bool,
    profile: Profile,
    profiles: Vec<Profile>,
    profiles_loading: Option<mpsc::Receiver<Result<Vec<Profile>, String>>>,
    profile_baseline: Option<Profile>,
    profile_form: u64,
    profile_saving: bool,
    profile_writer: Option<std::thread::JoinHandle<()>>,
    aliases: Vec<String>,
    pending: Option<Action>,
    quitting: bool,
    connection_cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
    notice: Option<(String, bool, Instant)>,
    status: String,
    font_size: f32,
    ui_scale: f32,
    ui_scale_save_pending: bool,
    themes: theme::Themes,
    terminal_colors: terminal_colors::State,
    theme_save_pending: bool,
    syntax_save_pending: bool,
    show_find: bool,
    find: String,
    replacement: String,
    find_focus: bool,
    case_sensitive: bool,
    started: Instant,
}

impl EditorApp {
    /// Create an editor with framework/test storage; production startup uses
    /// [`Self::new_native`] to keep shared preferences private from eframe's writer.
    pub fn new(cc: &eframe::CreationContext<'_>, startup: Startup) -> Self {
        Self::from_storage(cc, startup, cc.storage)
    }

    /// Create the production editor using its field-merging preference store and startup
    /// choices.
    pub fn new_native(
        cc: &eframe::CreationContext<'_>,
        startup: Startup,
        preferences: crate::persistence::Preferences,
    ) -> Self {
        let mut app = Self::from_storage(cc, startup, Some(&preferences));
        app.persistence = Some(preferences);
        app
    }

    fn from_storage(
        cc: &eframe::CreationContext<'_>,
        startup: Startup,
        storage: Option<&dyn eframe::Storage>,
    ) -> Self {
        let requested_theme = startup
            .theme
            .clone()
            .or_else(|| storage.and_then(|s| s.get_string(theme::STORAGE_KEY)))
            .unwrap_or_else(|| "phosphor".into());
        let (mut themes, mut warnings) = theme::Themes::load(&theme::directory(), &requested_theme);
        let requested_syntax = startup
            .syntax_theme
            .clone()
            .or_else(|| storage.and_then(|storage| storage.get_string(theme::syntax::STORAGE_KEY)))
            .unwrap_or_else(|| "ui".into());
        if let Err(error) = themes.select_syntax(&requested_syntax, &cc.egui_ctx) {
            warnings.push(format!(
                "Syntax theme '{requested_syntax}' is unavailable: {error}. Following the UI theme."
            ));
        }
        theme::install(&cc.egui_ctx, themes.active().palette);
        // Keep a display-relative scale separate from editor text and private
        // egui window state, which another RODE process may save independently.
        let ui_scale = storage
            .and_then(|s| s.get_string("ui_scale"))
            .and_then(|s| s.parse::<f32>().ok())
            .filter(|scale| scale.is_finite() && (0.5..=2.0).contains(scale))
            .unwrap_or(1.0);
        cc.egui_ctx
            .options_mut(|options| options.zoom_with_keyboard = false);
        cc.egui_ctx.set_zoom_factor(ui_scale);
        let markdown_images = Arc::new(markdown::Images::default());
        cc.egui_ctx.add_texture_loader(markdown_images.clone());
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            vaults: Default::default(),
            latex: Default::default(),
            markdown_images,
            marp_gate: Default::default(),
            image_budget: Arc::new(crate::images::Budget::default()),
            pdf_cancel: Arc::new(AtomicBool::new(false)),
            persistence: None,
            font_save_pending: false,
            agents: agents::Agents::load(storage),
            tabs: Default::default(),
            navigation: navigation::State::load(storage),
            workbench: Workbench::load(storage),
            debugger: Default::default(),
            extensions: extensions::Extensions::load(storage),
            workspace: None,
            documents: vec![],
            active: 0,
            next_id: 1,
            generation: 0,
            directories: HashMap::new(),
            tree_rows: Vec::new(),
            tree_dirty: true,
            expanded: HashSet::new(),
            loading: HashSet::new(),
            opening: HashSet::new(),
            terminal: None,
            sessions: Default::default(),
            ssh_master: None,
            preparing: false,
            project_token: 0,
            file_browser: Default::default(),
            folder_remote: false,
            folder_entries: vec![],
            folder_loading: false,
            folder_needs_load: false,
            folder_token: 0,
            folder_error: None,
            folder_policy_path: String::new(),
            sandbox_defaults: storage
                .and_then(|s| s.get_string("sandbox_defaults"))
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            folder_sandbox: Default::default(),
            startup_restore: None,
            last_window_title: String::new(),
            last_ssh: None,
            show_terminal: false,
            show_explorer: true,
            show_hidden: false,
            dialog: Dialog::None,
            dialog_focus: false,
            dialog_viewport: egui::ViewportId::ROOT,
            ui_viewport: egui::ViewportId::ROOT,
            dialog_input: String::new(),
            marp_overwrite: false,
            profile: Profile::default(),
            profiles: ssh::load_profiles(),
            profiles_loading: None,
            profile_baseline: None,
            profile_form: 0,
            profile_saving: false,
            profile_writer: None,
            aliases: ssh::aliases(),
            pending: None,
            quitting: false,
            connection_cancel: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
            notice: None,
            status: "Ready".into(),
            font_size: storage
                .and_then(|s| s.get_string("font_size"))
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(14.0)
                .clamp(10.0, 26.0),
            ui_scale,
            ui_scale_save_pending: false,
            terminal_colors: terminal_colors::State::load(storage, &themes),
            themes,
            theme_save_pending: startup.theme.is_some(),
            syntax_save_pending: startup.syntax_theme.is_some(),
            show_find: false,
            find: String::new(),
            replacement: String::new(),
            find_focus: false,
            case_sensitive: true,
            started: Instant::now(),
        };
        if !warnings.is_empty() {
            app.notify(warnings.join("\n"), true);
        }
        if let Some(mut profile) = startup.remote {
            profile.sandbox.enabled |= startup.sandbox;
            app.profile = profile.clone();
            app.connect(profile, &cc.egui_ctx, false);
            app.startup_restore = Some(startup.sandbox);
        } else if let Some(path) = startup.local {
            let mut options = app.sandbox_defaults.clone();
            options.enabled |= startup.sandbox;
            app.prepare_project(
                path.to_string_lossy().into(),
                options,
                false,
                false,
                Some(startup.sandbox),
                &cc.egui_ctx,
            );
        }
        app
    }

    fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some((text.into(), error, Instant::now()));
    }

    fn select_theme(&mut self, id: &str, ctx: &egui::Context) {
        match self.themes.select(id, ctx) {
            Ok(()) => {
                self.status = format!("Theme: {}", self.themes.active().name);
                self.theme_save_pending = true;
            }
            Err(error) => self.notify(error.to_string(), true),
        }
    }

    fn select_syntax_theme(&mut self, id: &str, ctx: &egui::Context) {
        match self.themes.select_syntax(id, ctx) {
            Ok(()) => {
                self.status = format!("Syntax theme: {}", self.themes.active_syntax().name);
                self.syntax_save_pending = true;
            }
            Err(error) => self.notify(error.to_string(), true),
        }
    }

    fn persist_theme_choice(&mut self, storage: Option<&mut (dyn eframe::Storage + 'static)>) {
        if !self.theme_save_pending && !self.syntax_save_pending {
            return;
        }
        let save_ui = std::mem::take(&mut self.theme_save_pending);
        let save_syntax = std::mem::take(&mut self.syntax_save_pending);
        if let Some(storage) = storage {
            if save_ui {
                storage.set_string(theme::STORAGE_KEY, self.themes.active().id.clone());
            }
            if save_syntax {
                storage.set_string(
                    theme::syntax::STORAGE_KEY,
                    self.themes.active_syntax().id.clone(),
                );
            }
            storage.flush();
        } else {
            self.notify("Theme applied, but local preference storage is unavailable. The selection could not be saved.", true);
        }
    }

    fn persist_native(&mut self, gui: Option<&dyn eframe::Storage>) {
        use eframe::Storage;
        let Some(mut storage) = self.persistence.take() else {
            return;
        };
        self.persist_theme_choice(Some(&mut storage));
        self.persist_ui_scale(Some(&mut storage));
        self.persist_project_preferences(Some(&mut storage));
        self.persist_agent_preferences(Some(&mut storage));
        self.persist_extensions(Some(&mut storage));
        self.navigation.bindings.store(&mut storage);
        self.terminal_colors.store(&mut storage);
        if self.font_save_pending {
            self.font_save_pending = false;
            storage.set_string("font_size", self.font_size.to_string());
        }
        if let Some(gui) = gui {
            for key in ["window", "egui"] {
                if let Some(value) = gui.get_string(key) {
                    storage.set_string(key, value);
                }
            }
        }
        storage.flush();
        if let Some(error) = storage.take_error() {
            self.notify(error, true);
        }
        self.persistence = Some(storage);
    }

    /// Scale the whole interface relative to the monitor DPI, preserving text
    /// sizes in points and document/terminal state. An explicit reset is an edit
    /// even when this window already displays 100%: another window may differ.
    fn set_ui_scale(&mut self, scale: f32, ctx: &egui::Context) {
        self.ui_scale = if scale.is_finite() {
            (scale.clamp(0.5, 2.0) * 100.).round() / 100.
        } else {
            1.0
        };
        ctx.set_zoom_factor(self.ui_scale);
        self.ui_scale_save_pending = true;
    }

    fn persist_ui_scale(&mut self, storage: Option<&mut (dyn eframe::Storage + 'static)>) {
        if !std::mem::take(&mut self.ui_scale_save_pending) {
            return;
        }
        if let Some(storage) = storage {
            storage.set_string("ui_scale", self.ui_scale.to_string());
            storage.flush();
        } else {
            self.notify("Interface scale changed, but could not be saved.", true);
        }
    }

    fn ui_scale_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let percent = (self.ui_scale * 100.).round() as u16;
        // Scaling moves every widget. Use completed clicks and close the menu
        // before the next frame applies the zoom, rather than dragging a
        // control whose geometry changes underneath the pointer.
        icons::submenu(
            ui,
            Icon::Settings,
            &format!("Interface scale: {percent}%"),
            |ui| {
                ui.small("Relative to system display scaling");
                for (icon, label, step, enabled) in [
                    (
                        Icon::ChevronLeft,
                        "Smaller (5%)",
                        -0.05,
                        self.ui_scale > 0.5,
                    ),
                    (Icon::ChevronRight, "Larger (5%)", 0.05, self.ui_scale < 2.0),
                ] {
                    if ui
                        .add_enabled(enabled, IconButton::new(icon, label))
                        .clicked()
                    {
                        self.set_ui_scale(self.ui_scale + step, ctx);
                        ui.close();
                    }
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(260.)
                    .show(ui, |ui| {
                        for preset in [50, 75, 90, 100, 110, 125, 150, 175, 200] {
                            let label = if preset == 100 {
                                "100% (system default)".into()
                            } else {
                                format!("{preset}%")
                            };
                            if ui.selectable_label(percent == preset, label).clicked() {
                                self.set_ui_scale(f32::from(preset) / 100., ctx);
                                ui.close();
                            }
                        }
                    });
            },
        );
        if ui
            .add(IconButton::new(Icon::Refresh, "Reset interface scale (100%)").shortcut("Ctrl+0"))
            .clicked()
        {
            self.set_ui_scale(1., ctx);
            ui.close();
        }
    }

    fn theme_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut selected = self.themes.active().id.clone();
        egui::ScrollArea::vertical()
            .max_height(280.0)
            .show(ui, |ui| {
                for entry in &self.themes.entries {
                    ui.radio_value(&mut selected, entry.id.clone(), &entry.name);
                }
            });
        if selected != self.themes.active().id {
            self.select_theme(&selected, ctx);
            ui.close();
        }
    }

    fn syntax_theme_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut selected = self.themes.active_syntax().id.clone();
        ui.label(RichText::new("Syntax colors and styles").small());
        egui::ScrollArea::vertical()
            .id_salt("syntax-theme-choices")
            .max_height(280.)
            .show(ui, |ui| {
                for entry in &self.themes.syntax_entries {
                    ui.radio_value(&mut selected, entry.id.clone(), &entry.name);
                }
            });
        if selected != self.themes.active_syntax().id {
            self.select_syntax_theme(&selected, ctx);
            ui.close();
        }
    }

    fn appearance_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(RichText::new("Interface theme").small());
        self.theme_menu(ui, ctx);
        ui.separator();
        icons::submenu(ui, Icon::Palette, "Syntax highlighting", |ui| {
            self.syntax_theme_menu(ui, ctx)
        });
    }

    /// Capture host, plan, generation and resource guards before spawning blocking work. The
    /// raw-host exception is limited to preparation/project discovery; file pickers retain
    /// active project restrictions.
    fn dispatch(&mut self, request: Request, job: Job, ctx: &egui::Context) {
        let raw = matches!(&job, Job::Prepare { .. } | Job::Browse { .. });
        if self.vaults.locking
            || (!raw
                && (self.workspace.as_ref().is_some_and(|w| !w.ready)
                    || (self.workspace.is_none() && self.preparing)))
        {
            match &job {
                Job::BrowseFiles { .. } => {
                    self.file_browser.loading = false;
                    self.file_browser.error = Some(
                        "Wait for the project environment to be ready before browsing files."
                            .into(),
                    );
                }
                Job::List(path) => {
                    self.loading.remove(path);
                }
                Job::Open(path) => {
                    self.opening.remove(path);
                }
                Job::Save { id, .. } | Job::SaveBinary { id, .. } => {
                    if let Some(doc) = self.documents.iter_mut().find(|d| d.id == *id) {
                        doc.saving = false;
                    }
                }
                _ => {}
            }
            self.notify("Open a project folder and wait for its environment to be ready before accessing files.", true);
            return;
        }
        let use_remote = match &job {
            Job::Prepare { remote, .. } | Job::Browse { remote, .. } => *remote,
            _ => self.workspace.as_ref().is_some_and(|w| w.remote.is_some()),
        };
        let remote = if use_remote {
            self.workspace.as_ref().and_then(|w| w.remote.clone())
        } else {
            None
        };
        let plan = if raw {
            None
        } else {
            self.workspace.as_ref().map(|w| w.plan.clone())
        };
        let mut job = job;
        let restore_policies = if matches!(
            job,
            Job::Prepare {
                restore_saved: Some(_),
                ..
            }
        ) {
            self.sandbox_policies_for_host(use_remote)
        } else {
            Default::default()
        };
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let generation = self.generation;
        let image_budget = self.image_budget.clone();
        let notebooks_enabled = self.feature(crate::extensions::Feature::Notebooks);
        let extension_cancel = self.extensions.cancel.clone();
        let pdf_cancel = if let Job::Reload { id, .. } = &job {
            self.documents
                .iter()
                .find(|doc| doc.id == *id)
                .and_then(|doc| doc.pdf.as_ref())
                .map(|pdf| pdf.cancellation())
                .unwrap_or_else(|| self.pdf_cancel.clone())
        } else {
            self.pdf_cancel.clone()
        };
        let texture_limit = ctx.input(|i| i.max_texture_side);
        let pdf_page = if let Job::Reload { id, .. } = &job {
            self.documents
                .iter()
                .find(|doc| doc.id == *id)
                .and_then(|doc| doc.pdf.as_ref())
                .map_or(1, |pdf| pdf.page)
        } else {
            1
        };
        let image_hint = if let Job::Reload { id, .. } = &job {
            self.documents
                .iter()
                .find(|d| d.id == *id)
                .and_then(|d| d.image.as_ref())
                .map(|i| format!("preview.{}", i.format.to_ascii_lowercase()))
        } else {
            None
        };
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let request = match (request, &job) {
                (Request::WriteBinary { path, expected, .. }, Job::SaveBinary { bytes, .. }) => {
                    use base64::Engine;
                    Request::WriteBinary {
                        path,
                        expected,
                        data: base64::engine::general_purpose::STANDARD.encode(bytes.as_ref()),
                    }
                }
                (request, _) => request,
            };
            let _image_io = matches!(
                &request,
                Request::ReadImage { .. } | Request::ReadBinary { .. } | Request::ReadHex { .. }
            )
            .then(|| image_budget.io.lock().unwrap_or_else(|e| e.into_inner()));
            let execute = |request: &Request| match &remote {
                Some(remote) => remote.request_in(request, plan.as_ref()),
                None => match &plan {
                    Some(plan) => plan.local_request(request),
                    None => files::local(request),
                },
            };
            let result = execute(&request)
                .and_then(|response| {
                    if let Job::Prepare {
                        restore_saved: Some(force),
                        options,
                        ..
                    } = &mut job
                    {
                        let mut selected = restore_policies
                            .get(&response.path)
                            .cloned()
                            .unwrap_or_else(|| options.clone());
                        selected.enabled |= *force;
                        *options = selected.clone();
                        if selected.enabled {
                            return execute(&Request::Prepare {
                                path: response.path,
                                sandbox: selected,
                            });
                        }
                    }
                    Ok(response)
                })
                .and_then(|mut response| {
                    if let Some(encoded) = response.binary.take() {
                        let _binary_io = matches!(&request, Request::ReadAuto { .. })
                            .then(|| image_budget.io.lock().unwrap_or_else(|e| e.into_inner()));
                        anyhow::ensure!(
                            !extension_cancel.load(Ordering::Acquire),
                            "Extension settings changed; reopen the document."
                        );
                        if matches!(&request, Request::ReadHex { .. } | Request::ReadAuto { .. }) {
                            response.hex =
                                Some(crate::binary::Loaded::decode(&encoded, &image_budget)?);
                        } else {
                            response.pdf = Some(crate::pdf::load_cancellable(
                                &encoded,
                                pdf_page,
                                &image_budget,
                                texture_limit,
                                &pdf_cancel,
                            )?);
                        }
                    }
                    if let Some(encoded) = response.image.take() {
                        anyhow::ensure!(
                            !extension_cancel.load(Ordering::Acquire),
                            "Extension settings changed; reopen the image."
                        );
                        response.preview = Some(
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                crate::images::decode(
                                    &encoded,
                                    if let Some(hint) = &image_hint {
                                        hint
                                    } else if let Request::ReadImage { path } = &request {
                                        path
                                    } else {
                                        &response.path
                                    },
                                    &image_budget,
                                    texture_limit,
                                )
                            }))
                            .map_err(|_| {
                                anyhow::anyhow!("Image decoder failed; the file may be corrupt.")
                            })??,
                        );
                    }
                    if notebooks_enabled
                        && response.hex.is_none()
                        && crate::notebook::is_path(&response.path)
                        && matches!(&request, Request::Read { .. } | Request::ReadAuto { .. })
                    {
                        response.notebook = Some(
                            crate::notebook::Notebook::parse(&response.text)
                                .map_err(|e| format!("{e:#}")),
                        );
                    }
                    Ok(response)
                })
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Done {
                generation,
                job,
                result: Box::new(result),
            });
            ctx.request_repaint();
        });
    }

    fn list(&mut self, path: String, ctx: &egui::Context) {
        if self.loading.insert(path.clone()) {
            self.dispatch(Request::List { path: path.clone() }, Job::List(path), ctx);
        }
    }

    fn open_file(&mut self, path: String, ctx: &egui::Context) {
        self.editor_intent();
        if ctx.viewport_id() != egui::ViewportId::ROOT {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        }
        if let Some(index) = self
            .documents
            .iter()
            .position(|d| d.path.as_deref() == Some(&path))
        {
            self.active = index;
            self.reveal_tab(self.documents[index].id);
            self.focus_editor(ctx);
            return;
        }
        if self.opening.insert(path.clone()) {
            let request = if crate::pdf::is_path(&path) {
                self.status = "Opening PDF preview…".into();
                Request::ReadBinary { path: path.clone() }
            } else if crate::images::format_for(&path).is_some() {
                Request::ReadImage { path: path.clone() }
            } else {
                if self.feature(crate::extensions::Feature::Binary) {
                    Request::ReadAuto { path: path.clone() }
                } else {
                    Request::Read { path: path.clone() }
                }
            };
            self.dispatch(request, Job::Open(path), ctx);
        }
    }

    /// Drain a bounded batch of worker results. Reject obsolete generations/tokens first,
    /// then apply domain-specific buffer/version checks before mutating visible state.
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = self.profiles_loading.take() {
            match receiver.try_recv() {
                Ok(Ok(profiles)) => self.profiles = profiles,
                Ok(Err(error)) => self.notify(error, true),
                Err(mpsc::TryRecvError::Empty) => self.profiles_loading = Some(receiver),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.notify("Connection profile reload failed.", true)
                }
            }
        }
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::NotebookParsed {
                    generation,
                    id,
                    version,
                    result,
                } if generation == self.generation => {
                    self.notebook_parsed(id, version, result, ctx)
                }
                Event::Session {
                    generation,
                    job,
                    result,
                } if generation == self.generation => self.handle_session(job, result, ctx),
                Event::PdfPage {
                    generation,
                    id,
                    version,
                    token,
                    page,
                    result,
                } if generation == self.generation => {
                    self.pdf_page(id, version, token, page, result)
                }
                Event::ProfileSaved {
                    result,
                    submitted,
                    form,
                } => {
                    if let Some(worker) = self.profile_writer.take() {
                        let _ = worker.join();
                    }
                    self.profile_saving = false;
                    match result {
                        Ok(profiles) => {
                            if form == self.profile_form {
                                self.profile_baseline = Some(*submitted);
                            }
                            self.profiles = profiles;
                            self.notify(
                                "Connection profile saved. Passwords are never stored.",
                                false,
                            );
                        }
                        Err(error) => self.notify(error, true),
                    }
                }
                Event::Latex {
                    generation,
                    token,
                    preview,
                    versions,
                    result,
                } => {
                    if generation == self.generation {
                        self.complete_latex(token, preview, versions, *result, ctx);
                    }
                }
                Event::Marp {
                    generation,
                    id,
                    version,
                    token,
                    exporting,
                    result,
                } => {
                    if generation == self.generation {
                        self.marp_complete(id, version, token, exporting, *result, ctx);
                    }
                }
                Event::MarpPage {
                    generation,
                    id,
                    version,
                    token,
                    page,
                    result,
                } => {
                    if generation == self.generation {
                        self.marp_page(id, version, token, page, result);
                    }
                }
                Event::MarkdownImage {
                    generation,
                    id,
                    token,
                    uri,
                    result,
                } if generation == self.generation => {
                    self.markdown_image(id, token, uri, result, ctx);
                }
                Event::Tool {
                    generation,
                    job,
                    result,
                } if generation == self.generation => self.handle_tool(*job, *result, ctx),
                Event::Connected { generation, result } if generation == self.generation => {
                    match result {
                        Ok(()) => {
                            if let Some(workspace) = &mut self.workspace {
                                workspace.connected = true;
                                let root = workspace.root.clone();
                                self.status = "SSH connected".into();
                                let options = workspace.sandbox_options.clone();
                                let restore = self.startup_restore.take();
                                if self.vaults.lost && self.vaults.active.is_some() {
                                    self.encrypted_dialog();
                                    self.vaults.message = "SSH reconnected. Unlock the encrypted project to restore file access; unsaved buffers are preserved.".into();
                                } else {
                                    self.prepare_project(root, options, true, true, restore, ctx);
                                }
                            }
                        }
                        Err(error) => {
                            self.ssh_failed(error, ctx);
                        }
                    }
                }
                Event::Done {
                    generation,
                    job,
                    result,
                } if generation == self.generation => {
                    match &job {
                        Job::Prepare { token, .. } if *token != self.project_token => continue,
                        Job::Reload { id, version }
                            if !self
                                .documents
                                .iter()
                                .any(|doc| doc.id == *id && doc.edit_version == *version) =>
                        {
                            continue;
                        }
                        Job::Browse { token, path, .. }
                            if *token != self.folder_token
                                || self.dialog != Dialog::Folder
                                || *path != self.dialog_input =>
                        {
                            continue;
                        }
                        Job::BrowseFiles { token, path }
                            if !file_browser::is_file_dialog(&self.dialog)
                                || !self.file_browser.matches(*token, path) =>
                        {
                            continue;
                        }
                        Job::BrowseFiles { .. } => self.file_browser.loading = false,
                        Job::Prepare { .. } => self.preparing = false,
                        Job::Browse { .. } => self.folder_loading = false,
                        Job::List(path) => {
                            self.loading.remove(path);
                        }
                        Job::Open(path) => {
                            self.opening.remove(path);
                        }
                        Job::Save { id, .. } | Job::SaveBinary { id, .. } => {
                            if let Some(doc) = self.documents.iter_mut().find(|d| d.id == *id) {
                                doc.saving = false;
                            }
                        }
                        Job::Reload { id, version } => {
                            if let Some(doc) = self
                                .documents
                                .iter_mut()
                                .find(|doc| doc.id == *id && doc.edit_version == *version)
                            {
                                if let Some(image) = &mut doc.image {
                                    image.loading = false;
                                }
                                if let Some(pdf) = &mut doc.pdf {
                                    pdf.image.loading = false;
                                }
                            }
                        }
                    }
                    if matches!(
                        &job,
                        Job::List(_) | Job::Open(_) | Job::Save { .. } | Job::SaveBinary { .. }
                    ) {
                        self.file_browser.invalidate_rows();
                    }
                    if let Ok(response) = result.as_ref()
                        && let Some(Err(error)) = &response.notebook
                    {
                        self.notify(format!("Notebook: {error} JSON source is available."), true);
                    }
                    match *result {
                        Err(error) => {
                            if matches!(job, Job::Browse { .. }) {
                                self.folder_error = Some(error);
                            } else if matches!(job, Job::BrowseFiles { .. }) {
                                self.file_browser.error = Some(error);
                                self.file_browser
                                    .location
                                    .clone_from(&self.file_browser.directory);
                            } else {
                                self.notify(error, true);
                            }
                        }
                        Ok(response) => match job {
                            Job::Reload { id, version } => {
                                if let Some(doc) = self
                                    .documents
                                    .iter_mut()
                                    .find(|d| d.id == id && d.edit_version == version && !d.dirty())
                                {
                                    if let Some(result) = response.notebook {
                                        doc.notebook = result.ok().map(|book| {
                                            notebook_view::View::new(
                                                book,
                                                self.markdown_images.clone(),
                                            )
                                        });
                                    }
                                    if let Some(loaded) = response.hex {
                                        doc.notebook = None;
                                        doc.hex = Some(binary_view::HexView::new(loaded));
                                        doc.pdf = None;
                                        doc.image = None;
                                        doc.markdown = None;
                                    }
                                    if let Some(loaded) = response.pdf {
                                        if let Some(pdf) = &mut doc.pdf {
                                            pdf.replace(loaded);
                                        } else {
                                            doc.pdf = Some(pdf_view::PdfView::new(loaded));
                                        }
                                    }
                                    if let Some(preview) = response.preview {
                                        if let Some(image) = &mut doc.image {
                                            image.replace(preview);
                                        } else {
                                            doc.image = Some(image_view::ImageView::new(preview));
                                        }
                                    }
                                    doc.text.clone_from(&response.text);
                                    doc.saved_text = response.text;
                                    doc.revision = Some(response.revision);
                                    doc.lines =
                                        doc.text.bytes().filter(|b| *b == b'\n').count() + 1;
                                    doc.edit_version = doc.edit_version.wrapping_add(1);
                                }
                            }
                            Job::Prepare {
                                remote,
                                options,
                                keep_buffers,
                                documents_at_start,
                                ..
                            } => {
                                let new_edits =
                                    self.documents.iter().filter(|d| d.dirty()).any(|d| {
                                        !documents_at_start.contains(&(d.id, d.edit_version))
                                    });
                                if !keep_buffers && new_edits {
                                    self.pending = Some(Action::PreparedProject {
                                        response: Box::new(response),
                                        remote,
                                        options,
                                    });
                                } else {
                                    self.commit_project(
                                        response,
                                        remote,
                                        options,
                                        keep_buffers,
                                        ctx,
                                    );
                                }
                            }
                            Job::BrowseFiles { .. } => self.file_browser.loaded(response),
                            Job::Browse { .. } => {
                                if self.folder_policy_path != response.path {
                                    if let Some(saved) =
                                        self.saved_sandbox_for(&response.path, self.folder_remote)
                                    {
                                        self.folder_sandbox = saved;
                                    }
                                    self.folder_policy_path.clone_from(&response.path);
                                }
                                self.dialog_input = response.path;
                                self.folder_entries =
                                    response.entries.into_iter().filter(|e| e.dir).collect();
                                self.folder_error = None;
                            }
                            Job::List(path) => {
                                if let Some(workspace) = &mut self.workspace
                                    && workspace.root == path
                                {
                                    workspace.root.clone_from(&response.path);
                                    self.expanded.insert(response.path.clone());
                                }
                                self.directories.insert(path, response.entries.clone());
                                self.directories.insert(response.path, response.entries);
                                self.tree_dirty = true;
                            }
                            Job::Open(_) => {
                                if let Some(index) = self
                                    .documents
                                    .iter()
                                    .position(|d| d.path.as_deref() == Some(&response.path))
                                {
                                    self.active = index;
                                } else {
                                    let id = self.next_id;
                                    self.next_id += 1;
                                    let lines =
                                        response.text.bytes().filter(|b| *b == b'\n').count() + 1;
                                    self.documents.push(Document {
                                        recovery: None,
                                        notebook: response.notebook.and_then(Result::ok).map(
                                            |book| {
                                                notebook_view::View::new(
                                                    book,
                                                    self.markdown_images.clone(),
                                                )
                                            },
                                        ),
                                        hex: response.hex.map(binary_view::HexView::new),
                                        markdown: None,
                                        marp: None,
                                        image: response.preview.map(image_view::ImageView::new),
                                        pdf: response.pdf.map(pdf_view::PdfView::new),
                                        id,
                                        path: Some(response.path),
                                        saved_text: response.text.clone(),
                                        text: response.text,
                                        revision: Some(response.revision),
                                        saving: false,
                                        modified: false,
                                        edit_version: 0,
                                        last_cursor: usize::MAX,
                                        cursor: (1, 1),
                                        selection: None,
                                        lines,
                                    });
                                    self.active = self.documents.len() - 1;
                                }
                                if self.documents.get(self.active).is_some_and(|d| {
                                    crate::languages::detect(d.path.as_deref())
                                        .is_some_and(|l| l.id == "python")
                                }) {
                                    self.refresh_environment(false, ctx);
                                }
                                self.status = "File loaded".into();
                                if self.navigation.accepts_editor_focus() {
                                    self.focus_editor(ctx);
                                }
                            }
                            Job::SaveBinary { id, bytes, .. } => {
                                if let Some(doc) = self.documents.iter_mut().find(|d| d.id == id) {
                                    if let Some(hex) = &mut doc.hex {
                                        hex.buffer.acknowledge(bytes);
                                        doc.modified = hex.buffer.dirty();
                                    }
                                    doc.path = Some(response.path);
                                    doc.recovery = None;
                                    doc.revision = Some(response.revision);
                                    doc.saving = false;
                                    self.status = format!("Saved {}", doc.name());
                                }
                            }
                            Job::Save { id, text } => {
                                if let Some(doc) = self.documents.iter_mut().find(|d| d.id == id) {
                                    doc.acknowledge_save(response.path, text, response.revision);
                                    self.status = format!("Saved {}", doc.name());
                                }
                                if self.documents.get(self.active).is_some_and(|d| d.id == id) {
                                    self.reveal_tab(id);
                                }
                                if let Some(root) = self.workspace.as_ref().map(|w| w.root.clone())
                                {
                                    self.list(root, ctx);
                                }
                            }
                        },
                    }
                }
                _ => {}
            }
        }
        if self.ssh_master.as_ref().is_some_and(|t| !t.is_alive())
            && self
                .workspace
                .as_ref()
                .is_some_and(|w| w.remote.is_some() && w.connected)
        {
            self.ssh_failed("SSH connection ended. Remote tabs are retained as disconnected buffers; local files and folders are available.".into(),ctx);
        }
    }

    /// Invalidate project work and release only window-owned services. When preserving
    /// buffers, clear host bindings and retain recovery metadata rather than authorizing
    /// local saves to old remote paths.
    fn clear_project(&mut self, keep_buffers: bool) {
        self.reset_debugger();
        self.stop_latex();
        self.latex = Default::default();
        self.pdf_cancel.store(true, Ordering::Release);
        self.pdf_cancel = Arc::new(AtomicBool::new(false));
        self.leave_project_agent();
        self.stop_notebook_kernels("Workspace changed");
        if !keep_buffers {
            self.tabs = Default::default();
        }
        self.cancel_extensions();
        self.workbench.reset_project();
        self.extensions.busy = false;
        self.extensions.registry.revision = self.extensions.registry.revision.wrapping_add(1);
        self.terminal = None;
        self.session_context_reset();
        for client in [
            self.vaults.active.take(),
            self.vaults.staged.take(),
            self.vaults.pending.take(),
        ]
        .into_iter()
        .flatten()
        {
            self.vaults.release(client);
        }
        if !keep_buffers {
            self.documents.clear();
            self.active = 0;
        }
        for doc in &mut self.documents {
            doc.saving = false;
            if let Some(pdf) = &mut doc.pdf {
                pdf.invalidate();
            }
            if let Some(preview) = &mut doc.markdown {
                preview.invalidate();
            }
            if let Some(view) = &mut doc.marp {
                view.invalidate();
            }
        }
        self.directories.clear();
        self.navigation.tree = Default::default();
        self.file_browser.reset();
        self.navigation.editing = Default::default();
        self.tree_rows.clear();
        self.tree_dirty = true;
        self.expanded.clear();
        self.loading.clear();
        self.opening.clear();
        self.generation += 1;
        self.show_find = false;
    }

    fn reset_workspace(&mut self) {
        self.connection_cancel.store(true, Ordering::Release);
        self.clear_project(false);
        self.ssh_master = None;
        if let Some(remote) = self.workspace.as_ref().and_then(|w| w.remote.as_ref()) {
            remote.cleanup();
        }
        self.workspace = None;
        self.preparing = false;
        self.project_token += 1;
        self.folder_token += 1;
    }

    fn disconnect_ssh(&mut self, ctx: &egui::Context) {
        if self.vaults.active.is_some()
            || self.vaults.pending.is_some()
            || self.vaults.staged.is_some()
            || self.vaults.cleanup_pending()
        {
            self.notify("Lock the encrypted project before disconnecting SSH.", true);
            return;
        }
        if self.workspace.as_ref().is_none_or(|w| w.remote.is_none()) {
            return;
        }
        if self.sessions_need_exit_check() || self.agent_installation_pending() {
            self.notify(
                "Wait for terminal startup or persistence changes to finish before disconnecting SSH.",
                false,
            );
            return;
        }
        self.detach_ssh(ctx);
    }

    /// Validate/probe a candidate asynchronously while retaining the current working project.
    /// Capture document versions so a late candidate cannot discard edits made during
    /// preparation.
    fn prepare_project(
        &mut self,
        path: String,
        options: sandbox::Options,
        remote: bool,
        keep_buffers: bool,
        restore_saved: Option<bool>,
        ctx: &egui::Context,
    ) {
        if remote
            && self
                .workspace
                .as_ref()
                .is_none_or(|w| w.remote.is_none() || !w.connected)
        {
            self.notify(
                "SSH is offline. Reconnect before opening a remote project.",
                true,
            );
            return;
        }
        if let Err(error) = options.validate() {
            self.notify(error.to_string(), true);
            return;
        }
        self.project_token += 1;
        self.preparing = true;
        self.status = if remote {
            "Opening SSH project…"
        } else {
            "Opening local project…"
        }
        .into();
        self.dispatch(
            Request::Prepare {
                path,
                sandbox: if restore_saved.is_some() {
                    Default::default()
                } else {
                    options.clone()
                },
            },
            Job::Prepare {
                remote,
                options,
                keep_buffers,
                token: self.project_token,
                documents_at_start: self
                    .documents
                    .iter()
                    .map(|d| (d.id, d.edit_version))
                    .collect(),
                restore_saved,
            },
            ctx,
        );
    }

    /// Install an accepted prepared context after common guards pass. Invalidate old
    /// services/generations before starting commands for the new host and policy.
    fn commit_project(
        &mut self,
        response: Response,
        remote: bool,
        options: sandbox::Options,
        keep_buffers: bool,
        ctx: &egui::Context,
    ) {
        let Some(plan) = response.project else {
            self.notify("Host returned an invalid project description.", true);
            return;
        };
        let connection = if remote {
            self.workspace.as_ref().and_then(|w| w.remote.clone())
        } else {
            None
        };
        if remote && connection.is_none() {
            self.notify("SSH connection ended before the project was opened.", true);
            return;
        }
        let encrypted = self
            .vaults
            .take_project(&response.path, connection.as_ref());
        if remote || (keep_buffers && self.workspace.as_ref().is_some_and(|w| w.remote.is_none())) {
            self.clear_project(keep_buffers);
        } else {
            self.reset_workspace();
        }
        let connection = connection.map(|mut r| {
            r.profile.root = response.path.clone();
            r.profile.sandbox = options.clone();
            self.profile = r.profile.clone();
            r
        });
        self.workspace = Some(Workspace {
            root: response.path.clone(),
            remote: connection,
            connected: true,
            ready: true,
            plan: plan.clone(),
            sandbox_options: options.clone(),
        });
        self.vaults.active = encrypted;
        self.vaults.lost = false;
        self.vaults.released = false;
        if remote {
            self.restore_ssh_buffers();
        }
        self.expanded.insert(response.path.clone());
        self.directories.insert(response.path, response.entries);
        self.tree_dirty = true;
        self.preparing = false;
        self.load_project_tools(ctx);
        self.status = format!(
            "{} project · {}",
            if remote { "SSH" } else { "Local" },
            plan.label()
        );
        if plan.requested && !plan.active {
            self.notify(plan.reason.clone(), false);
        }
        self.show_terminal |= remote || options.enabled;
        if self.show_terminal {
            self.start_project_terminal(ctx);
        }
    }

    fn start_project_terminal(&mut self, ctx: &egui::Context) {
        self.new_terminal(ctx);
    }

    fn browse_folder(&mut self, path: String, ctx: &egui::Context) {
        if self.folder_remote
            && self
                .workspace
                .as_ref()
                .is_none_or(|w| !w.connected || w.remote.is_none())
        {
            self.folder_error = Some("SSH is offline. Reconnect to browse folders.".into());
            return;
        }
        self.dialog_input = path.clone();
        self.folder_token += 1;
        self.folder_loading = true;
        self.folder_error = None;
        self.dispatch(
            Request::List { path: path.clone() },
            Job::Browse {
                remote: self.folder_remote,
                token: self.folder_token,
                path,
            },
            ctx,
        );
    }

    fn open_local_folder_dialog(&mut self) {
        self.set_dialog(Dialog::Folder);
        self.folder_remote = false;
        self.dialog_input = std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .into();
        self.folder_needs_load = true;
    }

    fn connect(&mut self, profile: Profile, ctx: &egui::Context, reconnect: bool) {
        if (self.vaults.active.is_some() && !self.vaults.lost)
            || (self.vaults.cleanup_pending() && !reconnect)
        {
            self.notify(
                "Lock the encrypted project before replacing its SSH connection.",
                true,
            );
            return;
        }
        self.startup_restore = None;
        let remote = match Remote::new(profile.clone()) {
            Ok(remote) => remote,
            Err(error) => {
                self.notify(error.to_string(), true);
                return;
            }
        };
        if reconnect {
            self.stop_debugger();
            self.stop_latex();
            self.connection_cancel.store(true, Ordering::Release);
            self.terminal = None;
            self.ssh_master = None;
            if let Some(old) = self.workspace.as_ref().and_then(|w| w.remote.as_ref()) {
                old.cleanup();
            }
            self.generation += 1;
            self.workbench.reset_monitor();
            self.loading.clear();
            self.opening.clear();
            for doc in &mut self.documents {
                doc.saving = false;
                if let Some(preview) = &mut doc.markdown {
                    preview.invalidate();
                }
                if let Some(view) = &mut doc.marp {
                    view.invalidate();
                }
            }
        } else {
            self.reset_workspace();
        }
        self.workspace = Some(Workspace {
            root: profile.root.clone(),
            remote: Some(remote.clone()),
            connected: false,
            ready: false,
            plan: sandbox::Plan {
                root: profile.root.clone(),
                ..Default::default()
            },
            sandbox_options: profile.sandbox.clone(),
        });
        match Terminal::remote(ctx, &remote) {
            Ok(terminal) => {
                let alive = terminal.alive.clone();
                self.ssh_master = Some(terminal);
                self.show_terminal = true;
                self.terminal_intent();
                self.status = "Connecting · complete any prompts in the terminal".into();
                let cancel = Arc::new(AtomicBool::new(false));
                self.connection_cancel = cancel.clone();
                let tx = self.tx.clone();
                let thread_ctx = ctx.clone();
                let generation = self.generation;
                let vault_work = self.vaults.work();
                std::thread::spawn(move || {
                    let _vault_work = vault_work;
                    let start = Instant::now();
                    let result = loop {
                        if cancel.load(Ordering::Acquire) {
                            return;
                        }
                        if !alive.load(Ordering::Acquire) {
                            break Err("SSH login failed.".into());
                        }
                        if remote.connected() {
                            break Ok(());
                        }
                        if start.elapsed() > Duration::from_secs(300) {
                            break Err(
                                "SSH login timed out after five minutes. Reconnect to try again."
                                    .into(),
                            );
                        }
                        std::thread::sleep(Duration::from_millis(200));
                    };
                    let _ = tx.send(Event::Connected { generation, result });
                    thread_ctx.request_repaint();
                });
                Terminal::focus(ctx);
            }
            Err(error) => {
                self.ssh_failed(format!("{error:#}"), ctx);
            }
        }
    }

    /// Defer an ownership-changing action when source edits or encrypted-project work require
    /// an explicit decision; route accepted actions through the same lifecycle path.
    fn guarded(&mut self, action: Action, ctx: &egui::Context) {
        if self.vaults.locking {
            return;
        }
        if self.vaults.cleanup_pending()
            && matches!(
                action,
                Action::Project { .. } | Action::Remote(_) | Action::PreparedProject { .. }
            )
        {
            self.notify(
                "Finish locking the encrypted project before switching projects or hosts.",
                true,
            );
            return;
        }
        if let Some(client) = &self.vaults.active {
            let leaves_project = match &action {
                Action::Remote(_) => true,
                Action::Project { path, remote, .. } => !client.matches(
                    path,
                    self.workspace
                        .as_ref()
                        .filter(|w| w.remote.is_some() == *remote)
                        .and_then(|w| w.remote.as_ref()),
                ),
                Action::PreparedProject {
                    response, remote, ..
                } => !client.matches(
                    &response.path,
                    self.workspace
                        .as_ref()
                        .filter(|w| w.remote.is_some() == *remote)
                        .and_then(|w| w.remote.as_ref()),
                ),
                _ => false,
            };
            if leaves_project && self.vaults.staged.is_none() {
                self.notify(
                    "Lock the encrypted project before switching projects or hosts.",
                    true,
                );
                return;
            }
        }
        if let Action::Remote(profile) = &action
            && self.can_resume_ssh(profile)
        {
            self.perform(action, ctx);
            return;
        }
        let dirty = match action {
            Action::CloseTab(id) => self.documents.iter().any(|d| d.id == id && d.dirty()),
            _ => self.documents.iter().any(Document::dirty),
        };
        if dirty {
            self.dialog_viewport = ctx.viewport_id();
            self.pending = Some(action);
        } else {
            self.perform(action, ctx);
        }
    }

    fn perform(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::PreparedProject {
                response,
                remote,
                options,
            } => self.commit_project(*response, remote, options, false, ctx),
            Action::Project {
                path,
                remote,
                options,
            } => self.prepare_project(path, options, remote, false, None, ctx),
            Action::Remote(profile) => {
                let resume = self.can_resume_ssh(&profile);
                self.connect(profile, ctx, resume);
            }
            Action::CloseTab(id) => {
                if let Some(index) = self.documents.iter().position(|d| d.id == id) {
                    self.documents.remove(index);
                    if self.active > index {
                        self.active -= 1;
                    }
                    self.active = self.active.min(self.documents.len().saturating_sub(1));
                    self.focus_editor(ctx);
                }
            }
            Action::Quit => {
                if self.vaults.active.is_some() && self.vaults.lost && self.vaults.released {
                    self.clear_project(false);
                    self.quit_terminals(ctx);
                } else if self.vaults.active.is_some() {
                    self.lock_encrypted(true, ctx);
                } else if self.vaults.pending.is_some()
                    || self.vaults.staged.is_some()
                    || self.vaults.cleanup_pending()
                {
                    self.notify(
                        "Finish encrypted-project unlocking or locking before exiting. Use Open encrypted project → Lock existing mount to resolve an unlocked mount.",
                        true,
                    );
                } else {
                    self.quit_terminals(ctx);
                }
            }
            Action::LockEncrypted => self.lock_encrypted(false, ctx),
        }
    }

    fn set_dialog(&mut self, dialog: Dialog) {
        if self.dialog == Dialog::Encrypted && dialog != Dialog::Encrypted && !self.vaults.locking {
            self.cancel_encrypted_dialog();
        }
        if dialog == Dialog::Connect && self.profiles_loading.is_none() {
            let (tx, rx) = mpsc::channel();
            self.profiles_loading = Some(rx);
            let vault_work = self.vaults.work();
            std::thread::spawn(move || {
                let _vault_work = vault_work;
                let _ = tx.send(ssh::load_profiles_checked().map_err(|e| format!("{e:#}")));
            });
        }
        self.navigation.list = Default::default();
        if file_browser::is_file_dialog(&dialog) {
            self.file_browser.reset();
        }
        self.dialog = dialog;
        self.dialog_viewport = self.ui_viewport;
        self.dialog_focus = true;
        self.dialog_input = match self.dialog {
            Dialog::Folder => {
                self.folder_token += 1;
                self.folder_remote = self.workspace.as_ref().is_some_and(|w| w.remote.is_some());
                self.folder_entries.clear();
                self.folder_needs_load = true;
                self.folder_error = None;
                self.folder_sandbox = self
                    .workspace
                    .as_ref()
                    .map(|w| w.sandbox_options.clone())
                    .unwrap_or_else(|| self.sandbox_defaults.clone());
                self.folder_policy_path = self
                    .workspace
                    .as_ref()
                    .map(|w| w.root.clone())
                    .unwrap_or_default();
                self.workspace
                    .as_ref()
                    .map(|w| w.root.clone())
                    .unwrap_or_else(|| {
                        std::env::current_dir()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into()
                    })
            }
            Dialog::Save(id) => self
                .documents
                .iter()
                .find(|d| d.id == id)
                .and_then(|d| {
                    d.path.clone().or_else(|| {
                        d.recovery
                            .as_ref()
                            .map(|r| self.join_root(ssh::display_name(&r.path)))
                    })
                })
                .unwrap_or_else(|| {
                    self.join_root(
                        if self
                            .documents
                            .iter()
                            .any(|d| d.id == id && d.notebook.is_some())
                        {
                            "untitled.ipynb"
                        } else if self
                            .documents
                            .iter()
                            .any(|d| d.id == id && d.marp.is_some())
                        {
                            "untitled.marp.md"
                        } else {
                            "untitled.rs"
                        },
                    )
                }),
            Dialog::MarpExport(id, format) => self
                .documents
                .iter()
                .find(|doc| doc.id == id)
                .map(|doc| {
                    std::path::Path::new(doc.path.as_deref().unwrap_or("untitled.marp.md"))
                        .with_extension(format.extension())
                        .to_string_lossy()
                        .into()
                })
                .unwrap_or_default(),
            _ => String::new(),
        };
        self.marp_overwrite = false;
    }

    fn join_root(&self, path: &str) -> String {
        if path.starts_with('/') || path.starts_with('~') {
            return path.into();
        }
        let root = self
            .workspace
            .as_ref()
            .map(|w| w.root.as_str())
            .unwrap_or(".");
        format!("{}/{path}", root.trim_end_matches('/'))
    }

    fn new_file(&mut self) {
        self.editor_intent();
        let id = self.next_id;
        self.next_id += 1;
        self.documents.push(Document {
            recovery: None,
            markdown: None,
            marp: None,
            image: None,
            pdf: None,
            hex: None,
            notebook: None,
            id,
            path: None,
            text: String::new(),
            saved_text: String::new(),
            revision: None,
            saving: false,
            modified: true,
            edit_version: 0,
            last_cursor: usize::MAX,
            cursor: (1, 1),
            selection: None,
            lines: 1,
        });
        self.active = self.documents.len() - 1;
    }

    fn save_active(&mut self, ctx: &egui::Context, save_as: bool) {
        if let Some(doc) = self.documents.get(self.active) {
            if doc.is_preview() && doc.hex.is_none() && doc.notebook.is_none() {
                self.notify("Image and PDF previews are read-only.", false);
                return;
            }
            if save_as || doc.path.is_none() {
                self.set_dialog(Dialog::Save(doc.id));
            } else {
                self.save_document(doc.id, doc.path.clone().unwrap_or_default(), false, ctx);
            }
        }
    }

    /// Submit the exact document snapshot/revision through protected host I/O. Save As
    /// creates without replacing unrelated files; acknowledgements update only the submitted
    /// baseline.
    fn save_document(&mut self, id: u64, path: String, save_as: bool, ctx: &egui::Context) {
        if !save_as
            && self
                .documents
                .iter()
                .any(|doc| doc.id == id && doc.recovery.is_some())
        {
            self.notify("This buffer is disconnected. Use Save As for a local copy, or reconnect to its SSH project.",true);
            return;
        }
        if self.documents.iter().any(|d| d.id == id && d.hex.is_some()) {
            self.save_hex(id, path, save_as, ctx);
            return;
        }
        if self
            .documents
            .iter()
            .any(|d| d.id == id && d.is_preview() && d.notebook.is_none())
        {
            self.notify("Image and PDF previews are read-only.", false);
            return;
        }
        if self
            .documents
            .iter()
            .all(|d| d.id != id || d.notebook.is_none())
            && self.save_with_tools(id, &path, save_as, ctx)
        {
            return;
        }
        let Some(doc) = self.documents.iter_mut().find(|d| d.id == id) else {
            return;
        };
        if doc.saving {
            return;
        }
        let expected = if save_as && doc.path.as_deref() != Some(&path) {
            None
        } else {
            doc.revision.clone()
        };
        let text = doc.text.clone();
        doc.saving = true;
        self.dispatch(
            Request::Write {
                path,
                text: text.clone(),
                expected,
            },
            Job::Save { id, text },
            ctx,
        );
    }

    fn toggle_terminal(&mut self, ctx: &egui::Context) {
        self.show_terminal = !(self.terminal_visible()
            && (!self.workbench.detached || ctx.viewport_id() == Self::tools_viewport()));
        if self.show_terminal {
            self.terminal_intent();
            if self.terminal.is_none() && self.ssh_master.is_none() {
                self.start_project_terminal(ctx);
            }
            Terminal::focus(ctx);
        } else {
            self.focus_editor(ctx);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.pending.is_some()
            || self.workbench_modal()
            || self.session_exit_pending()
            || self.vaults.locking
        {
            return;
        }
        if self.terminal_visible() && self.dialog == Dialog::None && !egui::Popup::is_any_open(ctx)
        {
            Terminal::prepare_input(ctx);
        }
        let terminal_focus = Terminal::focused(ctx) && self.terminal_visible();
        let consume = |mods, key| ctx.input_mut(|i| i.consume_key(mods, key));
        if consume(egui::Modifiers::CTRL, Key::Backtick) {
            self.toggle_terminal(ctx);
        }
        if consume(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::P) {
            self.set_dialog(Dialog::Commands);
        }
        if consume(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::O) {
            self.set_dialog(Dialog::Folder);
        }
        if consume_extensions_shortcut(ctx) {
            self.set_dialog(Dialog::Extensions);
        }
        if self.dialog != Dialog::None {
            return;
        }
        self.debugger_shortcuts(ctx, terminal_focus);
        if consume(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::Tab)
            || consume(egui::Modifiers::COMMAND, Key::PageUp)
        {
            self.cycle_tab(-1, ctx);
        } else if consume(egui::Modifiers::CTRL, Key::Tab)
            || consume(egui::Modifiers::COMMAND, Key::PageDown)
        {
            self.cycle_tab(1, ctx);
        }
        if consume(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::G) {
            self.toggle_panel(Panel::Git, ctx);
        }
        if consume(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::M) {
            self.toggle_panel(Panel::Problems, ctx);
        }
        if consume(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::A) {
            self.toggle_panel(Panel::Agent, ctx);
        }
        if !terminal_focus {
            if consume(egui::Modifiers::ALT | egui::Modifiers::SHIFT, Key::F) {
                self.format_active(true, false, ctx);
            }
            if consume(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::S) {
                self.save_active(ctx, true);
            } else if consume(egui::Modifiers::COMMAND, Key::S) {
                if self.navigation.bindings.style() == navigation::Style::Emacs {
                    self.show_find = true;
                    self.find_focus = true;
                } else {
                    self.save_active(ctx, false);
                }
            }
            if consume(egui::Modifiers::COMMAND, Key::P) {
                self.set_dialog(Dialog::Open);
            }
            if consume(egui::Modifiers::COMMAND, Key::N) {
                self.new_file();
            }
            if consume(egui::Modifiers::COMMAND, Key::W)
                && let Some(doc) = self.documents.get(self.active)
            {
                self.guarded(Action::CloseTab(doc.id), ctx);
            }
            if consume(egui::Modifiers::COMMAND, Key::B) {
                self.show_explorer = !self.show_explorer;
            }
            if consume(egui::Modifiers::COMMAND, Key::F) {
                self.markdown_source();
                self.show_find = true;
                self.find_focus = true;
            }
            if consume(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::R) {
                self.toggle_markdown(ctx);
            }
            if (consume(egui::Modifiers::COMMAND, Key::Equals)
                || consume(egui::Modifiers::COMMAND, Key::Plus))
                && !self.zoom_image_shortcut(ctx, 1.25)
            {
                self.font_size = (self.font_size + 1.0).min(26.0);
                self.font_save_pending = true;
            }
            if consume(egui::Modifiers::COMMAND, Key::Minus)
                && !self.zoom_image_shortcut(ctx, 1. / 1.25)
            {
                self.font_size = (self.font_size - 1.0).max(10.0);
                self.font_save_pending = true;
            }
            if consume(egui::Modifiers::COMMAND, Key::Num0) {
                self.set_ui_scale(1., ctx);
            }
        }
        if !terminal_focus && self.show_find && consume(egui::Modifiers::NONE, Key::Escape) {
            self.show_find = false;
            self.focus_editor(ctx);
        }
    }

    fn titlebar(&mut self, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        egui::TopBottomPanel::top("titlebar").exact_height(48.0).frame(theme::frame(palette.panel)).show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("[r]").monospace().size(22.0).strong().color(palette.accent));
                ui.label(RichText::new("RODE").strong().size(16.0));
                ui.add_space(14.0);
                let file_menu = ui.menu_button("File", |ui| {
                    if ui.add(IconButton::new(Icon::NewFile, "New file").shortcut(self.editing_hint("Ctrl+N"))).clicked() { self.new_file(); ui.close(); }
                    if ui.add(IconButton::new(Icon::NewFile,"New Jupyter notebook")).clicked() {self.new_notebook();ui.close();}
                    if ui.add(IconButton::new(Icon::NewFile,"New Marp presentation")).clicked() {self.new_marp();ui.close();}
                    if ui.add(IconButton::new(Icon::Files, "Open as hex / inspect binary…")).clicked() { self.set_dialog(Dialog::HexOpen); ui.close(); }
                    if ui.add(IconButton::new(Icon::Search, "Open file…").shortcut(self.editing_hint("Ctrl+P"))).clicked() { self.set_dialog(Dialog::Open); ui.close(); }
                    if ui.add(IconButton::new(Icon::Folder, "Open project folder…").shortcut("Ctrl+Shift+O")).clicked() { self.set_dialog(Dialog::Folder); ui.close(); }
                    if ui.add(IconButton::new(Icon::Folder, "Open local folder…")).clicked() { self.open_local_folder_dialog(); ui.close(); }
                    if ui.add(IconButton::new(Icon::Folder, "Open encrypted project…")).clicked() { self.encrypted_dialog(); ui.close(); }
                    if ui.add(IconButton::new(Icon::Remote, "Connect via SSH…")).clicked() { self.set_dialog(Dialog::Connect); ui.close(); }
                    ui.separator();
                    if ui.add(IconButton::new(Icon::Save, "Save").shortcut(self.editing_hint("Ctrl+S"))).clicked() { self.save_active(ctx, false); ui.close(); }
                    if ui.add(IconButton::new(Icon::SaveAs, "Save as…").shortcut(self.editing_hint("Ctrl+Shift+S"))).clicked() { self.save_active(ctx, true); ui.close(); }
                    if ui.add(IconButton::new(Icon::SaveAll, "Save all")).clicked() {
                        let ids: Vec<_> = self.documents.iter().filter(|d| d.dirty() && d.path.is_some()).map(|d| (d.id, d.path.clone().unwrap_or_default())).collect();
                        for (id, path) in ids { self.save_document(id, path, false, ctx); }
                        if self.documents.iter().any(|d| d.path.is_none()) { self.notify("Save untitled files individually with Ctrl+S to choose their paths.", false); }
                        ui.close();
                    }
                    ui.separator();
                    if ui.add(IconButton::new(Icon::Exit, "Quit")).clicked() { self.guarded(Action::Quit, ctx); ui.close(); }
                });
                self.navigation_menu(&file_menu.response);
                ui.menu_button("View", |ui| {
                    icons::submenu(ui, Icon::Palette, "Color theme", |ui| {
                        self.theme_menu(ui, ctx);
                    });
                    icons::submenu(ui, Icon::Palette, "Syntax highlighting", |ui| self.syntax_theme_menu(ui, ctx));
                    // Keep display sizing reachable even when system DPI makes menus large.
                    self.ui_scale_menu(ui, ctx);
                    if ui.add(IconButton::new(Icon::Palette, "Terminal colors…")).clicked() { self.terminal_colors.open = true; ui.close(); }
                    ui.separator();
                    for (panel,icon,label,shortcut) in [(Panel::Git,Icon::Git,"Source control","Ctrl+Shift+G"),(Panel::Monitor,Icon::Monitor,"System monitor",""),(Panel::Problems,Icon::Problems,"Problems","Ctrl+Shift+M"),(Panel::Python,Icon::Python,"Python environment",""),(Panel::Settings,Icon::Settings,"Project settings / sandbox access",""),(Panel::Agent,Icon::Chat,"Agent chat","Ctrl+Shift+A"),(Panel::Debug,Icon::Debug,"Debugger","F5")] {
                        if ui.add(IconButton::new(icon,label).shortcut(shortcut)).clicked() { self.toggle_panel(panel, ctx); ui.close(); }
                    }
                    if ui.add(IconButton::new(Icon::Extensions,"Extensions…").shortcut("Ctrl+Shift+X")).clicked(){self.set_dialog(Dialog::Extensions);ui.close();}
                    if ui.add(IconButton::new(Icon::Terminal,"Terminals & Slurm")).clicked(){self.open_sessions(ctx);ui.close();}
                    if ui.add(IconButton::new(Icon::Settings,"Keyboard shortcuts / navigation…")).clicked() {self.set_dialog(Dialog::Shortcuts);ui.close();}
                    ui.separator();
                    ui.add(egui::Checkbox::new(&mut self.show_explorer, ("Explorer", egui::Atom::grow(), RichText::new("Ctrl+B").text_style(egui::TextStyle::Button).color(palette.muted))));
                    if ui.add(IconButton::new(Icon::Terminal, "Toggle terminal").shortcut("Ctrl+`")).clicked() { self.toggle_terminal(ctx); ui.close(); }
                    if ui.add(IconButton::new(Icon::Files, "Markdown preview").shortcut("Ctrl+Shift+R")).clicked() { self.toggle_markdown(ctx); ui.close(); }
                    if ui.add(IconButton::new(Icon::Play, "Marp slide preview")).clicked() { self.toggle_marp(ctx); ui.close(); }
                    if ui.add(IconButton::new(Icon::Search, "Find / replace").shortcut(self.editing_hint("Ctrl+F"))).clicked() { self.markdown_source(); self.show_find = true; self.find_focus = true; ui.close(); }
                    if ui.checkbox(&mut self.show_hidden, "Show hidden files").changed() { self.tree_dirty = true; }
                    self.font_save_pending |= ui.add(egui::Slider::new(&mut self.font_size, 10.0..=26.0).text("Editor font size")).changed();
                });
                ui.menu_button("Code", |ui| {
                    if ui.add(IconButton::new(Icon::Format, "Format document").shortcut("Shift+Alt+F")).clicked() { self.format_active(true,false,ctx); ui.close(); }
                    if ui.add(IconButton::new(Icon::Check, "Check document")).clicked() { self.format_active(false,false,ctx); ui.close(); }
                    if ui.add(IconButton::new(Icon::Fix, "Ruff: apply safe fixes")).clicked() { self.format_active(true,true,ctx); ui.close(); }
                    if ui.add(IconButton::new(Icon::Play, "Run current file / project")).clicked() { self.run_active(false, ctx); ui.close(); }
                    if ui.add(IconButton::new(Icon::Play, "Build current file / project")).clicked() { self.run_active(true,ctx); ui.close(); }
                });
                ui.menu_button("Debug",|ui|{
                    if ui.add(IconButton::new(Icon::Debug,"Start / continue").shortcut("F5")).clicked(){self.start_debugger(ctx);ui.close();}
                    for (icon,label,cmd,key) in [(Icon::Pause,"Pause","pause","F6"),(Icon::StepOver,"Step over","next","F10"),(Icon::StepIn,"Step into","stepIn","F11"),(Icon::StepOut,"Step out","stepOut","Shift+F11")] {if ui.add(IconButton::new(icon,label).shortcut(key)).clicked(){self.debug_control(cmd);ui.close();}}
                    if ui.add(IconButton::new(Icon::Stop,"Stop / detach").shortcut("Shift+F5")).clicked(){self.stop_debugger();ui.close();}
                    if ui.add(IconButton::new(Icon::Check,"Toggle breakpoint").shortcut("F9")).clicked(){self.toggle_cursor_breakpoint();ui.close();}
                });
                ui.add_space(12.0);
                let label = self.workspace.as_ref().map(|w| format!("{}  /  {}", if w.remote.is_some() { "SSH" } else { "LOCAL" }, ssh::display_name(&w.root))).unwrap_or_else(|| "Your next workspace starts here".into());
                let roomy = ctx.content_rect().width() >= 1180.;
                let workspace_width = (ui.available_width()-if roomy {250.} else {150.}).clamp(90.,360.);
                if ui.add_sized([workspace_width,28.],egui::Button::new(RichText::new(&label).color(palette.muted)).truncate()).on_hover_text(format!("{label}\nOpen command palette · Ctrl+Shift+P")).clicked() { self.set_dialog(Dialog::Commands); }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(IconButton::new(Icon::Remote, "Connect SSH").color(palette.accent)).clicked() { self.set_dialog(Dialog::Connect); }
                    if roomy { ui.label(palette.tag("NATIVE / RUST")); }
                });
            });
        });
    }

    fn workspace_banner(&mut self, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let Some(workspace) = &self.workspace else {
            if self.last_window_title != "RODE" {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title("RODE".into()));
                self.last_window_title = "RODE".into();
            }
            return;
        };
        let remote_profile = workspace.remote.as_ref().map(|r| r.profile.clone());
        let remote = remote_profile.as_ref().map(Profile::endpoint);
        let root = workspace.root.clone();
        let connected = workspace.connected;
        let connecting = !connected && self.ssh_master.as_ref().is_some_and(Terminal::is_alive);
        let ready = workspace.ready;
        let plan = workspace.plan.clone();
        let title = format!(
            "RODE · {}{}{}",
            ssh::display_name(&root),
            remote
                .as_ref()
                .map(|r| {
                    format!(
                        " [SSH {r}{}] ",
                        if connected || connecting {
                            ""
                        } else {
                            " · offline"
                        }
                    )
                })
                .unwrap_or_default(),
            if !ready && (connecting || self.preparing) {
                " [Opening project]"
            } else if ready && plan.active {
                " [Sandbox]"
            } else {
                ""
            }
        );
        if title != self.last_window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_window_title = title;
        }
        if remote.is_none() && !plan.requested && !self.preparing {
            return;
        }
        egui::TopBottomPanel::top("workspace-banner")
            .exact_height(38.0)
            .frame(theme::frame(palette.raised))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if let Some(host) = &remote {
                        let state = if connected {
                            "CONNECTED"
                        } else if connecting {
                            "CONNECTING"
                        } else {
                            "OFFLINE"
                        };
                        chrome::bounded_label(
                            ui,
                            RichText::new(format!("SSH {state}  ·  {host}"))
                                .strong()
                                .color(if connected {
                                    palette.success
                                } else {
                                    palette.error
                                }),
                            230.,
                            20.,
                        )
                        .on_hover_text(format!("SSH {state} · {host}"));
                        if connected || connecting {
                            if ui
                                .add_enabled(
                                    !self.sessions_need_exit_check(),
                                    IconButton::new(Icon::Close, "Disconnect"),
                                )
                                .on_hover_text("Disconnect SSH and preserve open files. Terminals with Persist enabled keep running; other terminals end after their last view detaches.")
                                .on_disabled_hover_text("Wait for terminal startup or persistence changes to finish.")
                                .clicked()
                            {
                                self.disconnect_ssh(ctx);
                            }
                        } else if ui.add(IconButton::new(Icon::Remote, "Reconnect")).clicked() {
                            self.connect(remote_profile.clone().unwrap(), ctx, true);
                        }
                    } else {
                        ui.label(RichText::new("LOCAL PROJECT").strong());
                    }
                    ui.separator();
                    let path_width = (ui.available_width() - 410.0).clamp(60.0, 500.0);
                    chrome::bounded_label(
                        ui,
                        RichText::new(&root).monospace().size(12.0),
                        path_width,
                        20.,
                    )
                    .on_hover_text(&root);
                    if self.preparing {
                        ui.spinner();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Open project…").clicked() {
                            self.set_dialog(Dialog::Folder);
                        }
                        ui.label(
                            RichText::new(if remote.is_some() && !connected && !connecting {
                                "SSH OFFLINE"
                            } else if ready {
                                plan.label()
                            } else {
                                "PROJECT NOT READY"
                            })
                            .monospace()
                            .size(11.0)
                            .color(
                                if remote.is_some() && !connected && !connecting {
                                    palette.error
                                } else if !ready || (plan.requested && !plan.active) {
                                    palette.accent
                                } else {
                                    palette.success
                                },
                            ),
                        )
                        .on_hover_text(if plan.reason.is_empty() {
                            "Project file access and terminal commands use the displayed mode."
                        } else {
                            &plan.reason
                        });
                    });
                });
            });
        if plan.requested && !plan.active && !plan.reason.is_empty() {
            egui::TopBottomPanel::top("sandbox-fallback")
                .frame(theme::frame(palette.panel))
                .show(ctx, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&plan.reason).size(12.0).color(palette.accent),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&plan.reason);
                });
        }
    }

    fn activity(&mut self, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        egui::SidePanel::left("activity")
            .exact_width(52.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(palette.panel)
                    .inner_margin(egui::Margin::symmetric(7, 14)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.;
                ui.spacing_mut().button_padding = egui::vec2(0., 2.);
                let button_height = ((ui.available_height() - 68.) / 12. - 6.).clamp(22., 40.);
                for (icon, hint, action) in [
                    (Icon::Files, "Toggle explorer · Ctrl+B", 0),
                    (Icon::Search, "Open file · Ctrl+P", 1),
                    (Icon::Remote, "Connect via SSH", 2),
                    (Icon::Terminal, "Toggle terminal · Ctrl+`", 3),
                    (Icon::Git, "Source control · Ctrl+Shift+G", 4),
                    (Icon::Monitor, "System monitor", 5),
                    (Icon::Problems, "Problems · Ctrl+Shift+M", 6),
                    (Icon::Python, "Python environment", 7),
                    (Icon::Settings, "Project settings / sandbox access", 8),
                    (Icon::Chat, "Agent chat · Ctrl+Shift+A", 9),
                    (Icon::Extensions, "Extensions · Ctrl+Shift+X", 10),
                    (Icon::Debug, "Debugger · F5", 11),
                ] {
                    let selected = match action {
                        0 => self.show_explorer,
                        2 => self.dialog == Dialog::Connect,
                        3 => self.terminal_visible(),
                        4 => self.tools_visible() && self.workbench.panel == Some(Panel::Git),
                        5 => self.tools_visible() && self.workbench.panel == Some(Panel::Monitor),
                        6 => self.tools_visible() && self.workbench.panel == Some(Panel::Problems),
                        7 => self.tools_visible() && self.workbench.panel == Some(Panel::Python),
                        8 => self.tools_visible() && self.workbench.panel == Some(Panel::Settings),
                        9 => self.tools_visible() && self.workbench.panel == Some(Panel::Agent),
                        10 => self.dialog == Dialog::Extensions,
                        11 => self.tools_visible() && self.workbench.panel == Some(Panel::Debug),
                        _ => false,
                    };
                    let response = ui.add_sized(
                        [36.0, button_height],
                        IconButton::icon(icon, hint)
                            .icon_size(if button_height < 34. { 20. } else { 23. })
                            .selected(selected),
                    );
                    if selected {
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                egui::pos2(response.rect.left() - 7., response.rect.top() + 6.),
                                egui::vec2(2., response.rect.height() - 12.),
                            ),
                            0,
                            palette.accent,
                        );
                    }
                    if response.clicked() {
                        match action {
                            0 => self.show_explorer = !self.show_explorer,
                            1 => self.set_dialog(Dialog::Open),
                            2 => self.set_dialog(Dialog::Connect),
                            3 => self.toggle_terminal(ctx),
                            4 => self.toggle_panel(Panel::Git, ctx),
                            5 => self.toggle_panel(Panel::Monitor, ctx),
                            6 => self.toggle_panel(Panel::Problems, ctx),
                            7 => self.toggle_panel(Panel::Python, ctx),
                            8 => self.toggle_panel(Panel::Settings, ctx),
                            9 => self.toggle_panel(Panel::Agent, ctx),
                            11 => self.toggle_panel(Panel::Debug, ctx),
                            _ => self.set_dialog(Dialog::Extensions),
                        }
                    }
                    ui.add_space(6.0);
                }
                ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                    if ui
                        .add_sized(
                            [36., 28.],
                            IconButton::icon(Icon::Help, "Keyboard shortcuts").icon_size(21.),
                        )
                        .clicked()
                    {
                        self.set_dialog(Dialog::Shortcuts);
                    }
                    ui.label(palette.tag("0.1"));
                });
            });
    }

    fn explorer(&mut self, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        if !self.show_explorer || !self.navigation.visible(navigation::Pane::Explorer) {
            return;
        }
        let maximum = (ctx.available_rect().width() - 320.).clamp(180., 450.);
        let zoom = self.navigation.zoom == Some(navigation::Pane::Explorer);
        let panel = egui::SidePanel::left(if zoom { "explorer-zoom" } else { "explorer" })
            .default_width(250.0)
            .width_range(180.0..=maximum)
            .frame(theme::frame(palette.panel));
        let panel = if zoom {
            panel.exact_width(ctx.available_rect().width())
        } else if let Some(width) = self.navigation.widths[0].take() {
            panel.exact_width(width.clamp(180., maximum))
        } else {
            panel
        };
        panel.show(ctx, |ui| {
            self.pane_ui(ui, navigation::Pane::Explorer);
            ui.horizontal(|ui| {
                ui.label(palette.tag("EXPLORER"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icons::small(ui, Icon::Refresh, "Refresh expanded folders").clicked() {
                        let paths: Vec<_> = self.expanded.iter().cloned().collect();
                        for path in paths {
                            self.list(path, ctx);
                        }
                    }
                    if icons::small(ui, Icon::NewFile, "New file").clicked() {
                        self.new_file();
                    }
                });
            });
            ui.add_space(12.0);
            if let Some(workspace) = &self.workspace {
                let root = workspace.root.clone();
                let remote = workspace.remote.is_some();
                let connected = workspace.connected;
                ui.add(
                    egui::Label::new(
                        RichText::new(ssh::display_name(&root).to_uppercase())
                            .strong()
                            .size(12.0)
                            .color(palette.text),
                    )
                    .truncate(),
                )
                .on_hover_text(&root);
                if remote {
                    ui.label(palette.tag(if connected {
                        "● SSH WORKSPACE"
                    } else {
                        "○ SSH PENDING / OFFLINE"
                    }));
                    if !connected
                        && ui.button("Reconnect").clicked()
                        && let Some(profile) = self
                            .workspace
                            .as_ref()
                            .and_then(|w| w.remote.as_ref())
                            .map(|r| r.profile.clone())
                    {
                        self.connect(profile, ctx, true);
                    }
                }
                ui.add_space(8.0);
                ui.separator();
                self.tree(ui, &root, ctx);
            } else {
                ui.label(
                    RichText::new("NO FOLDER OPEN")
                        .size(12.0)
                        .color(palette.muted),
                );
                ui.add_space(10.0);
                ui.label("Open a local project or connect to another computer with SSH.");
                ui.add_space(10.0);
                let action_size = egui::vec2(ui.available_width(), 32.);
                if ui
                    .add_sized(action_size, IconButton::new(Icon::Folder, "Open folder"))
                    .clicked()
                {
                    self.set_dialog(Dialog::Folder);
                }
                if ui
                    .add_sized(action_size, IconButton::new(Icon::Remote, "Connect SSH"))
                    .clicked()
                {
                    self.set_dialog(Dialog::Connect);
                }
                ui.add_space(28.0);
                ui.separator();
                ui.add_space(12.0);
                ui.label(palette.tag("CONNECTIONS"));
                let profiles = self.profiles.clone();
                for profile in profiles {
                    let label = profile.label();
                    if ui
                        .add_sized(
                            action_size,
                            IconButton::new(Icon::Remote, &label)
                                .color(palette.accent)
                                .truncate(),
                        )
                        .on_hover_text(&label)
                        .clicked()
                    {
                        self.profile = profile;
                        self.guarded(Action::Remote(self.profile.clone()), ctx);
                    }
                }
                if self.profiles.is_empty() {
                    ui.label(
                        RichText::new("Saved SSH profiles appear here.")
                            .small()
                            .color(palette.muted),
                    );
                }
            }
        });
    }

    fn tree(&mut self, ui: &mut egui::Ui, root: &str, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        if self.tree_dirty {
            self.tree_rows.clear();
            flatten_tree(
                &self.directories,
                &self.expanded,
                root,
                0,
                self.show_hidden,
                &mut self.tree_rows,
            );
            self.tree_dirty = false;
        }
        if !self.loading.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(palette.tag("Loading…"));
            });
        } else if !self.directories.contains_key(root) {
            if ui.button("Load folder").clicked() {
                self.list(root.into(), ctx);
            }
        } else if self.tree_rows.is_empty() {
            ui.label(palette.tag("No visible files"));
        }
        self.tree_keyboard(ctx);
        ui.spacing_mut().item_spacing.y = 2.0;
        let mut scroll = egui::ScrollArea::vertical().id_salt("file-tree");
        if std::mem::take(&mut self.navigation.tree.reveal)
            && let Some(index) = self
                .tree_rows
                .iter()
                .position(|r| Some(&r.entry.path) == self.navigation.tree.path.as_ref())
        {
            let top = index as f32 * 27.;
            let offset = self.navigation.tree.offset;
            let height = self.navigation.tree.height.max(27.);
            if top < offset {
                scroll = scroll.vertical_scroll_offset(top);
            } else if top + 27. > offset + height {
                scroll = scroll.vertical_scroll_offset((top + 27. - height).max(0.));
            }
        }
        let output = scroll.show_rows(ui, 25.0, self.tree_rows.len(), |ui, range| {
            for index in range {
                let TreeRow { entry, depth } = self.tree_rows[index].clone();
                let expanded = self.expanded.contains(&entry.path);
                let selected = (ctx.memory(|m| m.has_focus(navigation::Pane::Explorer.id()))
                    && self.navigation.tree.path.as_deref() == Some(&entry.path))
                    || self
                        .documents
                        .get(self.active)
                        .is_some_and(|d| d.path.as_deref() == Some(&entry.path));
                let label = if entry.dir {
                    format!("{}  {}", if expanded { "⌄" } else { "›" }, entry.name)
                } else {
                    entry.name.clone()
                };
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 25.0),
                    Sense::click().difference(Sense::focusable_noninteractive()),
                );
                let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                if selected || response.hovered() {
                    painter.rect_filled(
                        rect,
                        3,
                        if selected {
                            palette.raised
                        } else {
                            palette.panel
                        },
                    );
                }
                let color = if selected {
                    palette.accent
                } else if entry.dir {
                    palette.text
                } else {
                    palette.muted
                };
                let x = rect.left() + depth as f32 * 13. + 6.;
                let mut job = egui::text::LayoutJob::simple(
                    label,
                    FontId::proportional(13.),
                    color,
                    (rect.right() - x - 4.).max(1.),
                );
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                painter.galley(
                    egui::pos2(x, rect.center().y - galley.size().y / 2.),
                    galley,
                    color,
                );
                if !entry.dir {
                    response.context_menu(|ui| {
                        if ui.button("Open as hex / inspect binary").clicked() {
                            self.open_hex(entry.path.clone(), ctx);
                            ui.close();
                        }
                    });
                }
                let response = response.on_hover_text(&entry.path);
                if response.clicked() {
                    self.navigation.tree.path = Some(entry.path.clone());
                    ctx.memory_mut(|m| m.request_focus(navigation::Pane::Explorer.id()));
                    if entry.dir {
                        if expanded {
                            self.expanded.remove(&entry.path);
                        } else {
                            self.expanded.insert(entry.path.clone());
                            if !self.directories.contains_key(&entry.path) {
                                self.list(entry.path.clone(), ctx);
                            }
                        }
                        self.tree_dirty = true;
                    } else {
                        self.open_file(entry.path, ctx);
                    }
                }
            }
        });
        self.navigation.tree.offset = output.state.offset.y;
        self.navigation.tree.height = output.inner_rect.height();
    }

    pub(in crate::app) fn terminal_content(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.terminal_colors.palette(self.themes.active());
        self.pane_ui(ui, navigation::Pane::Terminal);
        self.terminal_header(ui, ctx);
        self.terminal_tabs(ui, ctx);
        ui.separator();
        if self.terminal.is_none() && self.ssh_master.is_none() {
            ui.label(palette.tag(if self.session_busy() {
                "Starting terminal…"
            } else {
                "No terminal attached. Choose New or Sessions."
            }));
            ui.allocate_space(ui.available_size().max(egui::Vec2::ZERO));
        }
        if let Some(terminal) = self.terminal.as_mut().or(self.ssh_master.as_mut())
            && let Some(error) = terminal.ui(ui, self.font_size - 1.0, palette)
        {
            self.notify(error, true);
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        let top = ((ui.available_height() - 440.0) / 2.0).max(25.0);
        ui.add_space(top);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("[ r ]")
                    .monospace()
                    .size(64.0)
                    .color(palette.accent),
            );
            ui.add_space(12.0);
            ui.label(
                RichText::new("RODE")
                    .size(34.0)
                    .strong()
                    .color(palette.text),
            );
            ui.label(
                RichText::new("Small footprint. Far reach.")
                    .size(18.0)
                    .color(palette.muted),
            );
            ui.add_space(12.0);
            ui.label(palette.tag("RUST EDITOR  /  OPENSSH NATIVE  /  REMOTE READY"));
            ui.add_space(32.0);
            let action_size = egui::vec2(ui.available_width().min(320.), 32.);
            if ui
                .add_sized(
                    action_size,
                    egui::Button::new(
                        RichText::new("Connect via SSH")
                            .color(palette.background)
                            .strong(),
                    )
                    .fill(palette.accent),
                )
                .clicked()
            {
                self.set_dialog(Dialog::Connect);
            }
            ui.add_space(8.0);
            if ui
                .add_sized(
                    action_size,
                    IconButton::new(Icon::Folder, "Open project folder").shortcut("Ctrl+Shift+O"),
                )
                .clicked()
            {
                self.set_dialog(Dialog::Folder);
            }
            if ui
                .add_sized(
                    action_size,
                    IconButton::new(Icon::NewFile, "New file").shortcut("Ctrl+N"),
                )
                .clicked()
            {
                self.new_file();
            }
            ui.add_space(28.0);
            ui.label(
                RichText::new("Your SSH config. Your keys. Your machine.")
                    .color(palette.muted)
                    .size(12.0),
            );
            ui.add_space(18.0);
            if ui
                .link(
                    RichText::new("Keyboard shortcuts")
                        .color(palette.accent)
                        .size(12.0),
                )
                .clicked()
            {
                self.set_dialog(Dialog::Shortcuts);
            }
        });
    }

    fn editor(&mut self, ctx: &egui::Context) {
        if !self.navigation.visible(navigation::Pane::Editor) {
            return;
        }
        if self.vaults.locking {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.spinner();
                ui.label("Locking encrypted project…");
            });
            return;
        }
        let palette = self.themes.active().palette;
        let mut breakpoint_toggle = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.background))
            .show(ctx, |ui| {
                if self.foreign_modal(ctx) { ui.disable(); }
                self.pane_ui(ui,navigation::Pane::Editor);
                if ui.available_height() <= 1.0 {
                    return;
                }
                if self.documents.is_empty() {
                    self.welcome(ui);
                    return;
                }
                self.tab_strip(ui, ctx);
                if self.documents.is_empty() {
                    return;
                }
                self.active = self.active.min(self.documents.len() - 1);
                let path = self.documents[self.active]
                    .display_path()
                    .unwrap_or("Untitled");
                let relative = self
                    .workspace
                    .as_ref()
                    .and_then(|w| path.strip_prefix(&w.root))
                    .and_then(|p| p.strip_prefix('/'))
                    .unwrap_or(path);
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(18, 7))
                    .show(ui, |ui| {
                        chrome::bounded_label(ui, palette.tag(relative), ui.available_width(), 18.)
                            .on_hover_text(path);
                    });
                if self.documents[self.active].recovery.is_some() {
                    let doc = &self.documents[self.active];
                    let message = if doc.is_preview() && doc.notebook.is_none() {
                        "Disconnected SSH preview · Reconnect to reload its source."
                    } else {
                        "Disconnected SSH buffer · Save As a local copy, or reconnect to restore file access."
                    };
                    ui.colored_label(palette.muted, message);
                }
                if self.disabled_document_ui(ui) {
                    return;
                }
                if self.hex_ui(ui, ctx) {
                    return;
                }
                if self.pdf_ui(ui, ctx) {
                    return;
                }
                if self.documents[self.active].image.is_some() {
                    let focus = self.take_editor_focus(ui, ctx).is_some();
                    let doc = &mut self.documents[self.active];
                    let id = doc.id;
                    let reload = doc
                        .image
                        .as_mut()
                        .unwrap()
                        .ui(ui, ctx, id, palette, focus)
                        .reload;
                    if reload {
                        self.reload_image(id, ctx);
                    }
                    return;
                }
                if self.notebook_ui(ui, ctx) {
                    return;
                }
                if self.markdown_ui(ui, ctx) {
                    return;
                }
                self.latex_toolbar(ui, ctx);
                if self.show_find {
                    self.find_bar(ui);
                }
                let queued_focus = self.take_editor_focus(ui, ctx);
                let shortcut_style=self.navigation.bindings.style();
                if shortcut_style!=navigation::Style::Standard {ui.label(palette.tag(self.navigation.editing.label(shortcut_style)));}
                let source_stale=self.debug_source_stale(&self.documents[self.active]);
                if source_stale {ui.colored_label(palette.accent,"Source changed since debugging started; execution locations may be stale. Restart debugging to use these changes.");}
                let debug_line=self.debug_line().filter(|(p,_)|self.documents[self.active].path.as_deref()==Some(*p)).filter(|_|!source_stale).map(|(_,line)|line);
                let debug_breakpoints=self.documents[self.active].path.as_ref().and_then(|p|self.workbench.debugger_settings().breakpoints.get(p)).map_or(&[][..],Vec::as_slice);
                let debug_enabled=self.feature(crate::extensions::Feature::Debugger);
                let debug_verified=self.documents[self.active].path.as_ref().and_then(|p|self.debugger.client.as_ref().and_then(|c|c.breakpoints.get(p))).map_or(&[][..],Vec::as_slice);
                let themes = &self.themes;
                let diagnostics = self.document_issues(&self.documents[self.active]);
                let editing=&mut self.navigation.editing;
                let doc = &mut self.documents[self.active];
                editing.bind(doc.id,shortcut_style);
                let id = egui::Id::new(("document", doc.id));
                let jump_requested = doc.selection.is_some();
                let jump_cursor = doc.selection.map(|(_, end)| end);
                if let Some((start, end)) = doc.selection.take() {
                    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(start),
                            egui::text::CCursor::new(end),
                        )));
                    state.store(ctx, id);
                }
                let font_size = self.font_size;
                ui.style_mut()
                    .text_styles
                    .insert(egui::TextStyle::Monospace, FontId::monospace(font_size));
                let lang = syntax_token(
                    doc.display_path()
                        .or_else(|| doc.marp.as_ref().map(|_| "untitled.marp.md")),
                )
                .to_owned();
                let highlight = doc.text.len() < 512 * 1024;
                let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _wrap: f32| {
                    let mut job = if highlight {
                        themes.highlight(ui, buffer.as_str(), &lang, font_size)
                    } else {
                        egui::text::LayoutJob::simple(
                            buffer.as_str().into(),
                            FontId::monospace(font_size),
                            palette.text,
                            f32::INFINITY,
                        )
                    };
                    job.wrap.max_width = f32::INFINITY;
                    ui.fonts_mut(|fonts| fonts.layout_job(job))
                };
                egui::ScrollArea::both()
                    .id_salt(("editor-scroll", doc.id))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.horizontal_top(|ui| {
                            let line_height =
                                ui.fonts_mut(|f| f.row_height(&FontId::monospace(font_size)));
                            let digits=doc.lines.max(1).ilog10()+1;
                            let digit_width=ui.fonts_mut(|f|f.glyph_width(&FontId::monospace(font_size),'0'));
                            let gutter_width=46.0+(digits as f32*digit_width).max(24.0);
                            let gutter_top = ui.cursor().min;
                            let (gutter, _) = ui.allocate_exact_size(
                                egui::vec2(gutter_width, line_height * doc.lines as f32),
                                Sense::hover(),
                            );
                            let mut output = egui::TextEdit::multiline(&mut doc.text)
                                .id(id)
                                .code_editor()
                                .cursor_at_end(false)
                                .font(FontId::monospace(font_size))
                                .desired_width(ui.available_width().max(80.0))
                                .desired_rows(doc.lines.max(2))
                                .frame(false)
                                .margin(egui::vec2(6.0, 0.0))
                                .layouter(&mut layouter)
                                .show(ui);
                            editing.after_source(ui,doc.id,&mut output,&doc.text,palette);
                            if queued_focus == Some(doc.id)
                                || (jump_requested
                                    && ui.memory(|m| m.allows_interaction(ui.layer_id())))
                            {
                                output.response.request_focus();
                                ctx.request_repaint();
                            }
                            if jump_requested
                                && let Some(cursor) = output
                                    .cursor_range
                                    .map(|range| range.primary.index)
                                    .or(jump_cursor)
                            {
                                let caret = output
                                    .galley
                                    .pos_from_cursor(egui::text::CCursor::new(cursor))
                                    .translate(output.galley_pos.to_vec2());
                                ui.scroll_to_rect(caret, Some(Align::Center));
                            }
                            let painter = ui
                                .painter()
                                .with_clip_rect(ui.clip_rect().intersect(gutter));
                            let first = ((ui.clip_rect().top() - gutter_top.y) / line_height)
                                .floor()
                                .max(0.0) as usize;
                            let last = (((ui.clip_rect().bottom() - gutter_top.y) / line_height)
                                .ceil()
                                .max(0.0) as usize
                                + 1)
                            .min(doc.lines);
                            for line in first..last {
                                let y=gutter_top.y+(line as f32+0.5)*line_height;
                                if debug_enabled && let Some(path)=&doc.path {
                                    let rect=egui::Rect::from_min_max(egui::pos2(gutter.left(),y-line_height/2.),egui::pos2(gutter.left()+16.,y+line_height/2.));
                                    let response=ui.interact(rect,id.with(("breakpoint",line)),Sense::click()).on_hover_text("Toggle breakpoint · F9");
                                    if response.clicked(){breakpoint_toggle=Some((path.clone(),line+1));}
                                    if let Some(bp)=debug_breakpoints.iter().find(|b|b.line==line+1){
                                        let index=debug_breakpoints.iter().filter(|b|b.enabled&&b.line<bp.line).count();
                                        let verified=debug_verified.get(index).is_some_and(|b|b.verified);
                                        let pos=egui::pos2(gutter.left()+7.,y);
                                        if bp.enabled && verified {painter.circle_filled(pos,4.,palette.error);} else {painter.circle_stroke(pos,4.,Stroke::new(1.5_f32,if bp.enabled{palette.error}else{palette.muted}));}
                                    }
                                }
                                if debug_line==Some(line+1){let x=gutter.left()+18.;painter.add(egui::Shape::convex_polygon(vec![egui::pos2(x-3.,y-5.),egui::pos2(x+3.,y),egui::pos2(x-3.,y+5.)],palette.accent,Stroke::NONE));}
                                painter.text(
                                    egui::pos2(
                                        gutter.right() - 13.0,
                                        gutter_top.y + line as f32 * line_height,
                                    ),
                                    egui::Align2::RIGHT_TOP,
                                    (line + 1).to_string(),
                                    FontId::monospace(font_size),
                                    if line + 1 == doc.cursor.0 {
                                        palette.accent
                                    } else {
                                        palette.gutter
                                    },
                                );
                            }
                            painter.line_segment(
                                [
                                    egui::pos2(gutter.right() - 4.0, ui.clip_rect().top()),
                                    egui::pos2(gutter.right() - 4.0, ui.clip_rect().bottom()),
                                ],
                                Stroke::new(1.0_f32, palette.border),
                            );
                            if !output.response.changed() {
                                for (issue_index, issue) in diagnostics
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, d)| d.row > first && d.row <= last)
                                {
                                    let color = if issue.severity == "error" {
                                        palette.error
                                    } else {
                                        palette.accent
                                    };
                                    let marker = egui::pos2(
                                        gutter.left() + 21.0,
                                        gutter_top.y
                                            + (issue.row - 1) as f32 * line_height
                                            + line_height / 2.0,
                                    );
                                    painter.circle_filled(marker, 3.0, color);
                                    ui.interact(
                                        egui::Rect::from_center_size(
                                            marker,
                                            egui::vec2(12., line_height),
                                        ),
                                        id.with(("issue", issue_index)),
                                        Sense::hover(),
                                    )
                                    .on_hover_text(format!("{}: {}", issue.code, issue.message));
                                    let offset: usize = doc
                                        .text
                                        .split_inclusive('\n')
                                        .take(issue.row.saturating_sub(1))
                                        .map(|s| s.chars().count())
                                        .sum();
                                    let start = output
                                        .galley
                                        .pos_from_cursor(egui::text::CCursor::new(
                                            offset + issue.column.saturating_sub(1),
                                        ))
                                        .translate(output.galley_pos.to_vec2());
                                    let length = if issue.end_row == issue.row {
                                        issue.end_column.saturating_sub(issue.column).max(1)
                                    } else {
                                        1
                                    };
                                    let end = output
                                        .galley
                                        .pos_from_cursor(egui::text::CCursor::new(
                                            offset + issue.column.saturating_sub(1) + length,
                                        ))
                                        .translate(output.galley_pos.to_vec2());
                                    let underline = egui::Rect::from_min_max(
                                        egui::pos2(start.left(), start.top()),
                                        egui::pos2(
                                            end.right().max(start.left() + 5.),
                                            start.bottom(),
                                        ),
                                    );
                                    let points: Vec<_> = (0..=((underline.width() / 3.) as usize)
                                        .max(2))
                                        .map(|i| {
                                            egui::pos2(
                                                underline.left() + i as f32 * 3.,
                                                underline.bottom() - 1.
                                                    + if i % 2 == 0 { 0. } else { 2. },
                                            )
                                        })
                                        .collect();
                                    ui.painter().with_clip_rect(output.text_clip_rect).add(
                                        egui::Shape::line(points, Stroke::new(1.0_f32, color)),
                                    );
                                    ui.interact(
                                        underline,
                                        id.with(("issue-range", issue_index)),
                                        Sense::hover(),
                                    )
                                    .on_hover_text(format!("{}: {}", issue.code, issue.message));
                                }
                            }
                            if output.response.changed() {
                                doc.edit_version = doc.edit_version.wrapping_add(1);
                                doc.modified = doc.path.is_none() || doc.text != doc.saved_text;
                                doc.lines = doc.text.bytes().filter(|b| *b == b'\n').count() + 1;
                            }
                            if let Some(range) = output.cursor_range
                                && (range.primary.index != doc.last_cursor
                                    || output.response.changed())
                            {
                                doc.last_cursor = range.primary.index;
                                let mut line = 1;
                                let mut col = 1;
                                for c in doc.text.chars().take(range.primary.index) {
                                    if c == '\n' {
                                        line += 1;
                                        col = 1;
                                    } else {
                                        col += 1;
                                    }
                                }
                                doc.cursor = (line, col);
                            }
                        });
                    });
            });
        if let Some((path, line)) = breakpoint_toggle {
            self.toggle_breakpoint(path, line);
        }
    }

    fn find_bar(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        let mut next = false;
        let mut replace = false;
        egui::Frame::new()
            .fill(palette.raised)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                let compact = ui.available_width() < 650.;
                ui.horizontal(|ui| {
                    let field_width = if compact {
                        (ui.available_width() - 168.).max(70.)
                    } else {
                        180.
                    };
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.find)
                            .hint_text("Find in file")
                            .desired_width(field_width),
                    );
                    if self.find_focus {
                        response.request_focus();
                        self.find_focus = false;
                    }
                    next = ui.button("Next ↓").clicked()
                        || ((response.has_focus() || response.lost_focus())
                            && ui.input(|i| i.key_pressed(Key::Enter)));
                    ui.checkbox(&mut self.case_sensitive, "Aa");
                    if !compact {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.replacement)
                                .hint_text("Replace with")
                                .desired_width(140.),
                        );
                        replace = ui.button("Replace all").clicked();
                    }
                    if ui
                        .add_sized(
                            [26., 28.],
                            IconButton::icon(Icon::Close, "Close find").icon_size(14.),
                        )
                        .clicked()
                    {
                        self.show_find = false;
                    }
                });
                if compact {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.replacement)
                                .hint_text("Replace with")
                                .desired_width((ui.available_width() - 108.).max(70.)),
                        );
                        replace = ui.button("Replace all").clicked();
                    });
                }
            });
        if self.find.is_empty() {
            return;
        }
        if replace && let Some(doc) = self.documents.get_mut(self.active) {
            let matches = find_matches(&doc.text, &self.find, self.case_sensitive);
            if !matches.is_empty() {
                doc.edit_version = doc.edit_version.wrapping_add(1);
            }
            for (start, end) in matches.iter().rev() {
                doc.text.replace_range(*start..*end, &self.replacement);
            }
            doc.lines = doc.text.bytes().filter(|b| *b == b'\n').count() + 1;
            doc.modified = doc.path.is_none() || doc.text != doc.saved_text;
            self.status = format!("Replaced {} occurrences", matches.len());
        }
        if next {
            ui.ctx().input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::Enter);
            });
        }
        if next && let Some(doc) = self.documents.get_mut(self.active) {
            let matches = find_matches(&doc.text, &self.find, self.case_sensitive);
            let cursor = egui::TextEdit::load_state(ui.ctx(), egui::Id::new(("document", doc.id)))
                .and_then(|s| s.cursor.char_range())
                .map(|r| r.primary.index)
                .unwrap_or(0);
            let found = matches
                .iter()
                .find(|(start, _)| doc.text[..*start].chars().count() >= cursor)
                .or(matches.first());
            if let Some((start, end)) = found {
                doc.selection = Some((
                    doc.text[..*start].chars().count(),
                    doc.text[..*end].chars().count(),
                ));
            }
            self.status = format!("{} matches", matches.len());
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if ctx.viewport_id() != self.dialog_viewport {
            return;
        }
        let palette = self.themes.active().palette;
        if let Some(action) = self.pending.clone() {
            let modal = egui::Modal::new(egui::Id::new("unsaved")).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.heading("Keep your changes?");
                ui.add_space(8.0);
                ui.label(
                    "You have unsaved edits. Save them before continuing, or discard them here.",
                );
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if palette.primary(ui, "Keep editing").clicked() {
                        self.pending = None;
                        self.focus_editor(ctx);
                    }
                    if ui
                        .button(RichText::new("Discard changes").color(palette.error))
                        .clicked()
                    {
                        self.pending = None;
                        self.perform(action, ctx);
                    }
                });
            });
            if modal.should_close() {
                self.pending = None;
                self.focus_editor(ctx);
            }
            return;
        }
        if self.dialog == Dialog::None {
            return;
        }
        let encrypted_dialog = self.dialog == Dialog::Encrypted;
        let file_dialog = file_browser::is_file_dialog(&self.dialog);
        let response = egui::Modal::new(egui::Id::new("dialog")).show(ctx, |ui| {
            ui.set_width(
                if matches!(self.dialog, Dialog::Sessions | Dialog::AgentSessions) {
                    680_f32.min(ctx.content_rect().width() - 50.)
                } else if self.dialog == Dialog::Connect {
                    500.
                } else {
                    540.
                },
            );
            if self.dialog == Dialog::Extensions {
                self.extensions_dialog(ui, ctx);
            } else if self.dialog == Dialog::Encrypted {
                self.encrypted_ui(ui, ctx);
            } else if matches!(self.dialog, Dialog::MarpExport(..)) {
                self.marp_export_dialog(ui, ctx);
            } else if self.dialog == Dialog::Connect {
                self.connect_dialog(ui, ctx);
            } else if self.dialog == Dialog::Shortcuts {
                self.help_dialog(ui);
            } else if self.dialog == Dialog::Commands {
                self.command_dialog(ui, ctx);
            } else if self.dialog == Dialog::Sessions {
                self.sessions_dialog(ui, ctx);
            } else if self.dialog == Dialog::Slurm {
                self.slurm_dialog(ui, ctx);
            } else if self.dialog == Dialog::AgentSessions {
                self.background_agents_dialog(ui, ctx);
            } else if self.dialog == Dialog::TerminalExit {
                self.terminal_exit_dialog(ui, ctx);
            } else {
                if self.dialog == Dialog::Folder {
                    self.folder_dialog(ui, ctx);
                } else {
                    self.path_dialog(ui, ctx);
                }
            }
        });
        if response.should_close()
            && !(self.dialog == Dialog::TerminalExit && self.session_exit_pending())
            && !self.vaults.locking
        {
            self.dialog = Dialog::None;
        }
        if file_dialog && !file_browser::is_file_dialog(&self.dialog) {
            self.file_browser.reset();
        }
        if self.dialog == Dialog::None {
            if encrypted_dialog {
                self.cancel_encrypted_dialog();
            }
            // egui keeps a closed modal's input barrier for one pass. Clear it
            // promptly even when no background worker will request a repaint.
            ctx.request_repaint();
        }
    }

    fn connect_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.label(palette.tag("REMOTE WORKSPACE"));
        ui.heading("Connect via SSH");
        ui.label(
            RichText::new("OpenSSH under the hood. Your existing configuration works here.")
                .color(palette.muted),
        );
        ui.add_space(12.0);
        if !self.profiles.is_empty() || !self.aliases.is_empty() {
            egui::ComboBox::from_id_salt("profiles")
                .selected_text("Load a saved connection or SSH alias")
                .width(450.0)
                .show_ui(ui, |ui| {
                    for profile in &self.profiles {
                        if ui.selectable_label(false, profile.label()).clicked() {
                            self.profile = profile.clone();
                            self.profile_baseline = Some(profile.clone());
                            self.profile_form += 1;
                        }
                    }
                    if !self.aliases.is_empty() {
                        ui.separator();
                    }
                    for alias in &self.aliases {
                        if ui
                            .selectable_label(false, format!("SSH config / {alias}"))
                            .clicked()
                        {
                            self.profile = Profile {
                                host: alias.clone(),
                                ..Default::default()
                            };
                            self.profile_baseline = None;
                            self.profile_form += 1;
                        }
                    }
                });
            ui.add_space(8.0);
        }
        egui::Grid::new("connection-fields")
            .num_columns(2)
            .spacing([18.0, 10.0])
            .show(ui, |ui| {
                for (label, field, hint) in [
                    (
                        "Host / alias",
                        &mut self.profile.host,
                        "workstation or user@host.example.org",
                    ),
                    (
                        "Remote folder",
                        &mut self.profile.root,
                        "~/projects/my-project",
                    ),
                    ("Username", &mut self.profile.user, "From SSH config"),
                    (
                        "Port",
                        &mut self.profile.port,
                        "From SSH config (usually 22)",
                    ),
                    (
                        "Identity file",
                        &mut self.profile.identity,
                        "~/.ssh/id_ed25519 (optional)",
                    ),
                    (
                        "Jump host(s)",
                        &mut self.profile.jump,
                        "user@login-host (optional, comma-separated)",
                    ),
                    (
                        "SSH config file",
                        &mut self.profile.config,
                        "~/.ssh/config (default)",
                    ),
                    (
                        "Profile name",
                        &mut self.profile.name,
                        "Optional friendly name",
                    ),
                ] {
                    ui.label(label);
                    let response = ui.add(
                        egui::TextEdit::singleline(field)
                            .hint_text(hint)
                            .desired_width(340.0),
                    );
                    if self.dialog_focus && label == "Host / alias" {
                        response.request_focus();
                        self.dialog_focus = false;
                    }
                    ui.end_row();
                }
            });
        ui.add_space(12.0);
        ui.checkbox(
            &mut self.profile.sandbox.enabled,
            "Sandbox the remote project when bwrap is available",
        );
        if self.profile.sandbox.enabled {
            ui.collapsing("Sandbox access rules", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(300.)
                    .show(ui, |ui| {
                        workbench::sandbox_options_ui(ui, &mut self.profile.sandbox)
                    });
            });
        }
        ui.label(RichText::new("Authentication and host-key prompts open in the terminal. Remote file access uses Python 3's standard library.").small().color(palette.muted));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if palette.primary(ui, "Connect").clicked() {
                match self.profile.validate() {
                    Ok(()) => {
                        self.dialog = Dialog::None;
                        self.guarded(Action::Remote(self.profile.clone()), ctx);
                    }
                    Err(error) => self.notify(error.to_string(), true),
                }
            }
            if ui
                .add_enabled(
                    !self.profile_saving,
                    egui::Button::new(if self.profile_saving {
                        "Saving…"
                    } else {
                        "Save profile"
                    }),
                )
                .clicked()
            {
                if let Err(error) = self.profile.validate() {
                    self.notify(error.to_string(), true);
                } else {
                    let profile = self.profile.clone();
                    let previous = self
                        .profile_baseline
                        .as_ref()
                        .filter(|p| p.label() == profile.label())
                        .cloned();
                    let form = self.profile_form;
                    self.profiles_loading = None;
                    let tx = self.tx.clone();
                    let context = ctx.clone();
                    self.profile_saving = true;
                    self.profile_writer = Some(std::thread::spawn(move || {
                        let result = ssh::save_profile(&profile, previous.as_ref())
                            .map_err(|e| format!("{e:#}"));
                        if let Err(error) = &result {
                            eprintln!("Connection profile was not saved: {error}");
                        }
                        let _ = tx.send(Event::ProfileSaved {
                            result,
                            submitted: Box::new(profile),
                            form,
                        });
                        context.request_repaint();
                    }));
                }
            }
            if ui.button("Cancel").clicked() {
                self.dialog = Dialog::None;
            }
        });
    }

    fn path_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.file_dialog(ui, ctx);
    }

    fn folder_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let open_by_key = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Enter));
        if self.folder_needs_load {
            self.folder_needs_load = false;
            self.browse_folder(self.dialog_input.clone(), ctx);
        }
        let host = self
            .workspace
            .as_ref()
            .and_then(|w| w.remote.as_ref())
            .map(|r| r.profile.endpoint())
            .unwrap_or_default();
        ui.label(palette.tag(if self.folder_remote {
            format!("SSH · {host}")
        } else {
            "THIS MACHINE".into()
        }));
        ui.heading(if self.folder_remote {
            "Open remote project folder"
        } else {
            "Open project folder"
        });
        ui.label(
            RichText::new(
                "Browse folders, then open the project. Absolute paths and ~/ are supported.",
            )
            .small()
            .color(palette.muted),
        );
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.dialog_input)
                .desired_width(f32::INFINITY)
                .hint_text("Project folder"),
        );
        if self.dialog_focus {
            response.request_focus();
            self.dialog_focus = false;
        }
        let enter = (response.has_focus() || response.lost_focus())
            && ui.input(|i| i.key_pressed(Key::Enter));
        let mut browse = None;
        ui.horizontal(|ui| {
            if ui.button("Up").clicked() {
                browse = Some(
                    std::path::Path::new(&self.dialog_input)
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .map(|p| p.to_string_lossy().into())
                        .unwrap_or_else(|| "/".into()),
                );
            }
            if ui.button("Browse").clicked() || enter {
                browse = Some(self.dialog_input.clone());
            }
            if self.folder_loading {
                ui.spinner();
                ui.label(palette.tag("Loading folders…"));
            }
        });
        if let Some(error) = &self.folder_error {
            ui.label(RichText::new(error).color(palette.error));
        }
        let query = format!("folder:{}:{}", self.folder_token, self.show_hidden);
        if let Some(action) = file_browser::rows(
            ui,
            "project-folder-browser",
            &self.folder_entries,
            self.show_hidden,
            &mut self.navigation.list,
            &query,
            (0, false),
            None,
            !self.folder_loading,
        ) {
            browse = Some(match action {
                file_browser::Choice::Parent => std::path::Path::new(&self.dialog_input)
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map_or_else(|| "/".into(), |p| p.to_string_lossy().into()),
                file_browser::Choice::Select(entry) | file_browser::Choice::Open(entry) => {
                    entry.path
                }
            });
        }
        if self
            .folder_entries
            .iter()
            .all(|e| !self.show_hidden && e.name.starts_with('.'))
            && !self.folder_loading
        {
            ui.label(palette.tag("No visible subfolders. You can open this folder."));
        }
        if let Some(path) = browse {
            self.browse_folder(path, ctx);
        }
        ui.separator();
        ui.checkbox(
            &mut self.folder_sandbox.enabled,
            "Sandbox project when bwrap is available",
        );
        if self.folder_sandbox.enabled {
            ui.label(RichText::new("If sandboxing is unavailable on this host, the project opens in clearly labelled normal mode.").small().color(palette.muted));
            ui.collapsing("Sandbox access rules", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(260.)
                    .show(ui, |ui| {
                        workbench::sandbox_options_ui(ui, &mut self.folder_sandbox)
                    });
            });
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if palette.primary(ui, "Open project  ·  Ctrl+Enter").clicked() || open_by_key {
                let options = self.folder_sandbox.clone();
                if let Err(error) = options.validate() {
                    self.folder_error = Some(error.to_string());
                } else {
                    self.sandbox_defaults = options.clone();
                    self.workbench.preferences_pending = true;
                    self.dialog = Dialog::None;
                    self.guarded(
                        Action::Project {
                            path: self.dialog_input.clone(),
                            remote: self.folder_remote,
                            options,
                        },
                        ctx,
                    );
                }
            }
            if ui.button("Cancel").clicked() {
                self.dialog = Dialog::None;
            }
        });
    }

    fn help_dialog(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        ui.label(palette.tag("MAKE YOURSELF AT HOME"));
        ui.heading("Keyboard shortcuts");
        let focus = std::mem::take(&mut self.dialog_focus);
        self.navigation.bindings.ui(ui, focus);
        ui.add_space(8.0);
        ui.collapsing("Standard editor shortcuts", |ui| {
            egui::ScrollArea::vertical()
                .id_salt("editor-shortcut-reference")
                .max_height(180.)
                .show(ui, |ui| {
                    egui::Grid::new("shortcut-list")
                        .spacing([32.0, 10.0])
                        .show(ui, |ui| {
                            for (key, description) in [
                                ("Ctrl+Shift+P", "Command palette"),
                                ("Ctrl+P", "Open file by path"),
                                ("Ctrl+Shift+O", "Open project folder"),
                                ("Ctrl+Shift+R", "Toggle Markdown preview"),
                                ("Ctrl+N", "New file"),
                                ("Ctrl+S", "Save file"),
                                ("Ctrl+Shift+S", "Save to a different path"),
                                ("Ctrl+W", "Close tab"),
                                ("Ctrl+PageUp / Down", "Previous / next tab"),
                                ("Ctrl+Tab", "Next tab (Shift for previous)"),
                                ("Ctrl+Shift+A", "Agent chat"),
                                ("Ctrl+Shift+X", "Manage extensions"),
                                ("F5 / Shift+F5", "Debug: start/continue / stop/detach"),
                                ("F6 / F9", "Debug: pause / toggle breakpoint"),
                                ("F10 / F11 / Shift+F11", "Debug: step over / into / out"),
                                ("Ctrl+B", "Toggle explorer"),
                                ("Ctrl+`", "Toggle / focus terminal"),
                                ("Ctrl+F", "Find and replace"),
                                ("Ctrl+Plus / Minus", "Editor font size"),
                                ("Escape", "Close dialog / find"),
                            ] {
                                ui.label(RichText::new(key).monospace().color(palette.accent));
                                ui.label(description);
                                ui.end_row();
                            }
                        });
                });
        });
        ui.add_space(12.0);
        ui.label(RichText::new("On macOS use Command for editor shortcuts. Terminal keys go to the shell when it has focus. Use the Copy button to copy terminal output.").small().color(palette.muted));
        if ui.button("Done").clicked() {
            self.dialog = Dialog::None;
        }
    }

    fn notice_ui(&mut self, ctx: &egui::Context) {
        if egui::Popup::is_any_open(ctx) {
            return;
        }
        let palette = self.themes.active().palette;
        let Some((message, error, time)) = &self.notice else {
            return;
        };
        if !error && time.elapsed() > Duration::from_secs(8) {
            self.notice = None;
            return;
        }
        let text = message.clone();
        let error = *error;
        egui::Area::new(egui::Id::new("notification"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-20.0, -42.0])
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(palette.raised)
                    .stroke(Stroke::new(
                        1.0_f32,
                        if error {
                            palette.error
                        } else {
                            palette.success
                        },
                    ))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_max_width(520.0);
                        ui.horizontal_top(|ui| {
                            ui.label(RichText::new(if error { "!" } else { "✓" }).color(
                                if error {
                                    palette.error
                                } else {
                                    palette.success
                                },
                            ));
                            ui.add(egui::Label::new(text).wrap());
                            if icons::small(ui, Icon::Close, "Dismiss notification").clicked() {
                                self.notice = None;
                            }
                        });
                    });
            });
    }
}

fn consume_extensions_shortcut(ctx: &egui::Context) -> bool {
    ctx.input_mut(|input| {
        let shortcut = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        // egui-winit maps Command+X to Cut even with Shift held. If press and
        // release arrive in one frame, only the release retains its modifiers.
        let modifiers = input
            .events
            .iter()
            .find_map(|event| match event {
                egui::Event::Key {
                    key: Key::X,
                    modifiers,
                    ..
                } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or(input.modifiers);
        let cut = modifiers.matches_exact(shortcut)
            && input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::Cut));
        if cut {
            input
                .events
                .retain(|event| !matches!(event, egui::Event::Cut));
        }
        input.consume_key(shortcut, Key::X) || cut
    })
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.ui_viewport = egui::ViewportId::ROOT;
        self.poll(ctx);
        self.poll_vaults(ctx);
        self.poll_agents(ctx);
        self.poll_debugger(ctx);
        self.poll_notebooks(ctx);
        self.marp_tick();
        self.workbench_tick(ctx);
        self.session_tick(ctx);
        self.handle_close_request(ctx);
        self.replay_style_input(ctx);
        self.navigation_shortcuts(ctx);
        self.editing_shortcuts(ctx);
        self.shortcuts(ctx);
        if (self.dialog != Dialog::None || self.pending.is_some() || self.workbench_modal())
            && Terminal::focused(ctx)
        {
            ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("terminal-input")));
        }
        self.navigation.begin_layout();
        self.titlebar(ctx);
        self.workspace_banner(ctx);
        self.encrypted_banner(ctx);
        self.statusbar(ctx);
        self.activity(ctx);
        self.explorer(ctx);
        self.workspace_content(ctx);
        self.dialogs(ctx);
        if !self.workbench.detached {
            self.workbench_windows(ctx);
        }
        self.extension_windows(ctx);
        self.latex_window(ctx);
        self.terminal_colors.show(ctx, self.themes.active());
        self.notice_ui(ctx);
        self.navigation_finish(ctx);
        self.detached_tools(ctx);
        if self.profiles_loading.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.persistence.is_some() {
            self.persist_native(None);
        } else {
            self.persist_ui_scale(frame.storage_mut());
            self.persist_theme_choice(frame.storage_mut());
            self.persist_project_preferences(frame.storage_mut());
            self.persist_agent_preferences(frame.storage_mut());
            self.persist_extensions(frame.storage_mut());
            if let Some(storage) = frame.storage_mut() {
                self.navigation.bindings.store(storage);
                self.terminal_colors.store(storage);
            }
        }
        // Optional automation hook for a bounded native-window smoke test.
        if std::env::var_os("RODE_SMOKE_TEST").is_some() {
            if self.started.elapsed() > Duration::from_secs(5) {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.persistence.is_some() {
            self.persist_native(Some(storage));
            return;
        }
        self.navigation.bindings.store(storage);
        self.terminal_colors.store(storage);
        self.workbench.store(storage);
        self.agents.store(storage);
        self.persist_extensions(Some(storage));
        storage.set_string("font_size", self.font_size.to_string());
        storage.set_string("ui_scale", self.ui_scale.to_string());
        if let Ok(options) = serde_json::to_string(&self.sandbox_defaults) {
            storage.set_string("sandbox_defaults", options);
        }
        storage.set_string(theme::STORAGE_KEY, self.themes.active().id.clone());
        storage.set_string(
            theme::syntax::STORAGE_KEY,
            self.themes.active_syntax().id.clone(),
        );
    }
}

impl Drop for EditorApp {
    fn drop(&mut self) {
        if let Some(worker) = self.profile_writer.take() {
            let _ = worker.join();
        }
        self.reset_workspace();
    }
}

/// Return the registered highlighting language for a path, falling back to plain text.
pub fn language(path: Option<&str>) -> &'static str {
    crate::languages::detect(path).map_or("text", |language| language.id.as_str())
}

fn file_icon(name: &str) -> &'static str {
    if crate::pdf::is_path(name) {
        return "pdf";
    }
    if crate::images::format_for(name).is_some() {
        return "img";
    }
    match language(Some(name)) {
        "rust" => "rs",
        "python" => "py",
        "toml" | "json" | "yaml" => "{}",
        "markdown" => "md",
        "bash" => "$",
        _ => "·",
    }
}

/// Case-insensitive matching compares Unicode characters without using byte offsets
/// from a lowercased copy (which can have a different UTF-8 length).
pub fn find_matches(text: &str, needle: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return vec![];
    }
    if case_sensitive {
        return text
            .match_indices(needle)
            .map(|(start, matched)| (start, start + matched.len()))
            .collect();
    }
    let needle: Vec<char> = needle.chars().collect();
    let characters: Vec<(usize, char)> = text.char_indices().collect();
    let mut matches = Vec::new();
    let mut i = 0;
    while i + needle.len() <= characters.len() {
        if characters[i..i + needle.len()]
            .iter()
            .zip(&needle)
            .all(|((_, a), b)| a.to_lowercase().eq(b.to_lowercase()))
        {
            let end = characters
                .get(i + needle.len())
                .map(|(n, _)| *n)
                .unwrap_or(text.len());
            matches.push((characters[i].0, end));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    matches
}

fn flatten_tree(
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

/// Syntect accepts case-sensitive language names or filename extensions.
pub fn syntax_token(path: Option<&str>) -> &str {
    crate::languages::detect(path)
        .map(|language| language.syntax.as_str())
        .unwrap_or_else(|| {
            path.and_then(|p| std::path::Path::new(p).extension())
                .and_then(|p| p.to_str())
                .unwrap_or("txt")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_shortcut_handles_native_cut_events_without_consuming_plain_cut() {
        let shortcut = egui::Modifiers::CTRL | egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        for (modifiers, events, opens, cut_remains) in [
            (shortcut, vec![egui::Event::Cut], true, false),
            (
                egui::Modifiers::NONE,
                vec![
                    egui::Event::Cut,
                    egui::Event::Key {
                        key: Key::X,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: shortcut,
                    },
                ],
                true,
                false,
            ),
            (
                egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
                vec![egui::Event::Cut],
                false,
                true,
            ),
            (
                shortcut | egui::Modifiers::ALT,
                vec![egui::Event::Cut],
                false,
                true,
            ),
        ] {
            let ctx = egui::Context::default();
            let _ = ctx.run(
                egui::RawInput {
                    modifiers,
                    events,
                    ..Default::default()
                },
                |ctx| {
                    assert_eq!(consume_extensions_shortcut(ctx), opens);
                    assert_eq!(
                        ctx.input(|i| i.events.contains(&egui::Event::Cut)),
                        cut_remains
                    );
                },
            );
        }
    }

    #[test]
    fn terminal_navigation_reaches_the_pty_on_the_first_frame_after_focus() {
        use portable_pty::CommandBuilder;
        for (key, modifiers, bytes) in [
            (Key::Tab, egui::Modifiers::NONE, b"\t".as_slice()),
            (Key::Tab, egui::Modifiers::SHIFT, b"\x1b[Z".as_slice()),
            (Key::ArrowUp, egui::Modifiers::NONE, b"\x1b[A".as_slice()),
            (Key::ArrowLeft, egui::Modifiers::NONE, b"\x1b[D".as_slice()),
            (Key::Escape, egui::Modifiers::NONE, b"\x1b".as_slice()),
        ] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            app.new_file();
            let mut command = CommandBuilder::new("python3");
            command.args(["-u", "-c", &format!("import sys,tty;tty.setraw(0);print('READY',flush=True);data=sys.stdin.buffer.read({});print('BYTES_'+data.hex(),flush=True)", bytes.len())]);
            app.terminal = Some(Terminal::spawn(&ctx, command, "Input recorder".into()).unwrap());
            app.show_terminal = true;
            app.terminal_intent();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !app.terminal.as_ref().unwrap().contents().contains("READY") {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            let frame = |time| egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(850., 560.),
                )),
                time: Some(time),
                ..Default::default()
            };
            let _ = ctx.run(frame(0.), |ctx| {
                app.workspace_content(ctx);
                Terminal::focus(ctx);
            });
            let mut input = frame(0.1);
            input.modifiers = modifiers;
            input.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            });
            let _ = ctx.run(input, |ctx| {
                app.shortcuts(ctx);
                app.workspace_content(ctx);
            });
            // Back-tab navigation can schedule a focus change for the next frame.
            let _ = ctx.run(frame(0.2), |ctx| {
                app.shortcuts(ctx);
                app.workspace_content(ctx);
            });
            assert!(
                Terminal::focused(&ctx),
                "{modifiers:?}+{key:?} moved terminal focus into editor controls"
            );
            let expected = format!(
                "BYTES_{}",
                bytes
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            while !app
                .terminal
                .as_ref()
                .unwrap()
                .contents()
                .contains(&expected)
            {
                assert!(
                    Instant::now() < deadline,
                    "{modifiers:?}+{key:?} did not reach the PTY"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(app.documents[0].text.is_empty());
        }
    }

    #[test]
    fn edits_made_while_a_project_opens_are_not_silently_discarded() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.project_token = 1;
        app.preparing = true;
        // The request started with no buffers, but the user edited while waiting.
        app.new_file();
        app.documents[0].text = "typed during slow project opening".into();
        let project = tempfile::tempdir().unwrap();
        let response = files::local(&Request::Prepare {
            path: project.path().to_string_lossy().into(),
            sandbox: Default::default(),
        })
        .unwrap();
        app.tx
            .send(Event::Done {
                generation: app.generation,
                job: Job::Prepare {
                    remote: false,
                    options: Default::default(),
                    keep_buffers: false,
                    token: 1,
                    documents_at_start: vec![],
                    restore_saved: None,
                },
                result: Box::new(Ok(response)),
            })
            .unwrap();
        app.poll(&ctx);
        assert!(matches!(app.pending, Some(Action::PreparedProject { .. })));
        assert_eq!(app.documents[0].text, "typed during slow project opening");
        assert!(app.workspace.is_none());
        app.pending = None; // Keep editing.
        assert_eq!(app.documents.len(), 1);
    }

    #[test]
    fn file_operations_wait_until_the_requested_project_environment_is_ready() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let mut workspace = remote_workspace();
        workspace.ready = false;
        app.workspace = Some(workspace);
        app.new_file();
        app.documents[0].text = "keep edits".into();
        app.save_document(app.documents[0].id, "/not-written.rs".into(), true, &ctx);
        assert!(!app.documents[0].saving);
        assert_eq!(app.documents[0].text, "keep edits");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(text, _, _)| text.contains("environment to be ready"))
        );
        app.open_file("/not-read.rs".into(), &ctx);
        assert!(app.opening.is_empty());
        assert_eq!(app.documents.len(), 1);
    }

    pub(super) fn remote_workspace() -> Workspace {
        let remote = Remote::new(Profile {
            host: "test-host".into(),
            user: "researcher".into(),
            ..Default::default()
        })
        .unwrap();
        Workspace {
            root: "/scratch/researcher/project".into(),
            remote: Some(remote),
            connected: true,
            ready: true,
            plan: Default::default(),
            sandbox_options: Default::default(),
        }
    }

    #[test]
    fn disconnect_preserves_dirty_remote_buffers_and_invalidates_host_work() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.workspace = Some(remote_workspace());
        let remote = app.workspace.as_ref().unwrap().remote.clone().unwrap();
        std::fs::write(&remote.socket, "owned connection fixture").unwrap();
        app.new_file();
        let doc = &mut app.documents[0];
        doc.path = Some("/scratch/researcher/project/main.rs".into());
        doc.revision = Some("remote revision".into());
        doc.text = "unsaved remote edits".into();
        doc.modified = true;
        doc.edit_version = 3;
        doc.selection = Some((2, 5));
        let generation = app.generation;
        let project_token = app.project_token;
        let folder_token = app.folder_token;
        let cancel = app.connection_cancel.clone();
        app.preparing = true;
        app.disconnect_ssh(&ctx);
        assert!(app.workspace.is_none());
        assert!(!app.project_ready());
        assert!(!app.preparing);
        assert!(cancel.load(Ordering::Acquire));
        assert!(!remote.socket.exists());
        assert!(app.generation > generation);
        assert!(app.project_token > project_token);
        assert!(app.folder_token > folder_token);
        assert_eq!(app.documents[0].text, "unsaved remote edits");
        assert!(app.documents[0].dirty());
        assert_eq!(app.documents[0].edit_version, 3);
        assert_eq!(app.documents[0].selection, Some((2, 5)));
        assert!(app.documents[0].path.is_none());
        assert!(app.documents[0].revision.is_none());
        assert_eq!(app.documents[0].name(), "main.rs");
        assert_eq!(
            app.documents[0].display_path(),
            Some("/scratch/researcher/project/main.rs")
        );
        assert!(app.pending.is_none());
        assert!(app.ssh_master.is_none());
        assert!(app.terminal.is_none());
        app.tx
            .send(Event::Connected {
                generation,
                result: Ok(()),
            })
            .unwrap();
        app.poll(&ctx);
        assert!(app.workspace.is_none());
        app.set_dialog(Dialog::Folder);
        assert!(!app.folder_remote);
        assert_eq!(
            app.dialog_input,
            std::env::current_dir().unwrap().to_string_lossy()
        );
        app.workspace_banner(&ctx);
        assert_eq!(app.last_window_title, "RODE");
        app.save_document(app.documents[0].id, "/not-local.rs".into(), false, &ctx);
        assert!(!app.documents[0].saving);
        assert_eq!(app.documents[0].text, "unsaved remote edits");
    }

    #[test]
    fn open_folder_defaults_to_the_remote_host_and_local_open_is_explicit() {
        let cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.workspace = Some(remote_workspace());
        let socket = app
            .workspace
            .as_ref()
            .unwrap()
            .remote
            .as_ref()
            .unwrap()
            .socket
            .clone();
        app.set_dialog(Dialog::Folder);
        assert!(app.folder_remote);
        assert_eq!(app.dialog_input, "/scratch/researcher/project");
        app.open_local_folder_dialog();
        assert!(!app.folder_remote);
        assert_eq!(
            app.workspace
                .as_ref()
                .unwrap()
                .remote
                .as_ref()
                .unwrap()
                .socket,
            socket
        );
    }

    #[test]
    fn project_switch_still_protects_unsaved_buffers() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.workspace = Some(remote_workspace());
        app.new_file();
        app.documents[0].text = "unsaved remote edits".into();
        app.guarded(
            Action::Project {
                path: "/scratch/other".into(),
                remote: true,
                options: Default::default(),
            },
            &ctx,
        );
        assert!(app.pending.is_some());
        assert!(!app.preparing);
        assert_eq!(app.documents[0].text, "unsaved remote edits");
        assert_eq!(
            app.workspace.as_ref().unwrap().root,
            "/scratch/researcher/project"
        );
    }

    #[test]
    fn late_folder_browse_cannot_overwrite_another_dialog_or_user_input() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.set_dialog(Dialog::Folder);
        let old = app.dialog_input.clone();
        let token = app.folder_token;
        app.set_dialog(Dialog::Open);
        app.dialog_input = "keep-this.rs".into();
        let response = files::Response {
            path: "/old/folder".into(),
            image: None,
            binary: None,
            pdf: None,
            hex: None,
            notebook: None,
            preview: None,
            entries: vec![],
            text: String::new(),
            revision: String::new(),
            project: None,
            tool: None,
        };
        app.tx
            .send(Event::Done {
                generation: app.generation,
                job: Job::Browse {
                    remote: false,
                    token,
                    path: old,
                },
                result: Box::new(Ok(response.clone())),
            })
            .unwrap();
        app.poll(&ctx);
        assert_eq!(app.dialog_input, "keep-this.rs");
        app.set_dialog(Dialog::Folder);
        let old = app.dialog_input.clone();
        let token = app.folder_token;
        app.dialog_input = "/newly/typed/path".into();
        app.tx
            .send(Event::Done {
                generation: app.generation,
                job: Job::Browse {
                    remote: false,
                    token,
                    path: old,
                },
                result: Box::new(Ok(response)),
            })
            .unwrap();
        app.poll(&ctx);
        assert_eq!(app.dialog_input, "/newly/typed/path");
    }

    #[derive(Default)]
    struct MemoryStorage(HashMap<String, String>, usize);

    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.into(), value);
        }
        fn flush(&mut self) {
            self.1 += 1;
        }
    }

    #[test]
    fn interface_scale_tracks_monitor_dpi_and_preserves_editor_state() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].text = "unsaved work".into();
        app.font_size = 19.;
        app.set_ui_scale(0.75, &ctx);
        let mut storage = MemoryStorage::default();
        app.persist_ui_scale(Some(&mut storage));
        assert_eq!(storage.0["ui_scale"], "0.75");
        assert_eq!(storage.1, 1);
        app.persist_ui_scale(Some(&mut storage));
        assert_eq!(storage.1, 1, "Idle frames must not save scale repeatedly");
        for native_dpi in [1., 2.] {
            let mut input = egui::RawInput::default();
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(native_dpi);
            let _ = ctx.run(input, |ctx| {
                assert_eq!(ctx.pixels_per_point(), native_dpi * 0.75);
            });
        }
        app.select_theme("carbon", &ctx);
        assert_eq!(ctx.zoom_factor(), 0.75);
        assert_eq!(app.font_size, 19.);
        assert_eq!(app.documents[0].text, "unsaved work");
        assert!(app.documents[0].dirty());
        let restored_ctx = egui::Context::default();
        let mut cc = eframe::CreationContext::_new_kittest(restored_ctx.clone());
        // Ignore stale scale in egui's private GUI snapshot on restart.
        restored_ctx.options_mut(|options| options.zoom_factor = 2.);
        cc.storage = Some(&storage);
        let restored = EditorApp::new(&cc, Startup::default());
        assert_eq!(restored.ui_scale, 0.75);
        let _ = restored_ctx.run(Default::default(), |ctx| {
            assert_eq!(ctx.zoom_factor(), 0.75);
            assert!(!ctx.options(|options| options.zoom_with_keyboard));
        });
        assert!(!restored.ui_scale_save_pending);
    }

    #[test]
    fn invalid_interface_scale_uses_system_display_scaling() {
        for invalid in ["NaN", "inf", "bad", "0", "-1", "5"] {
            let mut storage = MemoryStorage::default();
            storage.0.insert("ui_scale".into(), invalid.into());
            let mut cc = eframe::CreationContext::_new_kittest(egui::Context::default());
            cc.storage = Some(&storage);
            let app = EditorApp::new(&cc, Startup::default());
            assert_eq!(app.ui_scale, 1., "{invalid}");
        }
    }

    #[test]
    fn theme_change_flushes_preferences_without_exiting_or_waiting_for_autosave() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let mut storage = MemoryStorage::default();
        storage.0.insert("font_size".into(), "19".into());
        app.persist_theme_choice(Some(&mut storage));
        assert_eq!(storage.1, 0);
        app.select_theme("carbon", &ctx);
        app.persist_theme_choice(Some(&mut storage));
        assert_eq!(storage.0[theme::STORAGE_KEY], "carbon");
        assert_eq!(storage.0["font_size"], "19");
        assert_eq!(storage.1, 1);
        app.persist_theme_choice(Some(&mut storage));
        assert_eq!(
            storage.1, 1,
            "Unchanged frames must not repeatedly flush storage"
        );
        app.select_theme("phosphor", &ctx);
        app.persist_theme_choice(Some(&mut storage));
        assert_eq!(storage.0[theme::STORAGE_KEY], "phosphor");
        assert_eq!(storage.1, 2);
    }

    #[test]
    fn missing_preference_storage_reports_that_the_theme_could_not_be_saved() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.select_theme("carbon", &ctx);
        app.persist_theme_choice(None);
        assert_eq!(app.themes.active().id, "carbon");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(text, error, _)| *error && text.contains("could not be saved"))
        );
        assert!(!app.theme_save_pending);
    }

    #[test]
    fn theme_choice_persists_without_touching_unsaved_edits_or_font_size() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].text = "unsaved work".into();
        app.font_size = 19.0;
        app.select_theme("carbon", &ctx);
        assert_eq!(app.documents.len(), 1);
        assert_eq!(app.documents[0].text, "unsaved work");
        assert!(app.documents[0].dirty());
        assert_eq!(app.font_size, 19.0);
        let mut storage = MemoryStorage::default();
        eframe::App::save(&mut app, &mut storage);
        let mut cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        cc.storage = Some(&storage);
        let restored = EditorApp::new(&cc, Startup::default());
        assert_eq!(restored.themes.active().id, "carbon");
        assert_eq!(restored.font_size, 19.0);
        let overridden = EditorApp::new(
            &cc,
            Startup {
                theme: Some("phosphor".into()),
                ..Default::default()
            },
        );
        assert_eq!(overridden.themes.active().id, "phosphor");
    }

    #[test]
    fn syntax_selection_saves_only_its_own_preference_and_restores_independently() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].text = "unsaved text".into();
        app.font_size = 19.;
        let mut storage = MemoryStorage::default();
        storage.0.insert(theme::STORAGE_KEY.into(), "neon".into());
        app.select_syntax_theme("dracula", &ctx);
        app.persist_theme_choice(Some(&mut storage));
        assert_eq!(storage.0[theme::syntax::STORAGE_KEY], "dracula");
        assert_eq!(storage.0[theme::STORAGE_KEY], "neon");
        assert_eq!(storage.1, 1);
        app.persist_theme_choice(Some(&mut storage));
        assert_eq!(storage.1, 1);
        assert_eq!(app.documents[0].text, "unsaved text");
        assert_eq!(app.font_size, 19.);
        assert!(app.documents[0].dirty());
        app.select_theme("carbon", &ctx);
        app.persist_theme_choice(Some(&mut storage));
        assert_eq!(storage.0[theme::syntax::STORAGE_KEY], "dracula");
        assert_eq!(app.themes.active_syntax().id, "dracula");
        let mut cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        cc.storage = Some(&storage);
        let restored = EditorApp::new(&cc, Startup::default());
        assert_eq!(restored.themes.active().id, "carbon");
        assert_eq!(restored.themes.active_syntax().id, "dracula");
        let override_app = EditorApp::new(
            &cc,
            Startup {
                syntax_theme: Some("nord".into()),
                ..Default::default()
            },
        );
        assert_eq!(override_app.themes.active().id, "carbon");
        assert_eq!(override_app.themes.active_syntax().id, "nord");
        let fallback = EditorApp::new(
            &cc,
            Startup {
                syntax_theme: Some("absent".into()),
                ..Default::default()
            },
        );
        assert_eq!(fallback.themes.active_syntax().id, "ui");
        assert!(fallback.notice.is_some());
    }

    #[test]
    fn save_acknowledgement_keeps_edits_typed_during_save() {
        let mut document = Document {
            recovery: None,
            markdown: None,
            marp: None,
            image: None,
            pdf: None,
            hex: None,
            notebook: None,
            id: 1,
            path: Some("file.rs".into()),
            text: "newer edits".into(),
            saved_text: "original".into(),
            revision: Some("old".into()),
            saving: true,
            modified: true,
            edit_version: 0,
            last_cursor: usize::MAX,
            cursor: (1, 1),
            selection: None,
            lines: 1,
        };
        document.acknowledge_save(
            "file.rs".into(),
            "submitted edits".into(),
            "new revision".into(),
        );
        assert!(document.dirty());
        assert!(!document.saving);
        assert_eq!(document.text, "newer edits");
        assert_eq!(document.revision.as_deref(), Some("new revision"));
    }

    #[test]
    fn explorer_flattens_only_expanded_visible_folders() {
        let entries = |path: &str, dir| Entry {
            path: path.into(),
            name: ssh::display_name(path).into(),
            dir,
        };
        let directories = HashMap::from([
            (
                "/root".into(),
                vec![
                    entries("/root/src", true),
                    entries("/root/.git", true),
                    entries("/root/main.rs", false),
                ],
            ),
            ("/root/src".into(), vec![entries("/root/src/lib.rs", false)]),
        ]);
        let mut rows = vec![];
        flatten_tree(&directories, &HashSet::new(), "/root", 0, false, &mut rows);
        assert_eq!(rows.len(), 2);
        rows.clear();
        flatten_tree(
            &directories,
            &HashSet::from(["/root/src".into()]),
            "/root",
            0,
            true,
            &mut rows,
        );
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].depth, 1);
    }
}
