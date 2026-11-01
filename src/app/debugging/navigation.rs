//! A delayed source read must not replace the selected frame or jump after
//! resume. Serialize debugger-owned source opens; editor I/O stays host-aware.
use super::*;

/// Deferred host source location tied to one stopped inspection epoch; coordinates are one-based.
pub(super) struct Target {
    path: String,
    row: usize,
    column: usize,
    epoch: u64,
    requested: bool,
}
impl Target {
    /// Capture a source location and stopped epoch before a potentially asynchronous file open.
    pub(super) fn new(path: String, row: usize, column: usize, epoch: u64) -> Self {
        Self {
            path,
            row,
            column,
            epoch,
            requested: false,
        }
    }
}
impl EditorApp {
    /// Complete at most one host-aware source read while the same pause/frame epoch
    /// remains current; resume, supersession or a failed read cancels the pending jump.
    pub(super) fn resolve_debug_source(&mut self, client: &Client, ctx: &egui::Context) {
        if client.phase != Phase::Paused
            || self
                .debugger
                .pending_jump
                .as_ref()
                .is_some_and(|target| target.epoch != client.inspection_epoch())
        {
            self.debugger.pending_jump = None;
            return;
        }
        if let Some(path) = &self.debugger.source_loading {
            if self.opening.contains(path) {
                return;
            }
            self.debugger.source_loading = None;
        }
        let Some(target) = &self.debugger.pending_jump else {
            return;
        };
        if let Some(doc) = self
            .documents
            .iter()
            .find(|d| d.path.as_deref() == Some(&target.path) && !d.is_preview())
        {
            let (id, row, column) = (doc.id, target.row, target.column);
            let version = self.debugger.source_version(doc);
            self.debugger.versions.entry(id).or_insert(version);
            self.debugger.pending_jump = None;
            self.jump_to_issue(id, row, column);
            self.focus_editor(ctx);
        } else if target.requested {
            // The completed read failed. Preserve its normal I/O error; do not
            // retry every frame or substitute a desktop-local source path.
            self.debugger.pending_jump = None;
        } else {
            let path = target.path.clone();
            self.debugger.pending_jump.as_mut().unwrap().requested = true;
            self.debugger.source_loading = Some(path.clone());
            self.open_file(path, ctx);
        }
    }
}
