//! File-result admission precedes all pending-marker cleanup and buffer mutation.
use super::*;

impl EditorApp {
    pub(super) fn file_completed(
        &mut self,
        job: Job,
        result: Result<Response, String>,
        ctx: &egui::Context,
    ) {
        match &job {
            Job::List(_, epoch) if *epoch != self.explorer_state.epoch => return,
            Job::Prepare { token, .. } | Job::StandaloneFile { token, .. }
                if !self.workspace.finish_preparation(*token) =>
            {
                return;
            }
            Job::Reload { id, version }
                if !self
                    .documents
                    .iter()
                    .any(|doc| doc.id == *id && doc.edit_version == *version) =>
            {
                return;
            }
            Job::Browse { token, path, .. }
                if self.dialog != Dialog::Folder || !self.project_picker.matches(*token, path) =>
            {
                return;
            }
            Job::BrowseFiles { token, path, .. }
                if !file_browser::is_file_dialog(&self.dialog)
                    || !self.file_browser.matches(*token, path) =>
            {
                return;
            }
            Job::BrowseFiles { .. } => self.file_browser.loading = false,
            Job::AgentAttachment { .. } => {}
            Job::Prepare { .. } | Job::StandaloneFile { .. } => {}
            Job::Browse { .. } => self.project_picker.loading = false,
            Job::List(path, _) => {
                self.explorer_state.loading.remove(path);
            }
            Job::Explorer(_) => self.explorer_state.busy = false,
            Job::Open(path) => {
                self.documents.finish_open(path);
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
            Job::List(..)
                | Job::Explorer(_)
                | Job::Open(_)
                | Job::Save { .. }
                | Job::SaveBinary { .. }
        ) {
            self.file_browser.invalidate_rows();
        }
        if let Ok(response) = result.as_ref()
            && let Some(Err(error)) = &response.notebook
        {
            self.notify(format!("Notebook: {error} JSON source is available."), true);
        }
        if let Job::Explorer(change) = job {
            self.finish_explorer(change, result, ctx);
            return;
        }
        match result {
            Err(error) => {
                if let Job::AgentAttachment { token, .. } = job {
                    self.agents.files.finish(token, Err(error));
                } else if matches!(job, Job::Browse { .. }) {
                    self.project_picker.error = Some(error);
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
                Job::AgentAttachment { token, .. } => {
                    self.agents.files.finish(
                        token,
                        response
                            .attachment
                            .ok_or_else(|| "Missing attachment contents".into()),
                    );
                    self.agents.focus = true;
                }
                Job::Reload { id, version } => {
                    if let Some(doc) = self
                        .documents
                        .iter_mut()
                        .find(|d| d.id == id && d.edit_version == version && !d.dirty())
                    {
                        doc.reload(response, &self.markdown_images, &self.notebook_shutdowns);
                    }
                }
                Job::StandaloneFile { path, .. } => self.open_file(path, ctx),
                Job::Prepare {
                    token,
                    remote,
                    options,
                    keep_buffers,
                    documents_at_start,
                    open_file,
                    ..
                } => {
                    let new_edits = self
                        .documents
                        .iter()
                        .filter(|d| d.dirty())
                        .any(|d| !documents_at_start.contains(&(d.id, d.edit_version)));
                    if !keep_buffers && new_edits {
                        self.workspace.hold_prepared(
                            token,
                            workspace::Prepared {
                                response,
                                remote,
                                options,
                                open_file,
                            },
                        );
                        self.pending = Some(Action::PreparedProject { id: token });
                    } else {
                        self.commit_project(
                            response,
                            remote,
                            options,
                            keep_buffers,
                            open_file,
                            ctx,
                        );
                    }
                }
                Job::BrowseFiles { .. } => self.file_browser.loaded(response),
                Job::Browse { options, .. } => {
                    let saved = self.saved_sandbox_for(&response.path, self.project_picker.remote);
                    self.project_picker.loaded(response, &options, saved);
                }
                Job::Explorer(_) => unreachable!("Explorer results are handled above"),
                Job::List(path, _) => {
                    if self.workspace.canonicalize_root(&path, &response.path) {
                        self.explorer_state.expanded.insert(response.path.clone());
                    }
                    self.explorer_state.loaded(path, response);
                }
                Job::Open(_) => {
                    self.documents
                        .open(response, &self.markdown_images, &self.notebook_shutdowns);
                    if self.documents.active().is_some_and(|d| {
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
                    if self.documents.active().is_some_and(|d| d.id == id) {
                        self.reveal_tab(id);
                    }
                    if let Some(root) = self.workspace.as_ref().map(|w| w.root.clone()) {
                        self.list(root, ctx);
                    }
                }
            },
        }
    }
}
