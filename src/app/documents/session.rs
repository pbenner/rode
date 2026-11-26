//! Saved workspace projection and revision-aware draft restoration. File paths
//! are rebound only after an ordinary read on the restored project host.
use super::*;
use crate::app::workspace_sessions::model;
/// Distinguish host revision conflicts from edits made in the destination view.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::app) enum Restore {
    Applied,
    FileChanged,
    Edited,
}
impl Document {
    pub(in crate::app) fn workspace_private(&self) -> bool {
        self.encrypted_source
    }
    /// Reuse the crash checkpoint format for ordinary saved-session drafts.
    pub(super) fn checkpoint_entry(
        &self,
    ) -> anyhow::Result<Option<crate::app::crash_recovery::Entry>> {
        if !self.dirty() || self.encrypted_source || self.hex().is_some() {
            return Ok(None);
        }
        let text = match self.notebook() {
            Some(view) if !view.source_mode() => view.book.serialize()?,
            _ => self.text.clone(),
        };
        Ok(Some(crate::app::crash_recovery::Entry {
            name: self
                .display_path()
                .map(str::to_owned)
                .unwrap_or_else(|| self.name()),
            text,
            notebook: self.notebook().is_some(),
        }))
    }
    /// Apply a captured draft only over its matching saved baseline. A changed
    /// host file leaves the draft unbound, requiring an explicit Save As.
    pub(in crate::app) fn restore_workspace_state(
        &mut self,
        saved: &model::Document,
        images: &Arc<markdown::Images>,
        shutdowns: &notebook_view::Shutdowns,
    ) -> Restore {
        self.encrypted_source |= saved.private;
        if self.dirty() && self.path.is_some() {
            return Restore::Edited;
        }
        let conflict = saved.draft.is_some() && saved.revision != self.revision;
        if let Some(entry) = &saved.draft {
            self.set_source(entry.text.clone());
            if entry.notebook
                && let Ok(book) = crate::notebook::Notebook::parse(&entry.text)
            {
                self.view = Representation::Notebook(Some(Box::new(notebook_view::View::new(
                    book,
                    images.clone(),
                    shutdowns.clone(),
                ))));
            }
            if conflict {
                self.content.path = None;
                self.content.revision = None;
                self.recovery = Some(Recovery {
                    path: saved.name.clone(),
                    revision: None,
                    connection: None,
                });
            }
        }
        if let Some(view) = self.notebook_mut() {
            view.restore_source_mode(saved.notebook_source);
        }
        let line = self
            .text
            .split('\n')
            .nth(saved.cursor.0.saturating_sub(1))
            .unwrap_or_default();
        let column = saved.cursor.1.saturating_sub(1).min(line.chars().count());
        let offset = self
            .text
            .split_inclusive('\n')
            .take(saved.cursor.0.saturating_sub(1))
            .map(|s| s.chars().count())
            .sum::<usize>()
            + column;
        let offset = offset.min(self.text.chars().count());
        self.selection = Some((offset, offset));
        self.cursor = saved.cursor;
        if conflict {
            Restore::FileChanged
        } else {
            Restore::Applied
        }
    }
}
impl Documents {
    pub(in crate::app) fn workspace_signature(&self) -> serde_json::Value {
        serde_json::json!(
            self.items
                .iter()
                .map(|d| (
                    d.id,
                    &d.path,
                    d.version(),
                    d.dirty(),
                    d.cursor,
                    d.notebook().is_some_and(|n| n.source_mode()),
                    self.owner(d.id).map(|v| v.0.value()),
                    self.owner(d.id).and_then(|v| self.selected_in(v)) == Some(d.id)
                ))
                .collect::<Vec<_>>()
        )
    }
    pub(in crate::app) fn workspace_documents(
        &mut self,
        root: &str,
        encrypted: bool,
    ) -> anyhow::Result<Vec<model::Document>> {
        let mut saved = Vec::new();
        for doc in &mut self.items {
            doc.encrypted_source |= encrypted;
            let relative = encrypted
                && doc
                    .path
                    .as_ref()
                    .is_some_and(|p| std::path::Path::new(p).strip_prefix(root).is_ok());
            let path = doc.path.as_ref().map(|p| {
                if relative {
                    std::path::Path::new(p)
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                } else {
                    p.clone()
                }
            });
            let owner = self
                .owners
                .get(&doc.id)
                .copied()
                .unwrap_or(egui::ViewportId::ROOT);
            saved.push(model::Document {
                key: doc.id,
                path,
                name: doc.name(),
                relative,
                revision: doc.revision.clone(),
                cursor: doc.cursor,
                group: if owner == egui::ViewportId::ROOT {
                    0
                } else {
                    owner.0.value()
                },
                selected: self.selected.get(&owner) == Some(&doc.id),
                private: doc.encrypted_source,
                notebook_source: doc.notebook().is_some_and(|n| n.source_mode()),
                draft: doc.checkpoint_entry()?,
            });
        }
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn saved(revision: &str) -> model::Document {
        model::Document {
            key: 1,
            path: Some("notes.txt".into()),
            name: "notes.txt".into(),
            relative: false,
            revision: Some(revision.into()),
            cursor: (2, 2),
            group: 0,
            selected: true,
            private: false,
            notebook_source: false,
            draft: Some(crate::app::crash_recovery::Entry {
                name: "notes.txt".into(),
                text: "Draft\n日本語\n".into(),
                notebook: false,
            }),
        }
    }
    #[test]
    fn matching_baselines_restore_bound_drafts_and_changed_files_require_save_as() {
        let images = Arc::new(markdown::Images::default());
        let shutdowns = notebook_view::Shutdowns::default();
        for (revision, conflict) in [("baseline", false), ("changed", true)] {
            let mut document = Document::loaded(
                1,
                Response {
                    path: "notes.txt".into(),
                    text: "Host contents\n".into(),
                    revision: revision.into(),
                    ..Default::default()
                },
                &images,
                &shutdowns,
            );
            assert_eq!(
                document.restore_workspace_state(&saved("baseline"), &images, &shutdowns)
                    == Restore::FileChanged,
                conflict
            );
            assert!(document.dirty());
            assert_eq!(document.text, "Draft\n日本語\n");
            assert_eq!(document.selection, Some((7, 7)));
            assert_eq!(document.path.is_none(), conflict);
            assert_eq!(document.revision.is_none(), conflict);
        }
    }
    #[test]
    fn restore_never_overwrites_edits_in_a_destination_buffer() {
        let images = Arc::new(markdown::Images::default());
        let shutdowns = notebook_view::Shutdowns::default();
        let mut document = Document::loaded(
            1,
            Response {
                path: "notes.txt".into(),
                text: "Host contents".into(),
                revision: "baseline".into(),
                ..Default::default()
            },
            &images,
            &shutdowns,
        );
        document.set_source("Typed while restoration was pending".into());
        let mut snapshot = saved("baseline");
        snapshot.private = true;
        snapshot.draft = None;
        assert_eq!(
            document.restore_workspace_state(&snapshot, &images, &shutdowns),
            Restore::Edited
        );
        assert_eq!(document.text, "Typed while restoration was pending");
        assert_eq!(document.path.as_deref(), Some("notes.txt"));
        assert!(document.workspace_private());
    }
    #[test]
    fn private_file_references_keep_their_exclusion_after_restoration() {
        let images = Arc::new(markdown::Images::default());
        let shutdowns = notebook_view::Shutdowns::default();
        let mut document = Document::loaded(
            1,
            Response {
                path: "notes.txt".into(),
                text: "Decrypted source".into(),
                revision: "baseline".into(),
                ..Default::default()
            },
            &images,
            &shutdowns,
        );
        let mut snapshot = saved("baseline");
        snapshot.private = true;
        snapshot.draft = None;
        document.restore_workspace_state(&snapshot, &images, &shutdowns);
        document.set_source("Edited decrypted source".into());
        assert!(document.dirty());
        assert!(document.workspace_private());
        assert!(document.checkpoint_entry().unwrap().is_none());
    }
    #[test]
    fn raw_notebook_drafts_stay_visible_even_with_incomplete_json() {
        let images = Arc::new(markdown::Images::default());
        let shutdowns = notebook_view::Shutdowns::default();
        let book = crate::notebook::Notebook::default();
        let mut document = Document::loaded(
            1,
            Response {
                path: "work.ipynb".into(),
                text: book.serialize().unwrap(),
                notebook: Some(Ok(book)),
                revision: "baseline".into(),
                ..Default::default()
            },
            &images,
            &shutdowns,
        );
        let mut snapshot = saved("baseline");
        snapshot.notebook_source = true;
        snapshot.draft.as_mut().unwrap().notebook = true;
        snapshot.draft.as_mut().unwrap().text = "{\"cells\": [".into();
        document.restore_workspace_state(&snapshot, &images, &shutdowns);
        assert!(document.notebook().unwrap().source_mode());
        assert!(document.dirty());
        assert_eq!(
            document.checkpoint_entry().unwrap().unwrap().text,
            "{\"cells\": ["
        );
    }
}
