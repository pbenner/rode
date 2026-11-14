//! Captured file/decode jobs and bounded result routing on the UI thread.
//!
//! Dispatch records the host, policy, generation and resource leases before
//! spawning work. Results must pass those identities and document-version checks
//! before changing buffers; decoding and blocking host I/O stay on workers.

use super::*;
use crate::sandbox::PlanExt as _;
use anyhow::Context;
use std::collections::BTreeMap;

/// Includes obsolete replies: a stale-result flood must also yield to native input.
const MAX_EVENTS_PER_FRAME: usize = 64;

mod completions;
mod events;
mod inbox;
#[cfg(test)]
mod tests;
pub(super) use events::{Job, WorkspaceEvent};
pub(super) use inbox::{Inbox, Message, ProfileSender, WorkspaceSender};

/// Resolve a local startup file to its parent before restoring that folder's
/// saved policy. Run on the dispatch worker; project pickers and SSH requests
/// keep directory-only semantics. A file whose inferred project would expose
/// HOME opens without a project when isolation came only from global defaults.
/// Explicit CLI requests and saved project policies must still be enforced.
pub(super) fn resolve_startup_path(
    request: &mut Request,
    job: &mut Job,
    policies: &BTreeMap<String, sandbox::Options>,
    home: &std::path::Path,
) -> anyhow::Result<()> {
    if let (
        Request::Prepare { path, .. },
        Job::Prepare {
            remote: false,
            restore_saved: Some(force),
            options,
            token,
            open_file,
            ..
        },
    ) = (request, &mut *job)
    {
        let target = files::expand_home(path)
            .canonicalize()
            .with_context(|| format!("Cannot open {path}"))?;
        if target.is_file() {
            let parent = target
                .parent()
                .context("File has no containing directory")?;
            *path = parent.to_string_lossy().into_owned();
            if options.enabled
                && !*force
                && !policies.contains_key(path)
                && sandbox::project_root_contains_home(parent, home)?
            {
                *job = Job::StandaloneFile {
                    path: target.to_string_lossy().into_owned(),
                    token: *token,
                };
                return Ok(());
            }
            *open_file = Some(target.to_string_lossy().into_owned());
        }
    }
    Ok(())
}

impl EditorApp {
    /// Capture one submitted source snapshot for both host I/O and the save acknowledgement.
    pub(super) fn dispatch_text_save(
        &mut self,
        id: u64,
        path: String,
        text: String,
        expected: Option<String>,
        ctx: &egui::Context,
    ) {
        self.dispatch(
            Request::Write {
                path,
                text: String::new(),
                expected,
            },
            Job::Save { id, text },
            ctx,
        );
    }

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
        let file_operation_pending = self.explorer_state.busy
            && matches!(
                &job,
                Job::Open(_) | Job::Save { .. } | Job::SaveBinary { .. }
            );
        if file_operation_pending
            || self.vaults.locking()
            || (!raw
                && (self.workspace.as_ref().is_some_and(|w| !w.ready)
                    || (self.workspace.is_none() && self.workspace.preparing())))
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
                Job::List(path, _) => {
                    self.explorer_state.loading.remove(path);
                }
                Job::Explorer(_) => self.explorer_state.busy = false,
                Job::Open(path) => {
                    self.documents.finish_open(path);
                }
                Job::Save { id, .. } | Job::SaveBinary { id, .. } => {
                    self.documents.finish_save(*id);
                }
                _ => {}
            }
            self.notify(if file_operation_pending { "Wait for the file operation to finish before opening or saving files." } else { "Open a project folder and wait for its environment to be ready before accessing files." }, true);
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
        let replies: WorkspaceSender = self.workers.workspace(ctx);
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
            // The submitted source has one owner: move it into the request for
            // I/O, then back into the job for acknowledgement. Do not retain a
            // second file-sized String merely to carry it through the worker.
            if let (Request::Write { text, .. }, Job::Save { text: snapshot, .. }) =
                (&mut request, &mut job)
            {
                std::mem::swap(text, snapshot);
            }
            let _image_io = matches!(
                &request,
                Request::ReadImage { .. }
                    | Request::ReadBinary { .. }
                    | Request::ReadHex { .. }
                    | Request::ReadAttachment { .. }
            )
            .then(|| image_budget.io.lock().unwrap_or_else(|e| e.into_inner()));
            let execute = |request: &Request| match &remote {
                Some(remote) => remote.request_in(request, plan.as_ref()),
                None => match &plan {
                    Some(plan) => plan.local_request(request),
                    None => files::local(request),
                },
            };
            let resolved = resolve_startup_path(
                &mut request,
                &mut job,
                &restore_policies,
                &files::expand_home("~"),
            );
            let result = resolved
                .and_then(|()| {
                    if matches!(&job, Job::StandaloneFile { .. }) {
                        // No directory listing, project services or sandbox probe for
                        // this file. The UI queues its ordinary reader after startup.
                        Ok(Response::default())
                    } else {
                        execute(&request)
                    }
                })
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
            if let (Request::Write { text, .. }, Job::Save { text: snapshot, .. }) =
                (&mut request, &mut job)
            {
                std::mem::swap(text, snapshot);
            }
            let _ = replies.send(WorkspaceEvent::Done {
                job,
                result: Box::new(result),
            });
        });
    }

    /// Queue a project-scoped directory refresh for the explorer cache.
    pub(super) fn list(&mut self, path: String, ctx: &egui::Context) {
        if let Some(epoch) = self.explorer_state.begin_list(&path) {
            self.dispatch(
                Request::List { path: path.clone() },
                Job::List(path, epoch),
                ctx,
            );
        }
    }

    /// Select an existing path-bound tab or queue a host-scoped read for a new document.
    pub(super) fn open_file(&mut self, path: String, ctx: &egui::Context) {
        self.open_file_in(path, self.command_editor(ctx), ctx);
    }

    /// Focus an already-open document in its owner, or dispatch one read for the requested
    /// viewport.
    /// The completion enters the editor coordinator after native input; it never clones a live
    /// buffer.
    pub(super) fn open_file_in(
        &mut self,
        path: String,
        viewport: egui::ViewportId,
        ctx: &egui::Context,
    ) {
        if let Some(index) = self
            .documents
            .iter()
            .position(|d| d.path.as_deref() == Some(&path))
        {
            self.focus_document(self.documents[index].id, ctx);
            return;
        }
        self.editor_intent_in(viewport);
        if self.documents.begin_open_in(path.clone(), viewport) {
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
        if let Some(error) = self.connection_profiles.poll_reload() {
            self.notify(error, true);
        }
        for index in 0..MAX_EVENTS_PER_FRAME {
            let Some(event) = self.workers.receive() else {
                break;
            };
            if index + 1 == MAX_EVENTS_PER_FRAME {
                // Do not consume a look-ahead event merely to check for more
                // work. One extra idle repaint is cheaper than dropping a reply.
                ctx.request_repaint();
            }
            match event {
                Message::Profiles(completion) => self.profile_completed(completion),
                Message::Workspace(event) => {
                    self.workspace_event(*event, ctx);
                }
                Message::Expired => {}
            }
        }
        self.poll_connection_health(ctx);
    }

    fn workspace_event(&mut self, event: WorkspaceEvent, ctx: &egui::Context) {
        match event {
            WorkspaceEvent::GitGutter(viewport, result) => {
                self.complete_editor_gutter(viewport, result)
            }
            WorkspaceEvent::NotebookParsed {
                id,
                version,
                result,
            } => self.notebook_parsed(id, version, result, ctx),
            WorkspaceEvent::Session { job, result } => self.handle_session(job, result, ctx),
            WorkspaceEvent::PdfPage {
                id,
                version,
                token,
                page,
                result,
            } => self.pdf_page(id, version, token, page, result),
            WorkspaceEvent::Latex {
                token,
                preview,
                versions,
                result,
            } => self.complete_latex(token, preview, versions, *result, ctx),
            WorkspaceEvent::Marp {
                id,
                version,
                token,
                exporting,
                result,
            } => self.marp_complete(id, version, token, exporting, *result, ctx),
            WorkspaceEvent::MarpPage {
                id,
                version,
                token,
                page,
                result,
            } => self.marp_page(id, version, token, page, result),
            WorkspaceEvent::MarkdownImage {
                id,
                token,
                uri,
                result,
            } => self.markdown_image(id, token, uri, result, ctx),
            WorkspaceEvent::Tool { job, result } => self.handle_tool(*job, *result, ctx),
            WorkspaceEvent::Connected { result } => self.connection_completed(result, ctx),
            WorkspaceEvent::Done { job, result } => self.file_completed(job, *result, ctx),
        }
    }

    fn poll_connection_health(&mut self, ctx: &egui::Context) {
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
