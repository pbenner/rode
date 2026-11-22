//! Document-owned response to project changes, feature toggles and vault locks.
//! Callers select a lifecycle operation, without inspecting individual viewers.
use super::*;
use crate::extensions::Feature;

impl Document {
    /// Stop derived work while retaining editable buffers, undo history and
    /// notebook shutdown ownership. Reconnect may keep an existing paged view.
    pub(super) fn invalidate_previews(&mut self, include_pdf: bool) {
        if include_pdf && let Some(view) = self.pdf_mut() {
            view.invalidate();
        }
        if let Some(view) = self.markdown_mut() {
            view.invalidate();
        }
        if let Some(view) = self.marp_mut() {
            view.invalidate();
        }
    }

    fn disable_feature(&mut self, id: &str) {
        let paged_feature = self.paged_feature();
        match &mut self.view {
            Representation::Source(views) => {
                if id == Feature::Marp.id()
                    && let Some(view) = &mut views.marp
                {
                    view.invalidate();
                    view.showing = false;
                }
                if (id == Feature::Markdown.id() || id == Feature::Images.id())
                    && let Some(view) = &mut views.markdown
                {
                    view.invalidate();
                    if id == Feature::Markdown.id() {
                        view.showing = false;
                    }
                }
            }
            Representation::Notebook(Some(view)) if id == Feature::Notebooks.id() => {
                view.leave(workspace::LeaveReason::NotebooksDisabled);
            }
            Representation::Paged(view)
                if paged_feature.is_some_and(|feature| id == feature.id()) =>
            {
                view.invalidate();
            }
            _ => {}
        }
    }
}

impl Documents {
    /// Disabling a feature releases executable work through each document's
    /// owner. The document and its unsaved contents remain available for saving.
    pub(in crate::app) fn disable_feature(&mut self, id: &str) {
        for doc in &mut self.items {
            doc.disable_feature(id);
        }
    }

    /// Locking an encrypted project also stops persistent notebook kernels.
    /// Ordinary project changes honor their existing persistence policy.
    pub(in crate::app) fn leave_notebooks(&mut self, reason: workspace::LeaveReason) {
        for doc in &mut self.items {
            if let Some(view) = doc.notebook_mut() {
                view.leave(reason);
            }
        }
    }

    /// Vault locking invalidates derived jobs without clearing pending saves.
    pub(in crate::app) fn invalidate_previews(&mut self) {
        for doc in &mut self.items {
            doc.invalidate_previews(true);
        }
    }
}
