//! Shared native directory rows. Project-folder discovery and project file I/O
//! deliberately retain their separate host/sandbox scopes.
//!
//! Listings are asynchronous, nonrecursive and generation/token checked. Project discovery
//! uses the selected host; file/hex/Save As operations keep the active sandbox. Directory
//! results never replace a filename being typed, and the list focus host survives loading.
use super::*;

#[derive(Default)]
/// One modal browser lifetime. `directory` is the last successful listing; `location` is
/// editable and `requested` identifies the in-flight path. Keep them distinct so slow results
/// do not overwrite typed destinations.
pub(super) struct State {
    pub token: u64,
    pub directory: String,
    pub location: String,
    pub requested: String,
    pub entries: Vec<Entry>,
    pub loading: bool,
    pub needs_load: bool,
    pub error: Option<String>,
    query: String,
    rows: Vec<Entry>,
    dirty: bool,
    selected: Option<String>,
}
impl State {
    pub fn reset(&mut self) {
        let token = self.token.wrapping_add(1);
        *self = Self {
            token,
            needs_load: true,
            dirty: true,
            ..Default::default()
        };
    }
    pub fn loaded(&mut self, response: files::Response) {
        self.directory = response.path;
        self.location.clone_from(&self.directory);
        self.entries = response.entries;
        self.loading = false;
        self.error = None;
        self.dirty = true;
    }
    fn rows(&mut self, directories: &HashMap<String, Vec<Entry>>, documents: &[Document]) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.rows.clear();
        if self.query.is_empty() {
            self.rows.clone_from(&self.entries);
            return;
        }
        let query = self.query.to_lowercase();
        let mut seen = HashSet::new();
        for entry in self.entries.iter().chain(directories.values().flatten()) {
            if self.rows.len() >= files::MAX_ENTRIES {
                break;
            }
            if entry.path.to_lowercase().contains(&query) && seen.insert(entry.path.clone()) {
                self.rows.push(entry.clone());
            }
        }
        for doc in documents {
            if self.rows.len() >= files::MAX_ENTRIES {
                break;
            }
            if let Some(path) = &doc.path
                && path.to_lowercase().contains(&query)
                && seen.insert(path.clone())
            {
                self.rows.push(Entry {
                    path: path.clone(),
                    name: ssh::display_name(path).into(),
                    dir: false,
                });
            }
        }
        self.rows.sort_by(|a, b| a.path.cmp(&b.path));
        let prefix = format!("{}/", self.directory.trim_end_matches('/'));
        for entry in &mut self.rows {
            entry.name = entry
                .path
                .strip_prefix(&prefix)
                .unwrap_or(&entry.path)
                .into();
        }
    }
    pub fn invalidate_rows(&mut self) {
        self.dirty = true;
    }
    /// Accept a directory reply only for this picker token and unchanged location text.
    pub fn matches(&self, token: u64, path: &str) -> bool {
        self.token == token && self.requested == path && self.location == path
    }
}
#[derive(Clone)]
pub(super) enum Choice {
    Select(Entry),
    Open(Entry),
    Parent,
}

fn shown(entry: &Entry, show_hidden: bool) -> bool {
    show_hidden || !ssh::display_name(&entry.path).starts_with('.')
}

/// Both pickers use this row renderer, including full-row hit targets, keyboard
/// selection and clipped, left-aligned labels. File names have no type prefixes.
#[allow(clippy::too_many_arguments)] // One shared renderer for project and file dialog state.
pub(super) fn rows(
    ui: &mut egui::Ui,
    id: &str,
    entries: &[Entry],
    show_hidden: bool,
    list: &mut navigation::List,
    query: &str,
    path_keys: (isize, bool),
    selected: Option<&str>,
    ready: bool,
) -> Option<Choice> {
    let visible: Vec<_> = entries
        .iter()
        .filter(|entry| shown(entry, show_hidden))
        .collect();
    let host = ui.make_persistent_id((id, "focus"));
    let ctx = ui.ctx().clone();
    let enabled = ui.is_enabled() && ready;
    let mut delta = if enabled { path_keys.0 } else { 0 };
    let mut activate = false;
    let mut parent = false;
    let mut edge = None;
    if enabled && ctx.memory(|m| m.has_focus(host)) {
        ctx.memory_mut(|m| {
            m.set_focus_lock_filter(
                host,
                egui::EventFilter {
                    tab: false,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: false,
                },
            )
        });
        let keys = navigation::List::keys(&ctx);
        delta += keys.0;
        activate |= keys.1;
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, Key::Home) {
                edge = Some(0);
            }
            if i.consume_key(egui::Modifiers::NONE, Key::End) {
                edge = Some(visible.len().saturating_sub(1));
            }
            if i.consume_key(egui::Modifiers::NONE, Key::PageDown) {
                delta += 10;
            }
            if i.consume_key(egui::Modifiers::NONE, Key::PageUp) {
                delta -= 10;
            }
            parent = i.consume_key(egui::Modifiers::NONE, Key::Backspace)
                || i.consume_key(egui::Modifiers::NONE, Key::ArrowLeft);
            activate |= i.consume_key(egui::Modifiers::NONE, Key::ArrowRight);
        });
    }
    let mut reveal = list.update(query, visible.len(), 0);
    if let Some(path) = selected
        && let Some(index) = visible.iter().position(|entry| entry.path == path)
    {
        list.index = index;
        list.navigated = true;
    }
    reveal |= list.update(query, visible.len(), delta);
    if let Some(index) = edge {
        list.index = index;
        list.navigated = true;
        reveal = true;
    }
    activate |= enabled && path_keys.1 && list.navigated;
    let height = (ctx.content_rect().height() - 390.).clamp(70., 220.);
    ui.interact(
        egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), height)),
        host,
        Sense::focusable_noninteractive(),
    );
    let mut action = if parent {
        Some(Choice::Parent)
    } else if activate {
        visible
            .get(list.index)
            .map(|entry| Choice::Open((*entry).clone()))
    } else {
        None
    };
    let scroll_id = ui.make_persistent_id(id);
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(height)
        .auto_shrink([false, false]);
    if reveal {
        let top = list.index as f32 * 27.;
        let offset =
            egui::scroll_area::State::load(&ctx, scroll_id).map_or(0., |state| state.offset.y);
        if top < offset {
            scroll = scroll.vertical_scroll_offset(top);
        } else if top + 27. > offset + height {
            scroll = scroll.vertical_scroll_offset((top + 27. - height).max(0.));
        }
    }
    let spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 2.;
    scroll.show_rows(ui, 25., visible.len(), |ui, range| {
        // Disable stale entries while navigating, but retain the list's focus
        // host above so keyboard browsing survives asynchronous directory reads.
        if !ready {
            ui.disable();
        }
        ui.spacing_mut().item_spacing.y = 2.;
        for index in range {
            let entry = visible[index];
            let label = if entry.dir {
                format!("›  {}", entry.name)
            } else {
                entry.name.clone()
            };
            let response = ui
                .push_id(&entry.path, |ui| {
                    ui.add_sized(
                        [ui.available_width(), 25.],
                        egui::Button::selectable(
                            list.navigated && index == list.index,
                            (label, egui::Atom::grow()),
                        )
                        .truncate()
                        .sense(Sense::click().difference(Sense::focusable_noninteractive())),
                    )
                })
                .inner
                .on_hover_text(&entry.path);
            if response.clicked() {
                list.index = index;
                list.navigated = true;
                ctx.memory_mut(|m| m.request_focus(host));
                action = Some(if response.double_clicked() {
                    Choice::Open(entry.clone())
                } else {
                    Choice::Select(entry.clone())
                });
            }
        }
    });
    ui.spacing_mut().item_spacing.y = spacing;
    action
}

pub(super) fn is_file_dialog(dialog: &Dialog) -> bool {
    matches!(
        dialog,
        Dialog::Open
            | Dialog::HexOpen
            | Dialog::Save(_)
            | Dialog::AgentAttachLocal
            | Dialog::AgentAttachHost
    )
}
fn parent(path: &str) -> String {
    std::path::Path::new(path)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map_or_else(|| "/".into(), |path| path.to_string_lossy().into())
}

impl EditorApp {
    pub(super) fn browse_files(&mut self, path: String, ctx: &egui::Context) {
        let browser = &mut self.file_browser;
        browser.token = browser.token.wrapping_add(1);
        browser.loading = true;
        browser.error = None;
        browser.requested = path.clone();
        browser.location = path.clone();
        let token = browser.token;
        self.dispatch(
            Request::List { path: path.clone() },
            Job::BrowseFiles {
                token,
                path,
                local: self.dialog == Dialog::AgentAttachLocal,
            },
            ctx,
        );
    }
    fn picker_path(&self, path: &str) -> String {
        if path.starts_with('/') || path.starts_with('~') {
            path.into()
        } else {
            let directory = if self.file_browser.loading {
                &self.file_browser.requested
            } else {
                &self.file_browser.directory
            };
            format!("{}/{}", directory.trim_end_matches('/'), path)
        }
    }
    fn picker_directory(&mut self, path: String, ctx: &egui::Context) {
        self.dialog_input = if matches!(self.dialog, Dialog::Save(_)) {
            ssh::display_name(&self.dialog_input).into()
        } else {
            String::new()
        };
        self.file_browser.query.clear();
        self.file_browser.selected = None;
        self.file_browser.dirty = true;
        self.browse_files(path, ctx);
    }
    pub(super) fn file_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        if self.file_browser.needs_load {
            self.file_browser.needs_load = false;
            let directory = if matches!(self.dialog, Dialog::Save(_))
                && self.dialog_input.starts_with('/')
            {
                parent(&self.dialog_input)
            } else {
                self.workspace
                    .as_ref()
                    .filter(|w| self.dialog != Dialog::AgentAttachLocal || w.remote.is_none())
                    .map(|w| w.root.clone())
                    .or_else(|| {
                        self.documents
                            .get(self.active)
                            .filter(|_| {
                                self.dialog != Dialog::AgentAttachLocal
                                    || self.workspace.as_ref().is_none_or(|w| w.remote.is_none())
                            })
                            .and_then(|d| d.path.as_deref())
                            .map(parent)
                    })
                    .unwrap_or_else(|| {
                        std::env::current_dir()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into()
                    })
            };
            self.file_browser.directory = directory.clone();
            if let Some(entries) = self.directories.get(&directory).filter(|_| {
                self.dialog != Dialog::AgentAttachLocal
                    || self.workspace.as_ref().is_none_or(|w| w.remote.is_none())
            }) {
                self.file_browser.entries.clone_from(entries);
            }
            self.browse_files(directory, ctx);
        }
        let host = self
            .workspace
            .as_ref()
            .filter(|_| self.dialog != Dialog::AgentAttachLocal)
            .and_then(|w| w.remote.as_ref())
            .map_or_else(
                || "THIS MACHINE".into(),
                |r| format!("SSH · {}", r.profile.endpoint()),
            );
        let host = if let Some(workspace) = self
            .workspace
            .as_ref()
            .filter(|_| self.dialog != Dialog::AgentAttachLocal)
        {
            format!("{host} · {}", workspace.plan.label())
        } else {
            host
        };
        ui.label(palette.tag(host));
        if matches!(
            self.dialog,
            Dialog::AgentAttachLocal | Dialog::AgentAttachHost
        ) {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.dialog == Dialog::AgentAttachLocal, "This computer")
                    .clicked()
                {
                    self.set_dialog(Dialog::AgentAttachLocal);
                }
                if self.workspace.as_ref().is_some_and(|w| w.remote.is_some())
                    && ui
                        .selectable_label(self.dialog == Dialog::AgentAttachHost, "Project host")
                        .clicked()
                {
                    self.set_dialog(Dialog::AgentAttachHost);
                }
            });
        }
        ui.heading(match self.dialog {
            Dialog::AgentAttachLocal | Dialog::AgentAttachHost => "Attach image or document",
            Dialog::Save(_) => "Save file as",
            Dialog::HexOpen => "Open as hex / inspect binary",
            _ => "Open file",
        });
        ui.label(
            RichText::new(if matches!(self.dialog, Dialog::Save(_)) {
                "Browse folders or enter a destination path. Select a file to reuse its name."
            } else {
                "Browse folders or enter a path. Double-click folders and files to open them."
            })
            .small()
            .color(palette.muted),
        );
        let path_id = egui::Id::new("file-dialog-path");
        let keys = if ctx.memory(|m| m.has_focus(path_id)) {
            navigation::List::keys(ctx)
        } else {
            (0, false)
        };
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.dialog_input)
                .id(path_id)
                .desired_width(f32::INFINITY)
                .hint_text(if matches!(self.dialog, Dialog::Save(_)) {
                    "File name or path…"
                } else {
                    "File name, search, or path…"
                }),
        );
        if self.dialog_focus {
            response.request_focus();
            self.dialog_focus = false;
        }
        if response.changed() {
            self.file_browser.query = self.dialog_input.clone();
            self.file_browser.selected = None;
            self.file_browser.dirty = true;
        }
        ui.add_space(4.);
        let mut browse = None;
        ui.horizontal(|ui| {
            if ui.button("Up").clicked() {
                browse = Some(parent(&self.file_browser.directory));
            }
            let location = ui.add(
                egui::TextEdit::singleline(&mut self.file_browser.location)
                    .desired_width((ui.available_width() - 155.).max(80.))
                    .hint_text("Folder path"),
            );
            if location.changed() && self.file_browser.loading {
                self.file_browser.token = self.file_browser.token.wrapping_add(1);
                self.file_browser.loading = false;
            }
            let enter = location.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if ui.button("Browse").clicked() || enter {
                browse = Some(self.picker_path(&self.file_browser.location));
            }
            if ui.checkbox(&mut self.show_hidden, "Hidden").changed() {
                self.tree_dirty = true;
            }
        });
        if self.file_browser.loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(palette.tag("Loading…"));
            });
        }
        if let Some(error) = &self.file_browser.error {
            ui.colored_label(palette.error, error);
        }
        if self.dialog == Dialog::AgentAttachLocal
            && self.workspace.as_ref().is_some_and(|w| w.remote.is_some())
        {
            self.file_browser.rows(&HashMap::new(), &[]);
        } else {
            self.file_browser.rows(&self.directories, &self.documents);
        }
        let query = format!(
            "{}:{}:{}",
            self.file_browser.token, self.file_browser.query, self.show_hidden
        );
        let selected = self.file_browser.selected.take();
        let cached = self.file_browser.requested == self.file_browser.directory;
        let action = rows(
            ui,
            "file-picker-rows",
            &self.file_browser.rows,
            self.show_hidden,
            &mut self.navigation.list,
            &query,
            keys,
            selected.as_deref(),
            !self.file_browser.loading || cached,
        );
        if self.navigation.list.navigated {
            self.file_browser.selected = self
                .file_browser
                .rows
                .iter()
                .filter(|e| shown(e, self.show_hidden))
                .nth(self.navigation.list.index)
                .map(|entry| entry.path.clone());
        }
        if self
            .file_browser
            .rows
            .iter()
            .all(|e| !shown(e, self.show_hidden))
            && !self.file_browser.loading
        {
            ui.label(palette.tag(if self.file_browser.query.is_empty() {
                "No visible entries in this folder."
            } else {
                "No matching entries. Any exact file path can be opened."
            }));
        }
        let mut open = None;
        if let Some(action) = action {
            match action {
                Choice::Parent => browse = Some(parent(&self.file_browser.directory)),
                Choice::Select(entry) if entry.dir => self.file_browser.selected = Some(entry.path),
                Choice::Open(entry) if entry.dir => browse = Some(entry.path),
                Choice::Select(entry) | Choice::Open(entry)
                    if matches!(self.dialog, Dialog::Save(_)) =>
                {
                    self.dialog_input = entry.path.clone();
                    self.file_browser.selected = Some(entry.path);
                    self.file_browser.query.clear();
                    self.file_browser.dirty = true;
                }
                Choice::Open(entry) => open = Some(entry.path),
                Choice::Select(entry) => {
                    self.dialog_input = entry.path.clone();
                    self.file_browser.selected = Some(entry.path);
                    self.file_browser.query.clear();
                    self.file_browser.dirty = true;
                }
            }
        }
        ui.add_space(8.);
        ui.horizontal(|ui| {
            let label = match self.dialog {
                Dialog::Save(_) => "Save",
                Dialog::HexOpen => "Open as hex",
                Dialog::AgentAttachLocal | Dialog::AgentAttachHost => "Attach",
                _ => "Open",
            };
            if palette.primary(ui, label).clicked() || keys.1 {
                let selected = self
                    .file_browser
                    .rows
                    .iter()
                    .filter(|e| shown(e, self.show_hidden))
                    .nth(self.navigation.list.index);
                if self.navigation.list.navigated
                    && let Some(entry) = selected
                {
                    if entry.dir {
                        browse = Some(entry.path.clone());
                    } else {
                        open = Some(entry.path.clone());
                    }
                } else if !self.dialog_input.trim().is_empty() {
                    let path = self.picker_path(&self.dialog_input);
                    if self
                        .file_browser
                        .entries
                        .iter()
                        .any(|e| e.dir && e.path == path)
                        || self.dialog_input.ends_with('/')
                    {
                        browse = Some(path);
                    } else {
                        open = Some(path);
                    }
                }
            }
            if ui.button("Cancel").clicked() {
                self.dialog = Dialog::None;
            }
        });
        if let Some(path) = browse {
            self.picker_directory(path, ctx);
        } else if let Some(path) = open.filter(|_| is_file_dialog(&self.dialog)) {
            let dialog = std::mem::replace(&mut self.dialog, Dialog::None);
            match dialog {
                Dialog::Save(id) => {
                    self.save_document(id, path, true, ctx);
                    self.focus_editor(ctx);
                }
                Dialog::HexOpen => self.open_hex(path, ctx),
                Dialog::AgentAttachLocal | Dialog::AgentAttachHost => {
                    self.attach_agent_file(path, dialog == Dialog::AgentAttachLocal, ctx)
                }
                _ => self.open_file(path, ctx),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> (EditorApp, egui::Context) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        (EditorApp::new(&cc, Startup::default()), ctx)
    }
    #[test]
    fn relative_paths_follow_the_browsed_directory_and_keep_absolute_and_home_paths() {
        let (mut app, _) = app();
        app.set_dialog(Dialog::Open);
        app.file_browser.directory = "/project/sub".into();
        assert_eq!(app.picker_path("test.rs"), "/project/sub/test.rs");
        assert_eq!(app.picker_path("~/test.rs"), "~/test.rs");
        assert_eq!(app.picker_path("/elsewhere/file"), "/elsewhere/file");
        app.file_browser.loading = true;
        app.file_browser.requested = "/project/other".into();
        assert_eq!(app.picker_path("new.txt"), "/project/other/new.txt");
    }
    #[test]
    fn directory_results_cannot_replace_another_picker_or_a_new_location() {
        let (mut app, ctx) = app();
        app.set_dialog(Dialog::Open);
        app.file_browser.requested = "/first".into();
        app.file_browser.location = "/first".into();
        let token = app.file_browser.token;
        app.set_dialog(Dialog::HexOpen);
        app.dialog_input = "keep.bin".into();
        app.tx
            .send(Event::Done {
                generation: app.generation,
                job: Job::BrowseFiles {
                    token,
                    local: false,
                    path: "/first".into(),
                },
                result: Box::new(Ok(files::Response {
                    path: "/stale".into(),
                    ..Default::default()
                })),
            })
            .unwrap();
        app.poll(&ctx);
        assert_eq!(app.dialog_input, "keep.bin");
        assert!(app.file_browser.directory.is_empty());
        let token = app.file_browser.token;
        app.file_browser.requested = "/old".into();
        app.file_browser.location = "/new".into();
        app.tx
            .send(Event::Done {
                generation: app.generation,
                job: Job::BrowseFiles {
                    token,
                    local: false,
                    path: "/old".into(),
                },
                result: Box::new(Ok(files::Response {
                    path: "/stale".into(),
                    ..Default::default()
                })),
            })
            .unwrap();
        app.poll(&ctx);
        assert_eq!(app.file_browser.location, "/new");
    }
    #[test]
    fn directory_results_preserve_typed_names_and_project_folder_policies() {
        let (mut app, ctx) = app();
        app.set_dialog(Dialog::Save(42));
        app.dialog_input = "new-name.txt".into();
        app.folder_sandbox.enabled = true;
        app.file_browser.requested = "/project".into();
        app.file_browser.location = "/project".into();
        let token = app.file_browser.token;
        app.tx
            .send(Event::Done {
                generation: app.generation,
                job: Job::BrowseFiles {
                    token,
                    local: false,
                    path: "/project".into(),
                },
                result: Box::new(Ok(files::Response {
                    path: "/canonical/project".into(),
                    entries: vec![Entry {
                        path: "/canonical/project/file.txt".into(),
                        name: "file.txt".into(),
                        dir: false,
                    }],
                    ..Default::default()
                })),
            })
            .unwrap();
        app.poll(&ctx);
        assert_eq!(app.dialog_input, "new-name.txt");
        assert_eq!(app.file_browser.directory, "/canonical/project");
        assert!(app.folder_sandbox.enabled);
    }
    #[test]
    fn file_listing_waits_for_the_project_policy_instead_of_using_raw_host_access() {
        let (mut app, ctx) = app();
        app.preparing = true;
        app.set_dialog(Dialog::Open);
        app.browse_files("/project".into(), &ctx);
        assert!(!app.file_browser.loading);
        assert!(
            app.file_browser
                .error
                .as_ref()
                .is_some_and(|e| e.contains("environment"))
        );
        assert!(app.rx.try_recv().is_err());
    }
    #[test]
    fn loading_disables_stale_rows_without_losing_keyboard_focus() {
        let ctx = egui::Context::default();
        let mut list = navigation::List::default();
        let entries = vec![Entry {
            path: "/project/note.txt".into(),
            name: "note.txt".into(),
            dir: false,
        }];
        for frame in 0..3 {
            let mut focus = None;
            let mut choice = None;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800., 600.),
                    )),
                    events: if frame == 0 {
                        vec![]
                    } else {
                        vec![egui::Event::Key {
                            key: Key::Enter,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }]
                    },
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let host = ui.make_persistent_id(("test-rows", "focus"));
                        focus = Some(host);
                        if frame == 0 {
                            ctx.memory_mut(|m| m.request_focus(host));
                        }
                        choice = rows(
                            ui,
                            "test-rows",
                            &entries,
                            false,
                            &mut list,
                            "",
                            (0, false),
                            None,
                            frame != 1,
                        );
                    });
                },
            );
            assert!(ctx.memory(|m| m.has_focus(focus.unwrap())));
            if frame == 2 {
                assert!(matches!(choice, Some(Choice::Open(e)) if e.path == entries[0].path));
            } else {
                assert!(choice.is_none());
            }
        }
    }
    #[test]
    fn cached_matches_are_deduplicated_disambiguated_and_keep_hidden_names_hidden() {
        let mut state = State {
            directory: "/project".into(),
            query: ".rs".into(),
            dirty: true,
            entries: vec![Entry {
                path: "/project/main.rs".into(),
                name: "main.rs".into(),
                dir: false,
            }],
            ..Default::default()
        };
        let dirs = HashMap::from([(
            "/project".into(),
            vec![
                Entry {
                    path: "/project/main.rs".into(),
                    name: "main.rs".into(),
                    dir: false,
                },
                Entry {
                    path: "/project/sub/main.rs".into(),
                    name: "main.rs".into(),
                    dir: false,
                },
                Entry {
                    path: "/project/sub/.hidden.rs".into(),
                    name: ".hidden.rs".into(),
                    dir: false,
                },
            ],
        )]);
        state.rows(&dirs, &[]);
        assert_eq!(state.rows.len(), 3);
        assert!(state.rows.iter().any(|e| e.name == "sub/main.rs"));
        assert_eq!(state.rows.iter().filter(|e| shown(e, false)).count(), 2);
    }
}
