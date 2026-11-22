//! Ordered project teardown. Domain owners release views/work before transports;
//! persistent services and acknowledged host-cleanup guards keep their own lifetimes.
use super::*;
mod connection;
pub(super) use connection::ConnectionId;

mod control;
pub(super) use control::{Controller, PreparationId, PreparationPolicy, Prepared};

/// Destination after the shared project-close guard succeeds. SSH is released
/// only after encrypted unmount has been acknowledged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CloseTarget {
    Welcome,
    Disconnect,
}

/// The operation determines lifetime policy; display text never selects cleanup behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LeaveReason {
    ProjectChanged,
    PolicyChanged,
    Disconnected,
    WindowClosed,
    VaultLock,
    NotebooksDisabled,
}

impl LeaveReason {
    /// Return the user-facing explanation for releasing workspace resources.
    pub fn message(self) -> &'static str {
        match self {
            Self::VaultLock => "Locking encrypted project",
            Self::NotebooksDisabled => "Notebook extension disabled",
            _ => "Workspace changed",
        }
    }
    /// Require persistent work to stop when locking a vault.
    /// Other departures retain explicitly persistent services according to their domain policy.
    pub fn stops_persistent_work(self) -> bool {
        self == Self::VaultLock
    }
}

/// Choose whether invalidating project preparation also closes and clears its picker.
pub(super) enum PickerReset {
    Invalidate,
    Close,
}

impl EditorApp {
    /// Return to the desktop after SSH teardown. Keep recovery buffers/leases independent of this
    /// neutral context, and defer while a close decision or encrypted lock still owns the window.
    pub(in crate::app) fn restore_local_workspace(&mut self, ctx: &egui::Context) {
        if self.exit.waiting()
            || self.exit.closing()
            || self.pending.as_ref().is_some()
            || self.vaults.locking()
            || self.explorer_state.busy
            || !self.workspace.take_local_fallback()
        {
            return;
        }
        match std::env::current_dir() {
            Ok(path) => self.prepare_project(
                path.to_string_lossy().into_owned(),
                Default::default(),
                false,
                true,
                PreparationPolicy::LocalFallback,
                ctx,
            ),
            Err(error) => self.notify(
                format!("Cannot reopen the local working directory: {error}"),
                true,
            ),
        }
    }

    /// An acknowledged local fallback installs no remote authority and discards no buffer, lease or
    /// foreground view. A newer project/connection already invalidates its preparation identity.
    pub(in crate::app) fn commit_local_fallback(
        &mut self,
        response: Response,
        ctx: &egui::Context,
    ) {
        if self.workspace.is_some() || self.exit.waiting() || self.exit.closing() {
            return;
        }
        let Some(plan) = response.project else {
            self.notify("Host returned an invalid local project description.", true);
            return;
        };
        if plan.active || plan.requested || !plan.prefix.is_empty() || plan.host_context.is_some() {
            self.notify(
                "Local working-directory fallback requires Normal mode on this computer.",
                true,
            );
            return;
        }
        self.workspace
            .activate_local_fallback(response.path.clone(), plan);
        self.explorer_state.expanded.insert(response.path.clone());
        self.explorer_state
            .directories
            .insert(response.path, response.entries);
        self.explorer_state.tree_dirty = true;
        self.load_project_tools(ctx, false);
        self.status = "Local project · NORMAL MODE".into();
    }

    /// Capture document versions so a late candidate cannot discard edits made during
    /// preparation. Explorer mutations and preparation are mutually exclusive so a
    /// candidate cannot replace a file operation's captured workspace.
    pub(in crate::app) fn prepare_project(
        &mut self,
        path: String,
        options: sandbox::Options,
        remote: bool,
        keep_buffers: bool,
        policy: PreparationPolicy,
        ctx: &egui::Context,
    ) {
        if self.exit.waiting()
            || self.exit.closing()
            || matches!(
                self.pending.as_ref(),
                Some(
                    Action::Quit
                        | Action::Disconnect
                        | Action::CloseProject
                        | Action::LockEncrypted
                )
            )
        {
            return;
        }
        if self.explorer_state.busy {
            self.notify(
                "Wait for the file operation to finish before preparing another project or policy.",
                false,
            );
            return;
        }
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
        // Implicit startup is deliberately independent of global grants,
        // saved per-project policies and sandbox host availability.
        let options = if policy.implicit_local() {
            sandbox::Options::default()
        } else {
            options
        };
        if let Err(error) = options.validate() {
            self.notify(error.to_string(), true);
            return;
        }
        let token = self.workspace.begin_preparation();
        self.bookmark_project_preparing(token, &path, remote);
        self.status = if remote {
            "Opening SSH project…"
        } else {
            "Opening local project…"
        }
        .into();
        self.dispatch(
            Request::Prepare {
                path,
                sandbox: if matches!(policy, PreparationPolicy::Saved(_)) {
                    Default::default()
                } else {
                    options.clone()
                },
            },
            Job::Prepare {
                remote,
                options,
                keep_buffers,
                token,
                documents_at_start: self.documents.versions(),
                policy,
                open_file: None,
            },
            ctx,
        );
    }

    /// Install an accepted prepared context after common guards pass. Invalidate old
    /// services/generations before starting commands for the new host and policy.
    /// A startup file is opened through the ordinary reader only after this commit.
    pub(in crate::app) fn commit_prepared(&mut self, prepared: Prepared, ctx: &egui::Context) {
        let Prepared {
            response,
            remote,
            options,
            policy,
            keep_buffers,
            open_file,
            ..
        } = prepared;
        let Some(plan) = response.project else {
            self.notify("Host returned an invalid project description.", true);
            return;
        };
        let opening_connection = remote && self.workspace.as_ref().is_some_and(|w| !w.ready);
        let connection = if remote {
            self.workspace.as_ref().and_then(|w| w.remote.clone())
        } else {
            None
        };
        if remote && connection.is_none() {
            self.notify("SSH connection ended before the project was opened.", true);
            return;
        }
        let settings_draft = keep_buffers
            .then(|| self.settings_draft_for(&response.path, remote))
            .flatten();
        let encrypted = self
            .vaults
            .take_project(&response.path, connection.as_ref());
        if remote || (keep_buffers && self.workspace.as_ref().is_some_and(|w| w.remote.is_none())) {
            self.clear_project(
                documents::Retention::from(keep_buffers),
                if keep_buffers {
                    workspace::LeaveReason::PolicyChanged
                } else {
                    workspace::LeaveReason::ProjectChanged
                },
            );
        } else {
            self.reset_workspace(workspace::LeaveReason::ProjectChanged);
        }
        let connection = connection.map(|mut r| {
            r.profile.root = response.path.clone();
            r.profile.sandbox = options.clone();
            self.connection_profiles.adopt(r.profile.clone());
            r
        });
        if policy.implicit_local() {
            self.workspace
                .activate_local_fallback(response.path.clone(), plan.clone());
        } else {
            self.workspace.activate(
                response.path.clone(),
                connection,
                plan.clone(),
                options.clone(),
            );
        }
        self.vaults.activate_project(encrypted);
        if remote {
            self.restore_ssh_buffers();
        }
        self.explorer_state.expanded.insert(response.path.clone());
        self.explorer_state
            .directories
            .insert(response.path, response.entries);
        self.explorer_state.tree_dirty = true;
        self.load_project_tools(ctx, !policy.implicit_local());
        if let Some(draft) = settings_draft {
            self.restore_settings_draft(draft);
        }
        self.status = format!(
            "{} project · {}",
            if remote { "SSH" } else { "Local" },
            plan.label()
        );
        if plan.requested && !plan.active {
            self.notify(plan.reason.clone(), false);
        }
        let show_welcome = open_file.is_none() && self.presentation.area == areas::Area::Welcome;
        self.show_terminal |= remote || options.enabled;
        if let Some(path) = open_file {
            self.open_file_in(path, egui::ViewportId::ROOT, ctx);
        } else if self.show_terminal {
            self.start_project_terminal(ctx);
        }
        if show_welcome || (opening_connection && self.documents.active().is_none()) {
            // A fresh login stays on Welcome. Late shell attachment must not
            // take over this presentation or the remembered project view.
            if self.presentation.surface == tool_windows::Surface::Terminal
                && self.presentation.last_tool != Some(ToolTab::Terminal)
            {
                if let Some(ToolTab::Panel(panel)) = self.presentation.last_tool
                    && self.panel_enabled(panel)
                    && !self.tab_detached(ToolTab::Panel(panel))
                {
                    self.presentation.select_panel(panel);
                } else {
                    self.presentation.select_editor();
                }
            }
            self.open_area(areas::Area::Welcome, ctx);
        } else if opening_connection {
            // Reconnect resumes retained buffers, including unsaved recovery
            // text, through the existing editor rather than the login terminal.
            self.focus_editor_in(egui::ViewportId::ROOT, ctx);
        }
    }

    /// Fixtures may install an already checked context without constructing the
    /// transition owner's version/admission envelope.
    #[cfg(test)]
    pub(in crate::app) fn commit_project(
        &mut self,
        response: Response,
        remote: bool,
        options: sandbox::Options,
        keep_buffers: bool,
        open_file: Option<String>,
        ctx: &egui::Context,
    ) {
        self.commit_prepared(
            Prepared {
                response,
                remote,
                options,
                keep_buffers,
                open_file,
                policy: PreparationPolicy::Given,
                approved_documents: vec![],
            },
            ctx,
        );
    }

    /// Invalidate project work and release window-owned views before transport cleanup.
    /// Same-host policy changes may keep bound buffers; disconnect callers must first
    /// replace their paths with non-authorizing recovery metadata.
    pub(super) fn clear_project(&mut self, retention: documents::Retention, reason: LeaveReason) {
        self.shared_folders.reset();
        self.invalidate_project_transfers();
        self.cancel_guarded_action();
        // Forced transport loss can invalidate the jobs whose acknowledgements
        // a waiting quit needs. Losing those jobs is not successful cleanup.
        if self.exit.cancel() {
            if self.dialog == Dialog::TerminalExit {
                self.dialog = Dialog::None;
            }
            self.notify(
                "Workspace changed while closing. Check runtime state before quitting again.",
                true,
            );
        }
        self.reset_debugger();
        self.latex.reset_project();
        self.reset_pdf_requests();
        self.leave_project_agent();
        self.documents.reset_context(retention, reason);
        self.reset_editor_contexts(retention);
        self.extensions.reset_project();
        self.workbench.reset_project();
        self.sessions.release_views(&mut self.terminal);
        self.session_context_reset();
        self.vaults.release_project();
        self.explorer_state.reset();
        self.file_browser.reset();
        self.navigation.cancel_input();
        self.workers.invalidate_workspace();
    }

    /// Complete an admitted close through the vault owner when necessary. An
    /// unmount refusal keeps the project and transport available for recovery.
    pub(super) fn close_project(&mut self, target: CloseTarget, ctx: &egui::Context) {
        if self.vaults.has_active() {
            self.lock_encrypted(
                match target {
                    CloseTarget::Welcome => vaults::AfterLock::Project,
                    CloseTarget::Disconnect => vaults::AfterLock::Disconnect,
                },
                ctx,
            );
        } else {
            self.finish_project_close(target, LeaveReason::ProjectChanged, ctx);
        }
    }

    /// Release a closed project's authority while retaining its authenticated
    /// host. The local CWD is prepared separately in Normal mode.
    pub(super) fn finish_project_close(
        &mut self,
        target: CloseTarget,
        reason: LeaveReason,
        ctx: &egui::Context,
    ) {
        if target == CloseTarget::Disconnect {
            self.detach_ssh(documents::Retention::Discard, ctx);
        } else {
            self.clear_project(documents::Retention::Discard, reason);
            self.workspace.clear_project_context();
            self.invalidate_project_candidate(PickerReset::Close);
            if self.workspace.is_none() {
                self.workspace.request_local_fallback();
            }
            self.status = "Project closed".into();
        }
        self.project_closed_presentation(ctx);
    }

    /// Closing a project returns to the same neutral presentation as startup;
    /// other application tools retain their independent controllers.
    fn project_closed_presentation(&mut self, ctx: &egui::Context) {
        self.show_terminal = false;
        self.presentation.select_editor();
        self.set_explorer_visible(egui::ViewportId::ROOT, false);
        self.open_area(areas::Area::Welcome, ctx);
    }

    /// Cancel connection work, clear project ownership and release only this window's private SSH master.
    pub(super) fn reset_workspace(&mut self, reason: LeaveReason) {
        self.workspace.cancel_connection();
        self.clear_project(documents::Retention::Discard, reason);
        self.release_workspace_transport();
        self.invalidate_project_candidate(PickerReset::Invalidate);
    }

    /// Release only this window's transport; remote objects held by old jobs stay invalid.
    pub(super) fn release_workspace_transport(&mut self) {
        self.workspace.cancel_connection();
        self.ssh_master = None;
        if let Some(remote) = self.workspace.release_transport() {
            remote.cleanup();
        }
    }

    /// Cancel pending project preparation and invalidate or reset the associated picker.
    pub(super) fn invalidate_project_candidate(&mut self, picker: PickerReset) {
        self.workspace.cancel_preparation();
        match picker {
            PickerReset::Invalidate => self.project_picker.invalidate(),
            PickerReset::Close => self.project_picker.reset(),
        }
    }
}
