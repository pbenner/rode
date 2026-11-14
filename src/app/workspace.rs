//! Ordered project teardown. Domain owners release views/work before transports;
//! persistent services and acknowledged host-cleanup guards keep their own lifetimes.
use super::*;
mod connection;
pub(super) use connection::ConnectionId;

mod control;
pub(super) use control::{Controller, PreparationId, Prepared};

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
    pub fn message(self) -> &'static str {
        match self {
            Self::VaultLock => "Locking encrypted project",
            Self::NotebooksDisabled => "Notebook extension disabled",
            _ => "Workspace changed",
        }
    }
    pub fn stops_persistent_work(self) -> bool {
        self == Self::VaultLock
    }
}

pub(super) enum PickerReset {
    Invalidate,
    Close,
}

impl EditorApp {
    /// Capture document versions so a late candidate cannot discard edits made during
    /// preparation. Explorer mutations and preparation are mutually exclusive so a
    /// candidate cannot replace a file operation's captured workspace.
    pub(in crate::app) fn prepare_project(
        &mut self,
        path: String,
        options: sandbox::Options,
        remote: bool,
        keep_buffers: bool,
        restore_saved: Option<bool>,
        ctx: &egui::Context,
    ) {
        if self.exit.waiting()
            || self.exit.closing()
            || matches!(
                self.pending.as_ref(),
                Some(Action::Quit | Action::Disconnect | Action::LockEncrypted)
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
        if let Err(error) = options.validate() {
            self.notify(error.to_string(), true);
            return;
        }
        let token = self.workspace.begin_preparation();
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
                token,
                documents_at_start: self.documents.versions(),
                restore_saved,
                open_file: None,
            },
            ctx,
        );
    }

    /// Install an accepted prepared context after common guards pass. Invalidate old
    /// services/generations before starting commands for the new host and policy.
    /// A startup file is opened through the ordinary reader only after this commit.
    pub(in crate::app) fn commit_project(
        &mut self,
        response: Response,
        remote: bool,
        options: sandbox::Options,
        keep_buffers: bool,
        open_file: Option<String>,
        ctx: &egui::Context,
    ) {
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
        self.workspace.activate(
            response.path.clone(),
            connection,
            plan.clone(),
            options.clone(),
        );
        self.vaults.activate_project(encrypted);
        if remote {
            self.restore_ssh_buffers();
        }
        self.explorer_state.expanded.insert(response.path.clone());
        self.explorer_state
            .directories
            .insert(response.path, response.entries);
        self.explorer_state.tree_dirty = true;
        self.load_project_tools(ctx);
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
        self.show_terminal |= remote || options.enabled;
        if let Some(path) = open_file {
            self.open_file(path, ctx);
        } else if self.show_terminal {
            self.start_project_terminal(ctx);
        }
        if opening_connection {
            self.show_explorer = false;
            self.reveal_panel(Panel::Agent, ctx);
            self.agents.focus = true;
        }
    }

    /// Invalidate project work and release window-owned views before transport cleanup.
    /// Same-host policy changes may keep bound buffers; disconnect callers must first
    /// replace their paths with non-authorizing recovery metadata.
    pub(super) fn clear_project(&mut self, retention: documents::Retention, reason: LeaveReason) {
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
        self.stop_latex();
        self.latex = Default::default();
        self.reset_pdf_requests();
        self.leave_project_agent();
        self.leave_notebooks(reason);
        self.reset_editor_contexts(retention);
        self.extensions.reset_project();
        self.workbench.reset_project();
        self.editor.git_gutter.reset();
        self.terminal = None;
        self.session_context_reset();
        self.vaults.release_project();
        self.documents.reset_context(retention);
        self.explorer_state.reset();
        self.file_browser.reset();
        self.editor.input = Default::default();
        self.navigation.cancel_input();
        self.workers.invalidate_workspace();
        self.editor.show_find = false;
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

    pub(super) fn invalidate_project_candidate(&mut self, picker: PickerReset) {
        self.workspace.cancel_preparation();
        match picker {
            PickerReset::Invalidate => self.project_picker.invalidate(),
            PickerReset::Close => self.project_picker.reset(),
        }
    }
}
