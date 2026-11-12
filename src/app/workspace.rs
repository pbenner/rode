//! Ordered project teardown. Domain owners release views/work before transports;
//! persistent services and acknowledged host-cleanup guards keep their own lifetimes.
use super::*;

pub(super) enum PickerReset {
    Invalidate,
    Close,
}

impl EditorApp {
    /// Invalidate project work and release window-owned views before transport cleanup.
    /// Same-host policy changes may keep bound buffers; disconnect callers must first
    /// replace their paths with non-authorizing recovery metadata.
    pub(super) fn clear_project(&mut self, retention: documents::Retention) {
        self.reset_debugger();
        self.stop_latex();
        self.latex = Default::default();
        self.reset_pdf_requests();
        self.leave_project_agent();
        self.stop_notebook_kernels("Workspace changed");
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
        self.navigation.editing = Default::default();
        self.generation += 1;
        self.show_find = false;
    }

    /// Cancel connection work, clear project ownership and release only this window's private SSH master.
    pub(super) fn reset_workspace(&mut self) {
        self.connection_cancel.store(true, Ordering::Release);
        self.clear_project(documents::Retention::Discard);
        self.release_workspace_transport();
        self.invalidate_project_candidate(PickerReset::Invalidate);
    }

    /// Release only this window's transport; remote objects held by old jobs stay invalid.
    pub(super) fn release_workspace_transport(&mut self) {
        self.connection_cancel.store(true, Ordering::Release);
        self.ssh_master = None;
        if let Some(remote) = self.workspace.take().and_then(|workspace| workspace.remote) {
            remote.cleanup();
        }
    }

    pub(super) fn invalidate_project_candidate(&mut self, picker: PickerReset) {
        self.preparing = false;
        self.project_token = self.project_token.wrapping_add(1);
        match picker {
            PickerReset::Invalidate => self.project_picker.invalidate(),
            PickerReset::Close => self.project_picker.reset(),
        }
    }
}
