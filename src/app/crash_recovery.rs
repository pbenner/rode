//! Crash-only draft checkpoints and explicit, unbound restoration. Host/SSH and
//! vault reconnection remain with their existing owners; recovery never grants
//! file access or writes a previously opened path automatically.
use super::*;
mod store;
pub(super) use store::Entry;

pub(super) struct State {
    store: store::Store,
    candidates: Vec<store::Candidate>,
    versions: Vec<(u64, u64)>,
    draft: String,
    encrypted: bool,
    workspace: bool,
    warned: bool,
    current: bool,
}
impl EditorApp {
    pub(super) fn init_crash_recovery(&mut self) {
        let result = eframe::storage_dir("RODE")
            .ok_or_else(|| anyhow::anyhow!("Cannot locate desktop recovery storage"))
            .and_then(|root| store::Store::open(&root.join("recovery")));
        match result {
            Ok((store, candidates)) => {
                self.crash_recovery = Some(State {
                    store,
                    candidates,
                    versions: Vec::new(),
                    draft: String::new(),
                    encrypted: false,
                    workspace: false,
                    warned: false,
                    current: true,
                })
            }
            Err(e) => self.notify(format!("Draft recovery unavailable: {e:#}"), true),
        }
    }
    /// Build a bounded flush callback that needs no UI locks. Native entry uses
    /// it before exiting on display loss; ordinary close retains its own guards.
    pub fn recovery_barrier(&self) -> Box<dyn FnOnce() + Send> {
        let shared = self.crash_recovery.as_ref().map(|s| s.store.shared.clone());
        let workspace = self.workspace_sessions.as_ref().map(|s| s.barrier());
        Box::new(move || {
            if let Some(workspace) = workspace {
                let _ = workspace.flush(Duration::from_secs(1));
            }
            if let Some(shared) = shared {
                let _ = shared.flush(Duration::from_secs(1));
            }
        })
    }
    pub(super) fn finish_crash_recovery(&self) {
        if self.exit.closing()
            && !std::thread::panicking()
            && let Some(state) = &self.crash_recovery
        {
            state.store.clear_on_exit();
        }
    }
    pub(super) fn checkpoint_drafts(&mut self) -> bool {
        let encrypted = self.vaults.has_active();
        let versions = self.documents.changes().dirty_versions;
        let permitted_draft = self.agents.checkpoint_draft(encrypted).map(str::to_owned);
        let draft = self.agents.recovery_draft();
        let workspace = self.workspace_drafts_durable(&versions, draft);
        let Some(state) = self.crash_recovery.as_mut() else {
            return false;
        };
        if let Some(error) = state.store.error() {
            self.notify(format!("Draft checkpoint failed: {error}"), true);
            return false;
        }
        if versions == state.versions
            && draft == state.draft
            && encrypted == state.encrypted
            && workspace == state.workspace
        {
            return state.current;
        }
        state.current = false;
        state.versions = versions;
        state.draft = draft.to_owned();
        state.encrypted = encrypted;
        state.workspace = workspace;
        if workspace {
            state.store.submit(Vec::new());
            state.current = true;
            return true;
        }
        let (mut entries, excluded) = match self.documents.crash_entries(encrypted) {
            Ok(snapshot) => snapshot,
            Err(e) => {
                self.notify(format!("Draft checkpoint failed: {e:#}"), true);
                return false;
            }
        };
        if let Some(draft) = permitted_draft.filter(|s| !s.is_empty()) {
            entries.push(Entry {
                name: "Agent prompt draft".into(),
                text: draft,
                notebook: false,
            });
        }
        state.store.submit(entries);
        state.current = true;
        if (excluded || (encrypted && !draft.is_empty())) && !state.warned {
            state.warned = true;
            self.notify("Desktop crash recovery excludes encrypted-project and binary buffers. Save those edits before leaving the desktop.", false);
        }
        true
    }
    pub(super) fn crash_recovery_ui(&mut self, ctx: &egui::Context) {
        let Some(state) = self.crash_recovery.as_ref() else {
            return;
        };
        if state.candidates.is_empty() {
            return;
        }
        let mut action = None;
        egui::Window::new("Recover unsaved work")
            .id(egui::Id::new("crash-recovery"))
            .default_width(460.)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("A previous RODE window left unsaved drafts. Recover opens separate unsaved buffers; saving requires you to choose a destination.");
                egui::ScrollArea::vertical().max_height(260.).show(ui, |ui| {
                    for candidate in &state.candidates {
                        if let Some(failure) = &candidate.failure { ui.colored_label(self.themes.active().palette.error, failure); }
                        for entry in &candidate.entries {
                            ui.add(egui::Label::new(&entry.name).truncate()).on_hover_text(&entry.name);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    let recover = crate::keyboard::button(ui, "Recover drafts");
                    ui_probe::button(ui, "recovery-restore", &recover);
                    if recover.clicked() { action = Some(true); }
                    let discard = crate::keyboard::button(ui, "Discard drafts");
                    ui_probe::button(ui, "recovery-discard", &discard);
                    if discard.clicked() { action = Some(false); }
                });
            });
        let Some(restore) = action else { return };
        let state = self.crash_recovery.as_mut().unwrap();
        if restore {
            let mut recovered = None;
            for candidate in &mut state.candidates {
                if candidate.restored {
                    continue;
                }
                for entry in &candidate.entries {
                    recovered = Some(self.documents.restore_crash_entry(
                        entry,
                        &self.markdown_images,
                        &self.notebook_shutdowns,
                    ));
                }
                candidate.restored = true;
            }
            if let Some(document) = recovered {
                self.submit_editor_operation(
                    editor_operations::Operation::Select {
                        document,
                        position: None,
                    },
                    ctx,
                );
            }
            // Retain the old checkpoint until the new desktop has durably saved
            // the recovered buffers. A second crash cannot erase the only copy.
            let current = self.checkpoint_drafts();
            let state = self.crash_recovery.as_ref().unwrap();
            if !current
                || self.vaults.has_active()
                || !state.store.shared.flush(Duration::from_secs(1))
            {
                self.notify("Recovered drafts are open; their previous checkpoint is retained until you save or discard it.", true);
                return;
            }
        }
        let state = self.crash_recovery.as_mut().unwrap();
        let mut error = None;
        state.candidates.retain(|candidate| {
            if restore && candidate.failure.is_some() {
                return true;
            }
            match candidate.discard() {
                Ok(()) => false,
                Err(e) => {
                    error = Some(format!("{e:#}"));
                    true
                }
            }
        });
        if let Some(error) = error {
            self.notify(error, true);
        }
    }
}
