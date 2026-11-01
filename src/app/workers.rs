//! Captured file/decode jobs and bounded result routing on the UI thread.
//!
//! Dispatch records the host, policy, generation and resource leases before
//! spawning work. Results must pass those identities and document-version checks
//! before changing buffers; decoding and blocking host I/O stay on workers.

use super::*;
use anyhow::Context;

/// Resolve a local startup file to its parent before restoring that folder's
/// saved policy. Run on the dispatch worker; project pickers and SSH requests
/// keep directory-only semantics. The file stays captured until project commit.
fn resolve_startup_path(request: &mut Request, job: &mut Job) -> anyhow::Result<()> {
    if let (
        Request::Prepare { path, .. },
        Job::Prepare {
            remote: false,
            restore_saved: Some(_),
            open_file,
            ..
        },
    ) = (request, job)
    {
        let target = files::expand_home(path)
            .canonicalize()
            .with_context(|| format!("Cannot open {path}"))?;
        if target.is_file() {
            *path = target
                .parent()
                .context("File has no containing directory")?
                .to_string_lossy()
                .into_owned();
            *open_file = Some(target.to_string_lossy().into_owned());
        }
    }
    Ok(())
}

impl EditorApp {
    /// Capture host, plan, generation and resource guards before spawning blocking work. The
    /// raw-host exceptions are preparation/discovery and explicit desktop attachment selection;
    /// project-host file pickers retain active project restrictions.
    pub(super) fn dispatch(&mut self, request: Request, job: Job, ctx: &egui::Context) {
        let raw = matches!(
            &job,
            Job::Prepare { .. }
                | Job::Browse { .. }
                | Job::BrowseFiles { local: true, .. }
                | Job::AgentAttachment { local: true, .. }
        );
        if self.vaults.locking
            || (!raw
                && (self.workspace.as_ref().is_some_and(|w| !w.ready)
                    || (self.workspace.is_none() && self.preparing)))
        {
            match &job {
                Job::AgentAttachment { token, .. } => self.agents.files.finish(
                    *token,
                    Err("Wait for the project environment to be ready.".into()),
                ),
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
            Job::BrowseFiles { local: true, .. } | Job::AgentAttachment { local: true, .. } => {
                false
            }
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
            let mut request = match (request, &job) {
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
            let result = resolve_startup_path(&mut request, &mut job)
                .and_then(|()| execute(&request))
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
                    if matches!(&job, Job::AgentAttachment { .. }) {
                        let encoded = response
                            .binary
                            .take()
                            .ok_or_else(|| anyhow::anyhow!("Missing attachment bytes"))?;
                        response.attachment = Some(
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                agents::attachments::prepare(
                                    &response.path,
                                    &encoded,
                                    &image_budget,
                                    &pdf_cancel,
                                )
                            }))
                            .map_err(|_| {
                                anyhow::anyhow!(
                                    "Attachment conversion failed; the file may be corrupt."
                                )
                            })??,
                        );
                        return Ok(response);
                    }
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

    /// Queue a project-scoped directory refresh for the explorer cache.
    pub(super) fn list(&mut self, path: String, ctx: &egui::Context) {
        if self.loading.insert(path.clone()) {
            self.dispatch(Request::List { path: path.clone() }, Job::List(path), ctx);
        }
    }

    /// Select an existing path-bound tab or queue a host-scoped read for a new document.
    pub(super) fn open_file(&mut self, path: String, ctx: &egui::Context) {
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
            } else if self.feature(crate::extensions::Feature::Binary) {
                Request::ReadAuto { path: path.clone() }
            } else {
                Request::Read { path: path.clone() }
            };
            self.dispatch(request, Job::Open(path), ctx);
        }
    }

    /// Drain a bounded batch of worker results. Reject obsolete generations/tokens first,
    /// then apply domain-specific buffer/version checks before mutating visible state.
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
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
                        Job::BrowseFiles { token, path, .. }
                            if !file_browser::is_file_dialog(&self.dialog)
                                || !self.file_browser.matches(*token, path) =>
                        {
                            continue;
                        }
                        Job::BrowseFiles { .. } => self.file_browser.loading = false,
                        Job::AgentAttachment { .. } => {}
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
                            if let Job::AgentAttachment { token, .. } = job {
                                self.agents.files.finish(token, Err(error));
                            } else if matches!(job, Job::Browse { .. }) {
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
                                    // Read-only refreshes must not dispose a live kernel merely
                                    // because Git or an agent finished touching other files.
                                    if response.notebook.is_some()
                                        && doc.notebook.is_some()
                                        && doc.text == response.text
                                        && doc.revision.as_deref() == Some(&response.revision)
                                    {
                                        continue;
                                    }
                                    if let Some(result) = response.notebook {
                                        doc.notebook = result.ok().map(|book| {
                                            notebook_view::View::new(
                                                book,
                                                self.markdown_images.clone(),
                                                self.notebook_shutdowns.clone(),
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
                                open_file,
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
                                        open_file,
                                    });
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
                                                    self.notebook_shutdowns.clone(),
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
}
