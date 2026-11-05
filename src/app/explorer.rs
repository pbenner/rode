//! Explorer rows and explicit project-host file operations. Menus dispatch typed
//! filesystem requests or guarded editor actions; blocking I/O stays on workers.
use super::*;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct State {
    form: Option<Form>,
    pub(super) busy: bool,
    /// Directory replies captured before a mutation must not restore stale rows.
    pub(super) epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    File,
    Folder,
    Rename,
    Duplicate,
    Delete,
}

impl Kind {
    fn title(self) -> &'static str {
        match self {
            Self::File => "New file",
            Self::Folder => "New folder",
            Self::Rename => "Rename",
            Self::Duplicate => "Duplicate",
            Self::Delete => "Delete",
        }
    }
}

struct Form {
    kind: Kind,
    entry: Entry,
    generation: u64,
}

/// Captured completion intent; paths are updated only after host acknowledgement.
pub(super) struct Change {
    kind: Kind,
    entry: Entry,
}

fn within(path: &str, entry: &Entry) -> bool {
    path == entry.path || (entry.dir && std::path::Path::new(path).starts_with(&entry.path))
}

fn renamed(path: &str, entry: &Entry, destination: &str) -> Option<String> {
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

fn valid_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 255
        && !name.contains(['/', '\0'])
}

/// A rename changes paths, not directory contents. Keep unrelated listings and
/// remap loaded descendants rather than launching a worker for every open node.
fn rename_directories(
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

impl EditorApp {
    fn explorer_writable(&self) -> bool {
        self.project_ready()
            && !self.preparing
            && !self.explorer_state.busy
            && self.opening.is_empty()
            && !self.documents.iter().any(|doc| doc.saving)
            && self.workspace.as_ref().is_some_and(|workspace| {
                !(workspace.plan.active && workspace.plan.project_read_only)
            })
    }

    /// A deleted entry can still own an open buffer. Do not create a second
    /// binding or silently reopen retained contents under a newly reused name.
    fn explorer_destination_in_use(&self, destination: &std::path::Path) -> bool {
        self.documents.iter().any(|doc| {
            doc.path
                .as_ref()
                .is_some_and(|path| std::path::Path::new(path).starts_with(destination))
        })
    }

    fn explorer_form(&mut self, kind: Kind, entry: Entry) {
        self.explorer_state.form = Some(Form {
            kind,
            entry: entry.clone(),
            generation: self.generation,
        });
        self.set_dialog(Dialog::Explorer);
        self.dialog_input = match kind {
            Kind::Rename => entry.name,
            Kind::Duplicate => {
                let path = std::path::Path::new(&entry.name);
                if !entry.dir
                    && let (Some(stem), Some(extension)) = (path.file_stem(), path.extension())
                {
                    format!(
                        "{} copy.{}",
                        stem.to_string_lossy(),
                        extension.to_string_lossy()
                    )
                } else {
                    format!("{} copy", entry.name)
                }
            }
            Kind::File => "untitled.txt".into(),
            Kind::Folder => "New folder".into(),
            Kind::Delete => String::new(),
        };
    }

    fn explorer_menu(&mut self, ui: &mut egui::Ui, entry: &Entry, root: bool, ctx: &egui::Context) {
        let ready = self.project_ready() && !self.explorer_state.busy;
        let writable = self.explorer_writable();
        let item = |ui: &mut egui::Ui, name: &str, label: &str, enabled| {
            let response = ui.add_enabled(enabled, egui::Button::new(label));
            ui_probe::button(ui, name, &response);
            if response.clicked() {
                ui.close();
                true
            } else {
                false
            }
        };
        if entry.dir {
            if item(
                ui,
                "explorer-open-project",
                "Open as project",
                ready && !root,
            ) {
                let remote = self.workspace.as_ref().is_some_and(|w| w.remote.is_some());
                let options = self
                    .saved_sandbox_for(&entry.path, remote)
                    .or_else(|| self.workspace.as_ref().map(|w| w.sandbox_options.clone()))
                    .unwrap_or_else(|| self.sandbox_defaults.clone());
                self.guarded(
                    Action::Project {
                        path: entry.path.clone(),
                        remote,
                        options,
                    },
                    ctx,
                );
            }
        } else {
            if item(ui, "explorer-open", "Open", ready) {
                self.open_file(entry.path.clone(), ctx);
            }
            if item(
                ui,
                "explorer-open-hex",
                "Open as hex / inspect binary",
                ready,
            ) {
                self.open_hex(entry.path.clone(), ctx);
            }
        }
        ui.separator();
        let parent = if entry.dir {
            entry.clone()
        } else {
            Entry {
                path: std::path::Path::new(&entry.path)
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("/"))
                    .to_string_lossy()
                    .into(),
                name: String::new(),
                dir: true,
            }
        };
        for (kind, name, label) in [
            (Kind::File, "explorer-new-file", "New file…"),
            (Kind::Folder, "explorer-new-folder", "New folder…"),
        ] {
            if item(ui, name, label, writable) {
                self.explorer_form(kind, parent.clone());
            }
        }
        if !root {
            ui.separator();
            for (kind, name, label) in [
                (Kind::Rename, "explorer-rename", "Rename…"),
                (Kind::Duplicate, "explorer-duplicate", "Duplicate…"),
                (Kind::Delete, "explorer-delete", "Delete…"),
            ] {
                if item(ui, name, label, writable) {
                    self.explorer_form(kind, entry.clone());
                }
            }
        }
        ui.separator();
        if item(ui, "explorer-copy-path", "Copy path", true) {
            ctx.copy_text(entry.path.clone());
        }
        if item(ui, "explorer-copy-relative", "Copy relative path", true) {
            let relative = self
                .workspace
                .as_ref()
                .and_then(|workspace| {
                    std::path::Path::new(&entry.path)
                        .strip_prefix(&workspace.root)
                        .ok()
                })
                .map(|path| {
                    if path.as_os_str().is_empty() {
                        ".".into()
                    } else {
                        path.to_string_lossy().into_owned()
                    }
                })
                .unwrap_or_else(|| entry.path.clone());
            ctx.copy_text(relative);
        }
        if item(ui, "explorer-refresh", "Refresh", ready) {
            self.list(parent.path, ctx);
        }
    }

    /// File-operation dialogs retain their originating workspace and literal name.
    pub(super) fn explorer_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(form) = &self.explorer_state.form else {
            self.dialog = Dialog::None;
            return;
        };
        let kind = form.kind;
        let entry = form.entry.clone();
        let current = form.generation == self.generation;
        let palette = self.themes.active().palette;
        layout::heading(ui, kind.title(), &entry.path, palette);
        if kind == Kind::Delete {
            ui.label(if entry.dir {
                "Delete this folder and all of its contents? This cannot be undone."
            } else {
                "Delete this file? This cannot be undone."
            });
            ui.label("Open editors are kept. Use Save As to retain their contents.");
        } else {
            let id = egui::Id::new("explorer-operation-name");
            if self.dialog_focus || ui.memory(|memory| memory.focused().is_none()) {
                ui.memory_mut(|memory| memory.request_focus(id));
            }
            if self.dialog_focus {
                let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(self.dialog_input.chars().count()),
                    )));
                state.store(ctx, id);
                self.dialog_focus = false;
            }
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.dialog_input)
                    .id(id)
                    .desired_width(f32::INFINITY)
                    .char_limit(255),
            );
            ui_probe::button(ui, "explorer-name", &response);
            if !valid_name(&self.dialog_input) {
                ui.colored_label(
                    palette.error,
                    "Enter one name of at most 255 bytes. It cannot contain / or be . or ..",
                );
            }
            if kind == Kind::Duplicate {
                ui.label(
                    "Copies the saved contents on disk. Existing destinations are never replaced.",
                );
            }
        }
        let destination = match kind {
            Kind::File | Kind::Folder => std::path::Path::new(&entry.path).join(&self.dialog_input),
            Kind::Delete => std::path::PathBuf::from(&entry.path),
            Kind::Rename | Kind::Duplicate => {
                std::path::Path::new(&entry.path).with_file_name(&self.dialog_input)
            }
        };
        let in_use = kind != Kind::Delete
            && valid_name(&self.dialog_input)
            && !(kind == Kind::Rename && self.dialog_input == entry.name)
            && self.explorer_destination_in_use(&destination);
        if in_use {
            ui.colored_label(palette.error, "An open editor already uses this destination. Save it under another name or close it before reusing the name.");
        }
        ui.add_space(12.);
        let allowed = current
            && self.explorer_writable()
            && !in_use
            && (kind == Kind::Delete
                || (valid_name(&self.dialog_input)
                    && (kind != Kind::Rename || self.dialog_input != entry.name)));
        let enter = !ui.is_sizing_pass()
            && allowed
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter));
        let mut apply = false;
        ui.horizontal(|ui| {
            let button = ui.add_enabled(allowed, egui::Button::new(kind.title()));
            if kind == Kind::Delete
                && (self.dialog_focus || ui.memory(|memory| memory.focused().is_none()))
            {
                button.request_focus();
                self.dialog_focus = false;
            }
            ui_probe::button(ui, "explorer-apply", &button);
            apply = button.clicked() || enter;
            let cancel = ui.button("Cancel");
            ui_probe::button(ui, "explorer-cancel", &cancel);
            if cancel.clicked() {
                self.dialog = Dialog::None;
            }
        });
        if apply {
            let destination = destination.to_string_lossy().into_owned();
            let request = match kind {
                Kind::File => Request::Write {
                    path: destination,
                    text: String::new(),
                    expected: None,
                },
                Kind::Folder => Request::Mutate {
                    mutation: files::Mutation::CreateDirectory { path: destination },
                },
                Kind::Rename => Request::Mutate {
                    mutation: files::Mutation::Rename {
                        path: entry.path.clone(),
                        destination,
                    },
                },
                Kind::Duplicate => Request::Mutate {
                    mutation: files::Mutation::Copy {
                        path: entry.path.clone(),
                        destination,
                    },
                },
                Kind::Delete => Request::Mutate {
                    mutation: files::Mutation::Remove {
                        path: entry.path.clone(),
                    },
                },
            };
            self.dialog = Dialog::None;
            self.explorer_state.form = None;
            self.explorer_state.epoch = self.explorer_state.epoch.wrapping_add(1);
            self.loading.clear();
            self.explorer_state.busy = true;
            self.status = format!("{}…", kind.title());
            self.dispatch(request, Job::Explorer(Change { kind, entry }), ctx);
        }
    }

    pub(super) fn finish_explorer(
        &mut self,
        change: Change,
        result: Result<Response, String>,
        ctx: &egui::Context,
    ) {
        self.explorer_state.busy = false;
        self.explorer_state.epoch = self.explorer_state.epoch.wrapping_add(1);
        self.loading.clear();
        let mut open = None;
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
                    for doc in &mut self.documents {
                        if let Some(path) = doc
                            .path
                            .as_deref()
                            .and_then(|path| renamed(path, &change.entry, &response.path))
                        {
                            doc.path = Some(path);
                            doc.edit_version = doc.edit_version.wrapping_add(1);
                            if let Some(preview) = &mut doc.markdown {
                                preview.invalidate();
                            }
                            if let Some(view) = &mut doc.marp {
                                view.invalidate();
                            }
                        }
                    }
                    self.expanded = self
                        .expanded
                        .iter()
                        .map(|path| {
                            renamed(path, &change.entry, &response.path)
                                .unwrap_or_else(|| path.clone())
                        })
                        .collect();
                    if let Some(path) = &self.navigation.tree.path {
                        self.navigation.tree.path = Some(
                            renamed(path, &change.entry, &response.path)
                                .unwrap_or_else(|| path.clone()),
                        );
                    }
                } else if change.kind == Kind::Delete {
                    self.expanded.retain(|path| !within(path, &change.entry));
                    if self
                        .navigation
                        .tree
                        .path
                        .as_ref()
                        .is_some_and(|path| within(path, &change.entry))
                    {
                        self.navigation.tree.path = None;
                    }
                } else if change.kind == Kind::File {
                    open = Some(response.path.clone());
                }
                self.status = format!("{} complete", change.kind.title());
            }
            Err(error) => {
                self.status = format!("{} failed", change.kind.title());
                self.notify(error, true);
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
        self.file_browser.invalidate_rows();
        let mut paths: Vec<_> = self
            .expanded
            .iter()
            .filter(|path| !self.directories.contains_key(*path))
            .cloned()
            .collect();
        if let Some(parent) = parent {
            paths.push(parent);
        }
        if let Some(workspace) = &self.workspace
            && !self.directories.contains_key(&workspace.root)
        {
            paths.push(workspace.root.clone());
        }
        paths.sort();
        paths.dedup();
        for path in paths {
            self.list(path, ctx);
        }
        if let Some(path) = open {
            self.open_file(path, ctx);
        }
    }

    /// Render cached directory rows and explicit project actions; directory I/O is delegated to workers.
    pub(super) fn explorer(&mut self, ctx: &egui::Context) {
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
            palette.paint_panel(ui);
            self.pane_ui(ui, navigation::Pane::Explorer);
            ui.horizontal(|ui| {
                ui.label(
                    palette
                        .tag("EXPLORER")
                        .color(if palette.chrome.is_decorated() {
                            palette.heading
                        } else {
                            palette.muted
                        }),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let ready = self.project_ready()
                        && !self.explorer_state.busy
                        && self.dialog == Dialog::None
                        && self.pending.is_none()
                        && !egui::Popup::is_any_open(ctx)
                        && ui.memory(|memory| memory.allows_interaction(ui.layer_id()));
                    let refresh = ui
                        .add_enabled_ui(ready, |ui| {
                            icons::small(ui, Icon::Refresh, "Refresh expanded folders")
                        })
                        .inner;
                    ui_probe::button(ui, "explorer-refresh-expanded", &refresh);
                    if refresh.clicked() {
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
            if self.explorer_state.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Updating files…");
                });
            }
            ui.add_space(12.0);
            if let Some(workspace) = &self.workspace {
                let root = workspace.root.clone();
                let remote = workspace.remote.is_some();
                let connected = workspace.connected;
                let root_response = ui
                    .add(
                        egui::Label::new(
                            RichText::new(ssh::display_name(&root).to_uppercase())
                                .strong()
                                .size(12.0)
                                .color(palette.text),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&root);
                ui_probe::button(ui, "explorer-root", &root_response);
                let root_entry = Entry {
                    path: root.clone(),
                    name: ssh::display_name(&root).into(),
                    dir: true,
                };
                root_response.context_menu(|ui| self.explorer_menu(ui, &root_entry, true, ctx));
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

    /// Render flattened visible tree rows, maintaining stable selection and deferred open/list intents.
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
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), 25.0), Sense::hover());
                // Context menus and clicks belong to this entry even if an
                // asynchronous listing changes row order or the scroll range.
                let response = ui.interact(
                    rect,
                    ui.id().with(("explorer-entry", &entry.path)),
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
                ui_probe::button(ui, &format!("explorer-row:{}", entry.path), &response);
                if response.secondary_clicked() {
                    self.navigation.tree.path = Some(entry.path.clone());
                    ctx.memory_mut(|memory| memory.request_focus(navigation::Pane::Explorer.id()));
                }
                response.context_menu(|ui| self.explorer_menu(ui, &entry, false, ctx));
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
}
