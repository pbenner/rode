//! Crash snapshot projection and unbound restoration belong to document ownership.
use super::*;

impl Documents {
    /// Capture all tab groups without leaking vault plaintext into desktop storage.
    pub(in crate::app) fn crash_entries(
        &mut self,
        encrypted: bool,
    ) -> anyhow::Result<(Vec<super::crash_recovery::Entry>, bool)> {
        let mut excluded = false;
        let mut entries = Vec::new();
        for doc in self.items.iter_mut() {
            doc.encrypted_source |= encrypted;
            if !doc.dirty() {
                continue;
            }
            if doc.encrypted_source || doc.hex().is_some() {
                excluded = true;
                continue;
            }
            let text = match doc.notebook() {
                Some(view) if !view.source_mode() => view.book.serialize()?,
                _ => doc.text.clone(),
            };
            entries.push(super::crash_recovery::Entry {
                name: doc
                    .display_path()
                    .map(str::to_owned)
                    .unwrap_or_else(|| doc.name()),
                text,
                notebook: doc.notebook().is_some(),
            });
        }
        Ok((entries, excluded))
    }
    /// Recovery retains only a display name. It cannot rebind or overwrite a host file.
    pub(in crate::app) fn restore_crash_entry(
        &mut self,
        entry: &super::crash_recovery::Entry,
        images: &Arc<markdown::Images>,
        shutdowns: &notebook_view::Shutdowns,
    ) -> u64 {
        let id = self.new_file_in(egui::ViewportId::ROOT);
        let doc = self.by_id_mut(id).unwrap();
        doc.set_source(entry.text.clone());
        doc.recovery = Some(Recovery {
            path: entry.name.clone(),
            revision: None,
            connection: None,
        });
        if entry.notebook
            && let Ok(book) = crate::notebook::Notebook::parse(&entry.text)
        {
            doc.view = Representation::Notebook(Some(Box::new(notebook_view::View::new(
                book,
                images.clone(),
                shutdowns.clone(),
            ))));
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_captures_large_drafts_without_a_configuration_size_limit() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = EditorApp::new(&cc, Startup::default());
        let large = "x".repeat(9 * 1024 * 1024);
        for text in [large.as_str(), "another unsaved draft"] {
            let id = app.documents.new_file();
            app.documents.by_id_mut(id).unwrap().set_source(text.into());
        }
        let (entries, excluded) = app.documents.crash_entries(false).unwrap();
        assert!(!excluded);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, large);
        assert_eq!(entries[1].text, "another unsaved draft");
    }

    #[test]
    fn recovery_keeps_remote_names_without_authorizing_file_io_and_excludes_vault_plaintext() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = EditorApp::new(&cc, Startup::default());
        let entry = crate::app::crash_recovery::Entry {
            name: "/a/remote/project/notes.txt".into(),
            text: "unsaved\n日本語".into(),
            notebook: false,
        };
        app.documents
            .restore_crash_entry(&entry, &app.markdown_images, &app.notebook_shutdowns);
        let doc = app.documents.active().unwrap();
        assert!(doc.dirty());
        assert!(doc.path.is_none());
        assert!(doc.revision.is_none());
        assert_eq!(doc.display_path(), Some(entry.name.as_str()));
        assert_eq!(doc.text, entry.text);
        let connection = workspace::ConnectionId::for_profile(&Profile::default());
        app.documents.restore_connection(&connection, true);
        assert!(app.documents.active().unwrap().path.is_none());
        let untitled = app.documents.new_file();
        app.documents
            .by_id_mut(untitled)
            .unwrap()
            .set_source("secret scratch draft".into());
        let (entries, excluded) = app.documents.crash_entries(true).unwrap();
        assert!(entries.is_empty() && excluded);
        app.documents.detach_connection(&connection, true);
        // A formerly encrypted retained document stays private even when the
        // desktop returns to its ordinary local CWD.
        let (entries, excluded) = app.documents.crash_entries(false).unwrap();
        assert!(entries.is_empty() && excluded);
    }

    #[test]
    fn structured_notebooks_recover_the_current_cells_not_the_original_source() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = EditorApp::new(&cc, Startup::default());
        let source = r##"{"cells":[{"cell_type":"markdown","metadata":{},"source":["# Edited notebook"]}],"metadata":{},"nbformat":4,"nbformat_minor":5}"##;
        let entry = crate::app::crash_recovery::Entry {
            name: "work.ipynb".into(),
            text: source.into(),
            notebook: true,
        };
        app.documents
            .restore_crash_entry(&entry, &app.markdown_images, &app.notebook_shutdowns);
        assert!(app.documents.active().unwrap().notebook().is_some());
        app.documents
            .active_mut()
            .unwrap()
            .notebook_mut()
            .unwrap()
            .book
            .cells[0]
            .set_source("# Current structured cell".into());
        let (entries, excluded) = app.documents.crash_entries(false).unwrap();
        assert!(!excluded);
        assert!(entries[0].notebook);
        assert!(entries[0].text.contains("Current structured cell"));
        assert!(!entries[0].text.contains("Edited notebook"));
    }
}
