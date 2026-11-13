//! Ordered project teardown. Domain owners release views/work before transports;
//! persistent services and acknowledged host-cleanup guards keep their own lifetimes.
use super::*;

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
    /// Invalidate project work and release window-owned views before transport cleanup.
    /// Same-host policy changes may keep bound buffers; disconnect callers must first
    /// replace their paths with non-authorizing recovery metadata.
    pub(super) fn clear_project(&mut self, retention: documents::Retention, reason: LeaveReason) {
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
        if retention == documents::Retention::Discard {
            self.tabs = Default::default();
        }
        self.extensions.reset_project();
        self.workbench.reset_project();
        self.git_gutter.reset();
        self.terminal = None;
        self.session_context_reset();
        for client in [
            self.vaults.active.take(),
            self.vaults.staged.take(),
            self.vaults.pending.take(),
        ]
        .into_iter()
        .flatten()
        {
            self.vaults.release(client);
        }
        self.documents.reset_context(retention);
        self.explorer_state.reset();
        self.file_browser.reset();
        self.input = Default::default();
        self.navigation.cancel_input();
        self.workers.invalidate_workspace();
        self.show_find = false;
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
