//! Confirmed file operations on a captured browser endpoint. Filesystem work
//! runs off the UI thread; generation checks separate host effects from views.
use super::*;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Rename,
    Delete,
}
impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rename => "Rename",
            Self::Delete => "Delete",
        }
    }
    fn mutation(self, entry: &Entry, name: &str) -> files::Mutation {
        match self {
            Self::Rename => files::Mutation::Rename {
                path: entry.path.clone(),
                destination: std::path::Path::new(&entry.path)
                    .with_file_name(name)
                    .to_string_lossy()
                    .into(),
            },
            Self::Delete => files::Mutation::Remove {
                path: entry.path.clone(),
            },
        }
    }
}

pub(super) struct Form {
    side: Side,
    action: Action,
    generation: u64,
    token: u64,
    directory: String,
    entries: Vec<Entry>,
    name: String,
}
impl Form {
    fn valid(&self) -> bool {
        self.action == Action::Delete
            || (self.entries.len() == 1
                && explorer::valid_name(&self.name)
                && self.name != self.entries[0].name)
    }
}
pub(super) struct Running {
    pub id: u64,
    generation: u64,
    project_generation: Option<u64>,
    side: Side,
    action: Action,
    requested: usize,
    pub cancel: Arc<AtomicBool>,
}
pub(super) struct Completed {
    entry: Entry,
    mutation: files::Mutation,
    result: Result<Response, String>,
}

/// Only listed immediate children may be renamed/deleted. In particular,
/// "Select this folder" is a copy intent, never authority to delete a root.
fn listed(pane: &Pane, entries: &[Entry]) -> bool {
    let current: HashMap<_, _> = pane
        .browser
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    !entries.is_empty()
        && entries.len() <= files::MAX_ENTRIES
        && entries.iter().map(|entry| entry.path.len()).sum::<usize>() <= 16 * 1024 * 1024
        && entries.iter().all(|entry| {
            std::path::Path::new(&entry.path).is_absolute()
                && std::path::Path::new(&entry.path).parent()
                    == Some(std::path::Path::new(&pane.browser.directory))
                && current.get(entry.path.as_str()).is_some_and(|current| {
                    current.path == entry.path
                        && current.name == entry.name
                        && current.dir == entry.dir
                })
        })
}

impl EditorApp {
    /// Host identity, rather than equal path strings, determines whether a
    /// transfer-browser operation can affect the project's open editor bindings.
    fn transfer_files_on_project_host(&self, side: Side) -> bool {
        let Some(workspace) = self.workspace.as_ref() else {
            return false;
        };
        let Some(endpoint) = self.transfer_endpoint(side) else {
            return false;
        };
        match (endpoint.remote(), workspace.remote.as_ref()) {
            (None, None) => true,
            (Some(selected), Some(project)) => selected.socket == project.socket,
            _ => false,
        }
    }

    pub(super) fn transfer_files_writable(&self, side: Side) -> bool {
        self.panel_enabled(Panel::Transfers)
            && self.transfers.file_operation.is_none()
            && self.transfers.running.is_none()
            && self.transfers.queue.is_empty()
            && self.transfers.panes[side].ready()
            && self.transfer_endpoint(side).is_some_and(|endpoint| {
                !endpoint
                    .plan()
                    .is_some_and(|plan| plan.active && plan.project_read_only)
            })
            && (!self.transfer_files_on_project_host(side)
                || (!self.workspace.preparing()
                    && !self.explorer_state.busy
                    && !self.documents.has_pending_opens()
                    && !self.documents.iter().any(|doc| doc.saving)))
    }

    pub(super) fn transfer_file_form(
        &mut self,
        side: Side,
        action: Action,
        mut entries: Vec<Entry>,
    ) {
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries.dedup_by(|a, b| a.path == b.path);
        if !self.transfer_files_writable(side)
            || !listed(&self.transfers.panes[side], &entries)
            || (action == Action::Rename && entries.len() != 1)
        {
            return;
        }
        let pane = &self.transfers.panes[side];
        self.transfers.file_form = Some(Form {
            side,
            action,
            generation: self.transfers.generation,
            token: pane.browser.token,
            directory: pane.browser.directory.clone(),
            name: entries[0].name.clone(),
            entries,
        });
        self.set_dialog(Dialog::TransferFiles);
    }

    /// The ordinary exclusive dialog host supplies viewport ownership, Escape,
    /// sizing-pass input retention and detach/reattach behavior.
    pub(in crate::app) fn transfer_file_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(form) = self.transfers.file_form.as_ref() else {
            self.dialog = Dialog::None;
            return;
        };
        let side = form.side;
        let action = form.action;
        let pane = &self.transfers.panes[side];
        let current = form.generation == self.transfers.generation
            && form.token == pane.browser.token
            && form.directory == pane.browser.directory
            && listed(pane, &form.entries);
        let writable = current && self.transfer_files_writable(side);
        let project_host = self.transfer_files_on_project_host(side);
        let palette = self.themes.active().palette;
        let host = self.transfer_host_label();
        layout::heading(
            ui,
            action.label(),
            if side == Side::Local {
                "This computer"
            } else {
                &host
            },
            palette,
        );
        ui.add(egui::Label::new(&form.directory).truncate())
            .on_hover_text(&form.directory);
        for entry in form.entries.iter().take(8) {
            ui.add(egui::Label::new(&entry.name).truncate())
                .on_hover_text(&entry.path);
        }
        if form.entries.len() > 8 {
            ui.weak(format!("… and {} more", form.entries.len() - 8));
        }
        let form = self.transfers.file_form.as_mut().unwrap();
        if action == Action::Delete {
            ui.label(if form.entries.len() == 1 {
                "Permanently delete this item?".into()
            } else {
                format!("Permanently delete these {} items?", form.entries.len())
            });
            ui.label("Folders include all their contents. This cannot be undone.");
            if project_host {
                ui.label("Open editors and unsaved changes are kept. Use Save As to retain their contents.");
            }
        } else {
            let id = ui.id().with("transfer-file-name");
            let input = ui.add(crate::keyboard::named(
                "New name",
                egui::TextEdit::singleline(&mut form.name)
                    .id(id)
                    .desired_width(f32::INFINITY)
                    .char_limit(255),
            ));
            if self.dialog_focus {
                input.request_focus();
                let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(form.name.chars().count()),
                    )));
                state.store(ctx, id);
                self.dialog_focus = false;
            }
            ui_probe::button(ui, "transfer-file-name", &input);
            ui_probe::value(ui, "transfer-file-name-value", &input, &form.name);
            if !explorer::valid_name(&form.name) {
                ui.colored_label(
                    palette.error,
                    "Enter one name of at most 255 bytes. It cannot contain / or be . or ..",
                );
            }
            ui.weak("Existing files and folders are never replaced.");
        }
        let destination = std::path::Path::new(&form.entries[0].path).with_file_name(&form.name);
        let valid = form.valid();
        let occupied = action == Action::Rename
            && valid
            && project_host
            && self.explorer_destination_in_use(&destination);
        if occupied {
            ui.colored_label(
                palette.error,
                "An open editor already uses this destination. Choose another name.",
            );
        }
        if !current {
            ui.colored_label(
                palette.error,
                "The folder or connection changed. Cancel and select the items again.",
            );
        }
        let allowed = writable && valid && !occupied;
        let mut apply = false;
        ui.add_space(12.);
        ui.horizontal(|ui| {
            let button = ui.add_enabled(allowed, crate::keyboard::Button::new(action.label()));
            ui_probe::button(ui, "transfer-file-apply", &button);
            let cancel = crate::keyboard::button(ui, "Cancel");
            ui_probe::button(ui, "transfer-file-cancel", &cancel);
            // Destructive dialogs start on Cancel, so opening one does not
            // turn an incidental Space press into filesystem deletion.
            if action == Action::Delete && self.dialog_focus {
                cancel.request_focus();
                self.dialog_focus = false;
            }
            // Enter in the name field submits a rename, but a focused Cancel
            // button must retain its native keyboard activation.
            let enter = action == Action::Rename
                && !ui.is_sizing_pass()
                && allowed
                && !cancel.has_focus()
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
            apply = button.clicked() || enter;
            if cancel.clicked() {
                self.transfers.file_form = None;
                self.dialog = Dialog::None;
                apply = false;
            }
        });
        if apply {
            self.start_transfer_files(ctx);
        }
    }

    fn start_transfer_files(&mut self, ctx: &egui::Context) {
        let Some(form) = self.transfers.file_form.take() else {
            return;
        };
        self.dialog = Dialog::None;
        // Recheck admission at dispatch as well as while rendering the button.
        if !self.transfer_files_writable(form.side)
            || form.generation != self.transfers.generation
            || form.token != self.transfers.panes[form.side].browser.token
            || !listed(&self.transfers.panes[form.side], &form.entries)
            || !form.valid()
        {
            return;
        }
        let Some(endpoint) = self.transfer_endpoint(form.side) else {
            return;
        };
        let project_generation = self
            .transfer_files_on_project_host(form.side)
            .then(|| self.workers.generation());
        if form.action == Action::Rename
            && project_generation.is_some()
            && self.explorer_destination_in_use(
                &std::path::Path::new(&form.entries[0].path).with_file_name(&form.name),
            )
        {
            return;
        }
        if project_generation.is_some() {
            self.explorer_state.begin_mutation();
        }
        self.status = format!("{}…", form.action.label());
        let state = &mut self.transfers;
        state.next = state.next.wrapping_add(1);
        let id = state.next;
        let cancel = Arc::new(AtomicBool::new(false));
        state.file_operation = Some(Running {
            id,
            generation: state.generation,
            project_generation,
            side: form.side,
            action: form.action,
            requested: form.entries.len(),
            cancel: cancel.clone(),
        });
        state.error = None;
        let pending = Pending::new(&state.pending);
        let work = self.vaults.work();
        let send = state.send.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _work = work;
            let mut completed = Vec::new();
            for entry in form.entries {
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                let mutation = form.action.mutation(&entry, &form.name);
                let result = endpoint
                    .mutate(mutation.clone())
                    .map_err(|e| format!("{} {}: {e:#}", form.action.label(), entry.path));
                let failed = result.is_err();
                completed.push(Completed {
                    entry,
                    mutation,
                    result,
                });
                if failed {
                    break;
                }
            }
            let _ = send.send(Event::Mutated { id, completed });
            drop(pending);
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        });
    }

    /// Apply real host effects even after the transfer view changes, but never
    /// rewrite document paths belonging to a different project generation.
    pub(super) fn finish_transfer_files(
        &mut self,
        operation: Running,
        completed: Vec<Completed>,
        ctx: &egui::Context,
    ) {
        let successes = completed.iter().filter(|item| item.result.is_ok()).count();
        let error = completed
            .iter()
            .find_map(|item| item.result.as_ref().err())
            .cloned();
        if operation.generation == self.transfers.generation {
            self.transfers.error = error.clone();
            for pane in self.transfers.panes.iter_mut() {
                pane.browser.needs_load = true;
            }
        }
        if operation.project_generation == Some(self.workers.generation()) {
            self.explorer_state.busy = false;
            self.finish_file_changes(
                completed.into_iter().map(|item| {
                    (
                        explorer::Change::from_mutation(item.entry, &item.mutation),
                        item.result,
                    )
                }),
                ctx,
            );
        }
        if operation.generation == self.transfers.generation {
            self.status = format!(
                "{}: {successes}/{} completed",
                operation.action.label(),
                operation.requested
            );
            if error.is_none() && successes < operation.requested {
                self.transfers.error = Some("File operation stopped. Some items may already have changed. Refresh before trying again.".into());
            }
            self.transfers.panes[operation.side].selected.clear();
        }
    }
}
