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
mod appearance;
mod binary_view;
mod chrome;
mod commands;
mod connections;
mod debugging;
mod dialogs;
mod diff_view;
mod documents;
mod editor_view;
mod editor_windows;
mod execution;
mod explorer;
mod extensions;
mod file_browser;
mod frame;
mod git_gutter;
mod icons;
mod image_view;
mod input;
mod latex_tools;
mod layout;
mod lifecycle;
mod transitions;
use transitions::Action;
mod markdown;
mod marp_view;
mod menus;
mod navigation;
mod notebook_view;
mod pdf_view;
mod project_bookmarks;
mod project_dialog;
mod sandbox_presets;
mod saved_connections;
mod sessions;
mod terminal_colors;
mod tunnels;
mod ui_probe;
mod vaults;
mod workbench;
mod workers;
mod workspace;
use commands::Command;
use eframe::egui::{self, Align, FontId, Key, Layout, RichText, Sense, Stroke};
use icons::{Icon, IconButton};
use saved_connections::Completion as ProfileEvent;
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
use workers::{Job, WorkspaceEvent};

/// Validate the serialized map before shared preference merging accepts project settings.
pub(crate) fn validate_project_preferences(text: &str) -> Result<(), serde_json::Error> {
    serde_json::from_str::<std::collections::BTreeMap<String, workbench::Preferences>>(text)
        .map(|_| ())
}

/// Normalize project settings through their typed defaults for field-level merge comparisons.
pub(crate) fn project_preference_defaults(
    value: serde_json::Value,
) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::from_value::<std::collections::BTreeMap<String, workbench::Preferences>>(value)
        .and_then(serde_json::to_value)
}

/// Validated command-line/startup choices consumed when creating the native editor.
#[derive(Default)]
pub struct Startup {
    /// Optional existing local project directory or file; a file uses its containing directory as the workspace.
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

/// Execution context selected by this window. A connected transport is not a ready project:
/// file/tool operations wait for preparation to establish the actual plan.
struct Workspace {
    /// Project path interpreted on `remote`, or on the desktop for local workspaces.
    root: String,
    remote: Option<Remote>,
    /// Transport availability; this alone does not authorize project operations.
    connected: bool,
    /// Preparation established a root and actual execution plan for this context.
    ready: bool,
    /// Effective policy returned by preparation, including explicit unavailable-host fallback.
    plan: sandbox::Plan,
    /// Requested grants retained for forms and future preparation, not proof of isolation.
    sandbox_options: sandbox::Options,
}

pub use documents::Document;

#[derive(PartialEq)]
/// Exclusive native modal/picker intent; payloads retain the originating document identity.
enum Dialog {
    None,
    Connect,
    Folder,
    Open,
    AgentAttachLocal,
    AgentAttachHost,
    Save(u64),
    Commands,
    Sessions,
    Tunnels,
    Slurm,
    AgentSessions,
    TerminalExit,
    Shortcuts,
    Extensions,
    Encrypted,
    HexOpen,
    Explorer,
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
    editor: editor_windows::View,
    editors: editor_windows::Windows,
    navigation: navigation::State,
    workbench: Workbench,
    debugger: debugging::State,
    extensions: extensions::Extensions,
    workspace: workspace::Controller,
    documents: documents::Documents,
    /// Cleanup ownership survives notebook view replacement and tab removal.
    notebook_shutdowns: notebook_view::Shutdowns,
    /// Owns completion admission and the workspace clock independently of domain state.
    workers: workers::Inbox,
    explorer_state: explorer::State,
    terminal: Option<Terminal>,
    sessions: sessions::Sessions,
    tunnels: tunnels::Tunnels,
    ssh_master: Option<Terminal>,
    file_browser: file_browser::State,
    project_picker: project_dialog::State,
    project_bookmarks: project_bookmarks::State,
    sandbox_defaults: sandbox::Options,
    sandbox_presets: sandbox_presets::State,
    startup_restore: Option<bool>,
    last_window_title: String,
    show_terminal: bool,
    show_explorer: bool,
    show_hidden: bool,
    dialog: Dialog,
    dialog_focus: bool,
    dialog_events: Vec<egui::Event>,
    dialog_viewport: egui::ViewportId,
    ui_viewport: egui::ViewportId,
    dialog_input: String,
    marp_overwrite: bool,
    connection_profiles: saved_connections::State,
    pending: transitions::Controller,
    exit: lifecycle::Exit,
    notice: Option<(String, bool, Instant)>,
    status: String,
    font_size: f32,
    ui_scale: f32,
    ui_scale_save_pending: bool,
    themes: theme::Themes,
    terminal_colors: terminal_colors::State,
    theme_save_pending: bool,
    syntax_save_pending: bool,
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

    /// Initialize settings and inert domain state from a preference snapshot, then
    /// start only the explicit local-project or SSH startup preparation path.
    fn from_storage(
        cc: &eframe::CreationContext<'_>,
        startup: Startup,
        storage: Option<&dyn eframe::Storage>,
    ) -> Self {
        ui_probe::install(&cc.egui_ctx);
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
            editor: Default::default(),
            editors: Default::default(),
            navigation: navigation::State::load(storage),
            workbench: Workbench::load(storage),
            debugger: Default::default(),
            extensions: extensions::Extensions::load(storage),
            workspace: Default::default(),
            documents: Default::default(),
            notebook_shutdowns: Default::default(),
            workers: Default::default(),
            explorer_state: Default::default(),
            terminal: None,
            sessions: Default::default(),
            tunnels: tunnels::Tunnels::load(storage, &mut warnings),
            ssh_master: None,
            file_browser: Default::default(),
            project_picker: Default::default(),
            project_bookmarks: Default::default(),
            sandbox_defaults: storage
                .and_then(|s| s.get_string("sandbox_defaults"))
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            sandbox_presets: sandbox_presets::State::load(storage, &mut warnings),
            startup_restore: None,
            last_window_title: String::new(),
            show_terminal: false,
            show_explorer: true,
            show_hidden: false,
            dialog: Dialog::None,
            dialog_focus: false,
            dialog_events: vec![],
            dialog_viewport: egui::ViewportId::ROOT,
            ui_viewport: egui::ViewportId::ROOT,
            dialog_input: String::new(),
            marp_overwrite: false,
            connection_profiles: saved_connections::State::new(),
            pending: Default::default(),
            exit: Default::default(),
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
            started: Instant::now(),
        };
        if !warnings.is_empty() {
            app.notify(warnings.join("\n"), true);
        }
        if let Some(mut profile) = startup.remote {
            profile.sandbox.enabled |= startup.sandbox;
            app.connection_profiles.select(profile.clone(), false);
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

    /// Replace the current transient notice with a timestamped status or error message.
    fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some((text.into(), error, Instant::now()));
    }

    /// Merge this window's changed preferences through the shared persistence worker;
    /// framework GUI state uses the private snapshot instead of replacing shared settings.
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
        self.sandbox_presets.store(&mut storage);
        self.tunnels.store(&mut storage);
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

    /// Validate/probe a candidate asynchronously while retaining the current working project.
    /// Request the default nonpersistent native terminal after project preparation.
    fn start_project_terminal(&mut self, ctx: &egui::Context) {
        self.new_terminal(ctx);
    }

    /// Queue selected-host project discovery with a superseding folder token; this raw-host
    /// picker is separate from ordinary sandboxed document/file browsing.
    fn browse_folder(&mut self, path: String, ctx: &egui::Context) {
        let browse = self.project_picker.begin(path);
        self.dispatch_folder_browse(browse, ctx);
    }

    /// The picker owns request identity; only the coordinator can choose a host and do I/O.
    fn dispatch_folder_browse(&mut self, browse: project_dialog::Browse, ctx: &egui::Context) {
        if browse.remote
            && self
                .workspace
                .as_ref()
                .is_none_or(|w| !w.connected || w.remote.is_none())
        {
            self.project_picker.loading = false;
            self.project_picker.error = Some("SSH is offline. Reconnect to browse folders.".into());
            return;
        }
        let project_dialog::Browse {
            path,
            remote,
            token,
            options,
        } = browse;
        self.dispatch(
            Request::List { path: path.clone() },
            Job::Browse {
                path,
                remote,
                token,
                options,
            },
            ctx,
        );
    }

    /// Prefill the normal selected-host picker without changing the workspace.
    /// Resolve this folder's saved policy now so its first directory reply does
    /// not replace sandbox choices edited while the listing was loading.
    fn open_folder_dialog_at(&mut self, path: String) {
        self.set_dialog(Dialog::Folder);
        if let Some(options) = self.saved_sandbox_for(&path, self.project_picker.remote) {
            self.project_picker.sandbox = options;
        }
        self.project_picker.policy_path.clone_from(&path);
        self.project_picker.path = path;
    }

    /// Initialize the project picker explicitly on the desktop host, even during an SSH workspace.
    fn open_local_folder_dialog(&mut self) {
        self.set_dialog(Dialog::Folder);
        self.project_picker.remote = false;
        self.project_picker.path = std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .into();
        self.project_picker.needs_load = true;
    }

    /// Switch exclusive dialog ownership, resetting the relevant picker/form state
    /// and recording which native viewport must receive modal input.
    fn set_dialog(&mut self, dialog: Dialog) {
        if self.dialog == Dialog::Encrypted && dialog != Dialog::Encrypted && !self.vaults.locking()
        {
            self.cancel_encrypted_dialog();
        }
        if dialog == Dialog::Connect {
            self.connection_profiles.open(self.vaults.work());
        }
        self.navigation.list = Default::default();
        if file_browser::is_file_dialog(&dialog) {
            self.file_browser.reset();
        }
        self.dialog = dialog;
        self.dialog_events.clear();
        self.dialog_viewport = self.ui_viewport;
        self.dialog_focus = true;
        self.dialog_input = match self.dialog {
            Dialog::Folder => {
                let remote = self.workspace.as_ref().is_some_and(|w| w.remote.is_some());
                let options = self
                    .workspace
                    .as_ref()
                    .map(|w| w.sandbox_options.clone())
                    .unwrap_or_else(|| self.sandbox_defaults.clone());
                let policy_path = self
                    .workspace
                    .as_ref()
                    .map(|w| w.root.clone())
                    .unwrap_or_default();
                let path = self
                    .workspace
                    .as_ref()
                    .map(|w| w.root.clone())
                    .unwrap_or_else(|| {
                        std::env::current_dir()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into()
                    });
                self.project_picker.open(remote, path, policy_path, options);
                String::new()
            }
            Dialog::Save(id) => self
                .documents
                .iter()
                .find(|d| d.id == id)
                .and_then(|d| {
                    d.path.clone().or_else(|| {
                        d.recovery_path()
                            .map(|path| self.join_root(ssh::display_name(path)))
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

    /// Join relative paths to the selected host root; absolute/home-marked paths
    /// remain unchanged for expansion by the actual host backend.
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

    /// Create an unsaved source buffer with a new stable tab identity and request editor focus.
    fn new_file(&mut self) {
        self.editor_intent();
        self.documents.new_file();
    }

    /// Route the active editable document to its current destination or shared Save As picker.
    fn save_active(&mut self, ctx: &egui::Context, save_as: bool) {
        if let Some(doc) = self.documents.active() {
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

    /// Save named dirty documents through the same guarded host I/O from every UI entry point.
    fn save_all(&mut self, ctx: &egui::Context) {
        let files: Vec<_> = self
            .documents
            .iter()
            .filter(|document| document.dirty())
            .filter_map(|document| document.path.clone().map(|path| (document.id, path)))
            .collect();
        for (id, path) in files {
            self.save_document(id, path, false, ctx);
        }
        if self
            .documents
            .iter()
            .any(|document| document.path.is_none())
        {
            self.notify(
                "Save untitled files individually with Ctrl+S to choose their paths.",
                false,
            );
        }
    }

    /// Submit the exact document snapshot/revision through protected host I/O. Save As
    /// creates without replacing unrelated files; acknowledgements update only the submitted
    /// baseline.
    fn save_document(&mut self, id: u64, path: String, save_as: bool, ctx: &egui::Context) {
        if self.explorer_state.busy {
            self.notify(
                "Wait for the file operation to finish before saving.",
                false,
            );
            return;
        }
        if !save_as
            && self
                .documents
                .iter()
                .any(|doc| doc.id == id && doc.is_recovery())
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
        let Some(snapshot) = doc.begin_text_save(&path, save_as) else {
            return;
        };
        self.dispatch_text_save(id, path, snapshot.contents, snapshot.expected, ctx);
    }

    /// Select or hide the terminal surface, lazily creating a shell only when none is owned or starting.
    fn toggle_terminal(&mut self, ctx: &egui::Context) {
        if self.tab_detached(workbench::ToolTab::Terminal) {
            self.focus_tool_tab(workbench::ToolTab::Terminal, ctx);
            return;
        }
        self.show_terminal = !self.terminal_visible() || self.workbench.detached;
        if self.show_terminal {
            self.terminal_intent();
            if self.terminal.is_none() && self.ssh_master.is_none() && !self.session_busy() {
                self.start_project_terminal(ctx);
            }
            self.focus_pane(navigation::Pane::Terminal, ctx);
            Terminal::focus(ctx);
        } else {
            self.focus_editor(ctx);
        }
    }
}

/// Recognize the platform extension shortcut even when the native backend translates it
/// into Cut, consuming only that exact modified shortcut rather than ordinary clipboard Cut.
fn consume_extensions_shortcut(ctx: &egui::Context) -> bool {
    Command::Extensions.shortcut().is_some_and(|binding| {
        ctx.input_mut(|input| {
            // egui-winit maps Command+X to Cut even with Shift held. A same-frame
            // press/release may retain modifiers only on the release event.
            let modifiers = input
                .events
                .iter()
                .find_map(|event| match event {
                    egui::Event::Key { key, modifiers, .. } if *key == binding.logical_key => {
                        Some(*modifiers)
                    }
                    _ => None,
                })
                .unwrap_or(input.modifiers);
            let cut = modifiers.matches_exact(binding.modifiers)
                && input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Cut));
            if cut {
                input
                    .events
                    .retain(|event| !matches!(event, egui::Event::Cut));
            }
            input.consume_shortcut(&binding) || cut
        })
    })
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.ui_viewport = egui::ViewportId::ROOT;
        self.editors.input_frame_pending = true;
        self.activate_editor(self.focused_editor());
        self.poll(ctx);
        self.poll_vaults(ctx);
        self.poll_agents(ctx);
        self.poll_debugger(ctx);
        self.poll_notebooks(ctx);
        self.marp_tick();
        self.workbench_tick(ctx);

        self.session_tick(ctx);
        self.tunnel_tick(ctx);
        if ctx.input(|i| i.focused) && (self.workbench.detached || !self.tool_surface()) {
            self.remember_editor_focus(egui::ViewportId::ROOT);
        }
        self.in_editor(self.focused_editor(), |app| {
            app.route_input(ctx);
            if (app.dialog != Dialog::None || app.pending.modal() || app.workbench_modal())
                && Terminal::focused(ctx)
            {
                ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("terminal-input")));
            }
            app.navigation.begin_layout();
            app.titlebar(ctx);
            app.workspace_banner(ctx);
            app.encrypted_banner(ctx);
            app.statusbar(ctx);
            app.activity(ctx);
            app.explorer(ctx);
            app.workspace_content(ctx);
            app.dialogs(ctx);
            app.workbench_windows(ctx);
            app.extension_windows(ctx);
            app.latex_window(ctx);
            app.terminal_colors.show(ctx, app.themes.active());
            app.notice_ui(ctx);
            app.navigation_finish(ctx);
        });
        self.activate_editor(self.focused_editor());
        self.detached_tools(ctx);
        self.detached_tabs(ctx);
        self.detached_editors(ctx);
        // Prepared replies and UI intents must see every editor's accepted edits.
        self.editors.input_frame_pending = false;
        self.in_editor(egui::ViewportId::ROOT, |app| app.handle_close_request(ctx));
        self.activate_editor(self.focused_editor());
        if let Some(remote) = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.remote.as_ref())
        {
            ui_probe::ssh_master_socket(ctx, &remote.socket);
        }
        if self.connection_profiles.reloading() {
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
                self.sandbox_presets.store(storage);
                self.tunnels.store(storage);
                self.navigation.bindings.store(storage);
                self.terminal_colors.store(storage);
            }
        }
        // Optional automation hook for a bounded native-window smoke test.
        if std::env::var_os("RODE_SMOKE_TEST").is_some() {
            if self.started.elapsed() > Duration::from_secs(5) {
                self.exit.close();
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
        self.sandbox_presets.store(storage);
        self.tunnels.store(storage);
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
        self.connection_profiles.join_writer();
        self.reset_workspace(workspace::LeaveReason::WindowClosed);
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
mod tests;
