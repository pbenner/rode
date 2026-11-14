//! Owns document creation, structural tab mutations, selection and pending reads.
//!
//! Views may edit buffers through a mutable slice, but cannot insert/remove tabs
//! or change the selected index independently. IDs never reset on workspace changes.
use super::*;

/// Source-only projection for keyboard interpretation; preview policy stays with documents.
pub(super) struct Source<'a> {
    pub id: u64,
    pub text: &'a str,
    pub path: Option<&'a str>,
}

/// Versioned dirty/save facts shared by transition and exit admission.
#[derive(Default, Clone)]
pub(super) struct Changes {
    pub dirty_versions: Vec<(u64, u64)>,
    pub saving: bool,
}
impl Changes {
    pub fn unapproved(&self, versions: &[(u64, u64)]) -> bool {
        self.dirty_versions
            .iter()
            .any(|version| !versions.contains(version))
    }
}

/// Former host bindings are display/reconnect metadata, never file-I/O authority.
struct Recovery {
    path: String,
    revision: Option<String>,
    connection: workspace::ConnectionId,
    encrypted: bool,
}

/// One stable tab identity and editable source/save baseline, with optional specialized
/// document state. Only bound paths authorize project-host file operations; recovery paths are
/// display/reconnect metadata.
pub struct Document {
    pub(super) markdown: Option<markdown::Preview>,
    pub(super) marp: Option<marp_view::View>,
    pub(super) image: Option<image_view::ImageView>,
    pub(super) pdf: Option<pdf_view::PdfView>,
    pub(super) hex: Option<binary_view::HexView>,
    pub(super) notebook: Option<notebook_view::View>,
    /// Stable runtime tab identity, preserved when reordering tabs.
    pub id: u64,
    /// Writable/readable path bound to the active project host, or `None` for unsaved/recovery
    /// buffers.
    pub path: Option<String>,
    recovery: Option<Recovery>,
    /// Current source text; structured/preview documents use their dedicated state where
    /// applicable.
    pub text: String,
    /// Exact source snapshot acknowledged by the last successful text save.
    pub saved_text: String,
    /// Content digest read/saved on the bound host, used for conflict detection.
    pub revision: Option<String>,
    /// Whether a source/binary save request is in flight.
    pub saving: bool,
    /// Cached dirty state, including structured notebook or binary edits.
    modified: bool,
    /// Source identity for worker results; acknowledging a save does not advance it.
    edit_version: u64,
    /// Last native source cursor position in characters; `usize::MAX` forces initial recomputation.
    pub(super) last_cursor: usize,
    /// Displayed one-based source line and character column.
    pub cursor: (usize, usize),
    /// Requested native selection as zero-based character offsets.
    pub(super) selection: Option<(usize, usize)>,
    lines: usize,
}

impl Document {
    pub(super) fn version(&self) -> u64 {
        self.edit_version
    }
    pub(super) fn line_count(&self) -> usize {
        self.lines
    }
    pub(super) fn is_recovery(&self) -> bool {
        self.recovery.is_some()
    }
    /// Non-authorizing former path for recovery labels and Save As suggestions.
    pub(super) fn recovery_path(&self) -> Option<&str> {
        self.recovery
            .as_ref()
            .map(|recovery| recovery.path.as_str())
    }
    /// Complete one accepted source mutation, including programmatic edits and undo.
    /// Save acknowledgements retain this version so later edits cannot be overwritten.
    pub(super) fn source_changed(&mut self) {
        self.edit_version = self.edit_version.wrapping_add(1);
        self.modified = self.path.is_none() || self.text != self.saved_text;
        self.lines = self.text.bytes().filter(|b| *b == b'\n').count() + 1;
    }
    /// Binary edits publish their own dirty state but share source reply invalidation.
    pub(super) fn binary_changed(&mut self, dirty: bool) {
        self.edit_version = self.edit_version.wrapping_add(1);
        self.modified = dirty;
    }
    /// A new decoder request invalidates earlier replies without making a file dirty.
    pub(super) fn begin_preview_reload(&mut self) -> u64 {
        self.edit_version = self.edit_version.wrapping_add(1);
        self.edit_version
    }
    /// A successful rename changes host identity and invalidates source-derived previews.
    pub(super) fn renamed(&mut self, path: String) {
        self.path = Some(path);
        self.edit_version = self.edit_version.wrapping_add(1);
        if let Some(preview) = &mut self.markdown {
            preview.invalidate();
        }
        if let Some(view) = &mut self.marp {
            view.invalidate();
        }
    }
    #[cfg(test)]
    pub(super) fn set_version_fixture(&mut self, version: u64) {
        self.edit_version = version;
    }
    #[cfg(test)]
    pub(super) fn advance_version_fixture(&mut self) {
        self.edit_version = self.edit_version.wrapping_add(1);
    }
    #[cfg(test)]
    pub(super) fn set_dirty_fixture(&mut self, dirty: bool) {
        self.modified = dirty;
    }
    /// Identify specialized documents whose bytes/source use a dedicated editor or preview path.
    pub(super) fn is_preview(&self) -> bool {
        self.image.is_some() || self.pdf.is_some() || self.hex.is_some() || self.notebook.is_some()
    }
    /// Report an in-flight image/PDF decode without treating source-only documents as previews.
    pub(super) fn preview_loading(&self) -> bool {
        self.image.as_ref().is_some_and(|image| image.loading)
            || self.pdf.as_ref().is_some_and(|pdf| pdf.image.loading)
    }
    /// Return the cached edit state maintained by ordinary source/structured editing paths.
    pub fn dirty(&self) -> bool {
        self.modified
    }
    /// Acknowledge the submitted text snapshot, updating path/revision while preserving newer
    /// unsaved edits.
    pub fn acknowledge_save(&mut self, path: String, text: String, revision: String) {
        self.saved_text = text;
        self.modified = self.text != self.saved_text;
        self.acknowledge_binding(path, revision);
    }
    /// Acknowledge exactly the submitted bytes while preserving later binary edits.
    pub(super) fn acknowledge_binary_save(
        &mut self,
        path: String,
        bytes: Arc<Vec<u8>>,
        revision: String,
    ) {
        if let Some(hex) = &mut self.hex {
            hex.buffer.acknowledge(bytes);
            self.modified = hex.buffer.dirty();
        }
        self.acknowledge_binding(path, revision);
    }
    fn acknowledge_binding(&mut self, path: String, revision: String) {
        self.path = Some(path);
        self.recovery = None;
        self.revision = Some(revision);
        self.saving = false;
    }
    /// Recovery paths describe retained tabs; only `path` authorizes file I/O.
    pub(super) fn display_path(&self) -> Option<&str> {
        self.path
            .as_deref()
            .or_else(|| self.recovery.as_ref().map(|r| r.path.as_str()))
    }
    /// Derive a tab label from its bound/recovery path, falling back to its stable untitled ID.
    pub(super) fn name(&self) -> String {
        self.display_path()
            .map(ssh::display_name)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Untitled-{}", self.id))
    }
}

impl Document {
    /// Create a dirty unsaved buffer with the same cursor/save defaults for every caller.
    fn untitled(id: u64) -> Self {
        Self {
            id,
            path: None,
            recovery: None,
            text: String::new(),
            saved_text: String::new(),
            revision: None,
            saving: false,
            modified: true,
            edit_version: 0,
            last_cursor: usize::MAX,
            cursor: (1, 1),
            selection: None,
            lines: 1,
            markdown: None,
            marp: None,
            image: None,
            pdf: None,
            hex: None,
            notebook: None,
        }
    }

    fn loaded(
        id: u64,
        mut response: Response,
        images: &Arc<markdown::Images>,
        shutdowns: &notebook_view::Shutdowns,
    ) -> Self {
        let mut doc = Self::untitled(id);
        doc.path = Some(std::mem::take(&mut response.path));
        doc.replace_views(&mut response, images, shutdowns);
        doc.replace_source(response.text, response.revision);
        doc.modified = false;
        doc
    }

    /// Apply only a current, clean reload; the worker coordinator checks version/host first.
    /// Unchanged notebook reads retain the live kernel and its cleanup ownership.
    pub(super) fn reload(
        &mut self,
        mut response: Response,
        images: &Arc<markdown::Images>,
        shutdowns: &notebook_view::Shutdowns,
    ) {
        if response.notebook.is_some()
            && self.notebook.is_some()
            && self.text == response.text
            && self.revision.as_deref() == Some(&response.revision)
        {
            return;
        }
        self.replace_views(&mut response, images, shutdowns);
        self.replace_source(response.text, response.revision);
        self.edit_version = self.edit_version.wrapping_add(1);
    }

    fn replace_views(
        &mut self,
        response: &mut Response,
        images: &Arc<markdown::Images>,
        shutdowns: &notebook_view::Shutdowns,
    ) {
        if let Some(result) = response.notebook.take() {
            self.notebook = result
                .ok()
                .map(|book| notebook_view::View::new(book, images.clone(), shutdowns.clone()));
        }
        if let Some(loaded) = response.hex.take() {
            self.notebook = None;
            self.hex = Some(binary_view::HexView::new(loaded));
            self.pdf = None;
            self.image = None;
            self.markdown = None;
        }
        if let Some(loaded) = response.pdf.take() {
            if let Some(pdf) = &mut self.pdf {
                pdf.replace(loaded);
            } else {
                self.pdf = Some(pdf_view::PdfView::new(loaded));
            }
        }
        if let Some(loaded) = response.preview.take() {
            if let Some(image) = &mut self.image {
                image.replace(loaded);
            } else {
                self.image = Some(image_view::ImageView::new(loaded));
            }
        }
    }

    fn replace_source(&mut self, text: String, revision: String) {
        self.lines = text.bytes().filter(|b| *b == b'\n').count() + 1;
        self.saved_text.clone_from(&text);
        self.text = text;
        self.revision = Some(revision);
    }

    /// Cancel source/preview acknowledgements without disposing notebook cleanup owners.
    pub(super) fn invalidate_context(&mut self, include_pdf: bool) {
        self.saving = false;
        if include_pdf && let Some(pdf) = &mut self.pdf {
            pdf.invalidate();
        }
        if let Some(preview) = &mut self.markdown {
            preview.invalidate();
        }
        if let Some(view) = &mut self.marp {
            view.invalidate();
        }
    }
}

/// Whether replacing a workspace retains recoverable buffers or discards the tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Retention {
    Keep,
    Discard,
}
impl From<bool> for Retention {
    fn from(keep: bool) -> Self {
        if keep { Self::Keep } else { Self::Discard }
    }
}

/// One structural owner for buffers. An empty collection uses index zero only as
/// an inert display sentinel; active()/active_mut() return None until insertion.
pub(super) struct Documents {
    items: Vec<Document>,
    active: usize,
    next_id: u64,
    opening: HashSet<String>,
}

impl Default for Documents {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            active: 0,
            next_id: 1,
            opening: HashSet::new(),
        }
    }
}

impl Documents {
    /// Forced host loss unbinds every path before transport release or generation changes.
    pub(super) fn detach_connection(
        &mut self,
        connection: &workspace::ConnectionId,
        encrypted: bool,
    ) {
        for doc in &mut self.items {
            if let Some(path) = doc.path.take() {
                doc.recovery = Some(Recovery {
                    path,
                    revision: doc.revision.take(),
                    connection: connection.clone(),
                    encrypted,
                });
            }
            if let Some(image) = &mut doc.image {
                image.loading = false;
            }
        }
    }
    pub(super) fn all_unbound(&self) -> bool {
        self.items.iter().all(|doc| doc.path.is_none())
    }
    /// Only a ready matching host and, when required, a recovered encrypted lease can rebind.
    pub(super) fn restore_connection(
        &mut self,
        connection: &workspace::ConnectionId,
        encrypted_ready: bool,
    ) {
        for doc in &mut self.items {
            if doc.recovery.as_ref().is_some_and(|origin| {
                &origin.connection == connection && (!origin.encrypted || encrypted_ready)
            }) {
                let origin = doc.recovery.take().unwrap();
                doc.path = Some(origin.path);
                doc.revision = origin.revision;
            }
        }
    }
    pub(super) fn finish_save(&mut self, id: u64) {
        if let Some(doc) = self.items.iter_mut().find(|doc| doc.id == id) {
            doc.saving = false;
        }
    }
    pub(super) fn active_source(&self) -> Option<Source<'_>> {
        let doc = self.active()?;
        if doc.image.is_some()
            || doc.pdf.is_some()
            || doc.hex.is_some()
            || doc.notebook.as_ref().is_some_and(|v| !v.source_mode())
            || doc.markdown.as_ref().is_some_and(|v| v.showing)
            || doc.marp.as_ref().is_some_and(|v| v.showing)
        {
            return None;
        }
        Some(Source {
            id: doc.id,
            text: &doc.text,
            path: doc.display_path(),
        })
    }
    pub(super) fn changes(&self) -> Changes {
        self.changes_for(None)
    }
    pub(super) fn changes_for(&self, document: Option<u64>) -> Changes {
        Changes {
            dirty_versions: self
                .items
                .iter()
                .filter(|doc| document.is_none_or(|id| id == doc.id) && doc.dirty())
                .map(|doc| (doc.id, doc.edit_version))
                .collect(),
            saving: self
                .items
                .iter()
                .any(|doc| document.is_none_or(|id| id == doc.id) && doc.saving),
        }
    }
    pub(super) fn versions(&self) -> Vec<(u64, u64)> {
        self.items
            .iter()
            .map(|doc| (doc.id, doc.edit_version))
            .collect()
    }
    pub(super) fn notebook_pending(&self, id: u64) -> bool {
        self.items
            .iter()
            .find(|doc| doc.id == id)
            .and_then(|doc| doc.notebook.as_ref())
            .is_some_and(|view| view.lifetime_pending())
    }

    pub(super) fn active_index(&self) -> usize {
        self.active
    }
    pub(super) fn active(&self) -> Option<&Document> {
        self.items.get(self.active)
    }
    pub(super) fn active_mut(&mut self) -> Option<&mut Document> {
        self.items.get_mut(self.active)
    }
    pub(super) fn selected_id(&self) -> Option<u64> {
        self.active().map(|doc| doc.id)
    }

    pub(super) fn select_index(&mut self, index: usize) -> bool {
        if index >= self.items.len() {
            return false;
        }
        self.active = index;
        true
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .expect("Document identity space exhausted");
        id
    }

    pub(super) fn new_file(&mut self) -> u64 {
        let id = self.allocate_id();
        self.items.push(Document::untitled(id));
        self.active = self.items.len() - 1;
        id
    }

    /// Canonical host replies may name a path already opened by a different alias.
    /// Select the existing buffer without replacing edits or allocating a second ID.
    pub(super) fn open(
        &mut self,
        response: Response,
        images: &Arc<markdown::Images>,
        shutdowns: &notebook_view::Shutdowns,
    ) -> u64 {
        if let Some(index) = self
            .items
            .iter()
            .position(|doc| doc.path.as_deref() == Some(&response.path))
        {
            self.active = index;
            return self.items[index].id;
        }
        let id = self.allocate_id();
        self.items
            .push(Document::loaded(id, response, images, shutdowns));
        self.active = self.items.len() - 1;
        id
    }

    pub(super) fn close(&mut self, id: u64) -> Option<Document> {
        let index = self.items.iter().position(|doc| doc.id == id)?;
        let doc = self.items.remove(index);
        if self.active > index {
            self.active -= 1;
        }
        self.active = self.active.min(self.items.len().saturating_sub(1));
        Some(doc)
    }

    pub(super) fn cycle(&mut self, direction: isize) {
        if !self.items.is_empty() {
            self.active =
                (self.active as isize + direction).rem_euclid(self.items.len() as isize) as usize;
        }
    }

    /// Preserve selection by ID while positions change. Missing/stale drag targets are inert.
    pub(super) fn reorder(&mut self, source: u64, target: u64, after: bool) -> bool {
        if source == target || !self.items.iter().any(|doc| doc.id == target) {
            return false;
        }
        let Some(from) = self.items.iter().position(|doc| doc.id == source) else {
            return false;
        };
        let selected = self.selected_id();
        let document = self.items.remove(from);
        let target = self.items.iter().position(|doc| doc.id == target).unwrap();
        self.items.insert(target + usize::from(after), document);
        self.active = self
            .items
            .iter()
            .position(|doc| Some(doc.id) == selected)
            .unwrap_or(0);
        true
    }

    pub(super) fn clear(&mut self) {
        self.items.clear();
        self.active = 0;
        self.opening.clear();
    }

    pub(super) fn reset_context(&mut self, retention: Retention) {
        self.cancel_opens();
        if retention == Retention::Discard {
            self.clear();
        }
        for doc in &mut self.items {
            doc.invalidate_context(true);
        }
    }

    /// Reconnecting keeps document/PDF views while cancelling old saves and source previews.
    pub(super) fn reconnecting(&mut self) {
        self.cancel_opens();
        for doc in &mut self.items {
            doc.invalidate_context(false);
        }
    }

    pub(super) fn begin_open(&mut self, path: String) -> bool {
        self.opening.insert(path)
    }
    pub(super) fn finish_open(&mut self, path: &str) {
        self.opening.remove(path);
    }
    pub(super) fn is_opening(&self, path: &str) -> bool {
        self.opening.contains(path)
    }
    pub(super) fn has_pending_opens(&self) -> bool {
        !self.opening.is_empty()
    }
    pub(super) fn cancel_opens(&mut self) {
        self.opening.clear();
    }
}

impl std::ops::Deref for Documents {
    type Target = [Document];
    fn deref(&self) -> &Self::Target {
        &self.items
    }
}
impl std::ops::DerefMut for Documents {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.items
    }
}
impl<'a> IntoIterator for &'a Documents {
    type Item = &'a Document;
    type IntoIter = std::slice::Iter<'a, Document>;
    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}
impl<'a> IntoIterator for &'a mut Documents {
    type Item = &'a mut Document;
    type IntoIter = std::slice::IterMut<'a, Document>;
    fn into_iter(self) -> Self::IntoIter {
        self.items.iter_mut()
    }
}

#[cfg(test)]
mod tests;
