//! Owns document creation, structural tab mutations, selection and pending reads.
//!
//! Source/save facts are read-only projections. Scoped source/byte edits publish
//! their metadata before returning; views cannot mutate those facts independently.
//! Structural tab ownership and selected IDs stay here across workspace changes.
use super::*;
mod edit;
pub(super) use edit::Edit;

/// Source-only projection for keyboard interpretation; preview policy stays with documents.
pub(super) struct Source<'a> {
    /// Stable identity for edit and navigation admission.
    pub id: u64,
    /// Borrowed source at the current widget boundary.
    pub text: &'a str,
    /// Display/language hint, including recovery paths; never grants file-I/O authority.
    pub path: Option<&'a str>,
}

/// Versioned dirty/save facts shared by transition and exit admission.
#[derive(Default, Clone)]
pub(super) struct Changes {
    /// Exact document/version pairs that a discard decision must approve.
    pub dirty_versions: Vec<(u64, u64)>,
    /// Whether a save is pending within the same document filter as `dirty_versions`.
    pub saving: bool,
}
impl Changes {
    /// Whether any currently dirty document/version pair lacks explicit discard approval. Approving
    /// an older version never approves later edits.
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
    pub(super) notebook: Option<notebook_view::View>,
    content: Content,
    recovery: Option<Recovery>,
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

/// Read-only source and save facts. `Document` exposes this projection through
/// `Deref` without `DerefMut`; mutation must pass through its owner operations.
pub struct Content {
    /// Stable runtime tab identity, preserved when reordering tabs.
    pub id: u64,
    /// Writable/readable path bound to the active project host, or `None` for unsaved/recovery
    /// buffers.
    pub path: Option<String>,
    /// Current source text; structured/preview documents use their dedicated state where
    /// applicable.
    pub text: String,
    /// Exact source snapshot acknowledged by the last successful text save.
    pub saved_text: String,
    /// Content digest read/saved on the bound host, used for conflict detection.
    pub revision: Option<String>,
    /// Whether a source/binary save request is in flight.
    pub saving: bool,
    pub(super) hex: Option<binary_view::HexView>,
}

impl std::ops::Deref for Document {
    type Target = Content;
    fn deref(&self) -> &Self::Target {
        &self.content
    }
}

/// Exact source/version captured before an asynchronous analysis operation.
pub(super) struct SourceSnapshot {
    /// Owned text submitted for analysis.
    pub text: String,
    /// Matching document source version required before applying its result.
    pub version: u64,
}
/// Exact submitted contents and the revision required at their destination.
pub(super) struct SaveSnapshot<T> {
    /// Immutable contents submitted to the host, independent of later buffer edits.
    pub contents: T,
    /// Destination revision used for conflict detection, or `None` without a baseline.
    pub expected: Option<String>,
}

impl Document {
    /// Native widgets and batched byte replacements cannot bypass source metadata.
    pub(super) fn edit_source<R>(&mut self, edit: impl FnOnce(&mut Edit<'_>) -> R) -> R {
        let mut source = Edit::new(self);
        edit(&mut source)
    }
    /// Byte editing has the same publication boundary, using the buffer's own revision.
    pub(super) fn edit_binary<R>(
        &mut self,
        edit: impl FnOnce(&mut binary_view::HexView) -> R,
    ) -> Option<R> {
        let revision = self.content.hex.as_ref()?.buffer.revision();
        let mut source = edit::BinaryEdit::new(self, revision);
        Some(source.apply(edit))
    }
    /// Accept a complete formatter/notebook/template snapshot only when it differs.
    pub(super) fn set_source(&mut self, text: String) {
        if self.content.text != text {
            self.content.text = text;
            self.source_changed();
        }
    }
    /// Prepare the preview against one owned source/version pair without copying the buffer.
    pub(super) fn prepare_markdown(&mut self, path: &str) -> Vec<(String, String)> {
        self.markdown.as_mut().map_or_else(Vec::new, |preview| {
            preview.prepare(self.edit_version, path, &self.content.text)
        })
    }
    /// Revision guard for the destination: a new Save As path has no baseline, while writes to the
    /// current binding retain its captured revision.
    pub(super) fn expected_revision(&self, path: &str, save_as: bool) -> Option<String> {
        if save_as && self.path.as_deref() != Some(path) {
            None
        } else {
            self.revision.clone()
        }
    }
    /// Capture the submitted text and destination revision before marking its save in flight.
    pub(super) fn begin_text_save(
        &mut self,
        path: &str,
        save_as: bool,
    ) -> Option<SaveSnapshot<String>> {
        if self.saving {
            return None;
        }
        let snapshot = SaveSnapshot {
            contents: self.text.clone(),
            expected: self.expected_revision(path, save_as),
        };
        self.content.saving = true;
        Some(snapshot)
    }
    /// Capture immutable binary contents and destination revision, then mark the save in flight.
    /// Return `None` for a duplicate save or non-binary document.
    pub(super) fn begin_binary_save(
        &mut self,
        path: &str,
        save_as: bool,
    ) -> Option<SaveSnapshot<binary_view::SaveContents>> {
        if self.saving {
            return None;
        }
        let snapshot = SaveSnapshot {
            contents: self.hex.as_ref()?.save_snapshot(),
            expected: self.expected_revision(path, save_as),
        };
        self.content.saving = true;
        Some(snapshot)
    }
    /// Analysis captures one version/source pair; saving analyses own the same in-flight flag.
    pub(super) fn analysis_snapshot(&mut self, save: bool) -> Option<SourceSnapshot> {
        if save && self.saving {
            return None;
        }
        let snapshot = SourceSnapshot {
            text: self.text.clone(),
            version: self.version(),
        };
        self.content.saving |= save;
        Some(snapshot)
    }
    /// Synthetic stale/save fixtures may construct state that production cannot publish.
    #[cfg(test)]
    pub(super) fn fixture_content(&mut self) -> &mut Content {
        &mut self.content
    }
    /// Refresh cached source metadata after a test directly mutates synthetic content.
    #[cfg(test)]
    pub(super) fn sync_source_fixture(&mut self) {
        self.source_changed();
    }

    /// Monotonic edit identity used to reject stale analysis and navigation results. Save
    /// acknowledgements do not advance it.
    pub(super) fn version(&self) -> u64 {
        self.edit_version
    }
    /// Cached logical source-line count, maintained by scoped content edits.
    pub(super) fn line_count(&self) -> usize {
        self.lines
    }
    /// Whether the document retains disconnected-host metadata rather than an authorizing file
    /// binding.
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
    fn source_changed(&mut self) {
        self.edit_version = self.edit_version.wrapping_add(1);
        self.modified = self.path.is_none() || self.text != self.saved_text;
        self.lines = self.text.bytes().filter(|b| *b == b'\n').count() + 1;
    }
    /// Binary edits publish their own dirty state but share source reply invalidation.
    fn binary_changed(&mut self, dirty: bool) {
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
        self.content.path = Some(path);
        self.edit_version = self.edit_version.wrapping_add(1);
        if let Some(preview) = &mut self.markdown {
            preview.invalidate();
        }
        if let Some(view) = &mut self.marp {
            view.invalidate();
        }
    }
    /// Install an explicit edit version for stale-reply regression fixtures.
    #[cfg(test)]
    pub(super) fn set_version_fixture(&mut self, version: u64) {
        self.edit_version = version;
    }
    /// Advance only the synthetic edit identity when testing stale asynchronous work.
    #[cfg(test)]
    pub(super) fn advance_version_fixture(&mut self) {
        self.edit_version = self.edit_version.wrapping_add(1);
    }
    /// Override dirty-cache state for transition fixtures without performing a source edit.
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
        self.content.saved_text = text;
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
        if let Some(hex) = &mut self.content.hex {
            hex.buffer.acknowledge(bytes);
            self.modified = hex.buffer.dirty();
        }
        self.acknowledge_binding(path, revision);
    }
    fn acknowledge_binding(&mut self, path: String, revision: String) {
        self.content.path = Some(path);
        self.recovery = None;
        self.content.revision = Some(revision);
        self.content.saving = false;
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
            content: Content {
                id,
                path: None,
                text: String::new(),
                saved_text: String::new(),
                revision: None,
                saving: false,
                hex: None,
            },
            recovery: None,
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
        doc.content.path = Some(std::mem::take(&mut response.path));
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
            && self.content.text == response.text
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
            self.content.hex = Some(binary_view::HexView::new(loaded));
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
        self.content.saved_text.clone_from(&text);
        self.content.text = text;
        self.content.revision = Some(revision);
    }

    /// Cancel source/preview acknowledgements without disposing notebook cleanup owners.
    pub(super) fn invalidate_context(&mut self, include_pdf: bool) {
        self.content.saving = false;
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

/// One structural owner for buffers, tab-group membership and selection. Indices
/// locate shared storage; an editor without tabs has no active document.
pub(super) struct Documents {
    items: Vec<Document>,
    viewport: egui::ViewportId,
    owners: HashMap<u64, egui::ViewportId>,
    selected: HashMap<egui::ViewportId, u64>,
    next_id: u64,
    opening: HashMap<String, egui::ViewportId>,
}

impl Default for Documents {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            viewport: egui::ViewportId::ROOT,
            owners: HashMap::new(),
            selected: HashMap::new(),
            next_id: 1,
            opening: HashMap::new(),
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
            if let Some(path) = doc.content.path.take() {
                doc.recovery = Some(Recovery {
                    path,
                    revision: doc.content.revision.take(),
                    connection: connection.clone(),
                    encrypted,
                });
            }
            if let Some(image) = &mut doc.image {
                image.loading = false;
            }
        }
    }
    /// Whether every buffer lacks an authorizing host path, including untitled and
    /// disconnected-recovery documents.
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
                doc.content.path = Some(origin.path);
                doc.content.revision = origin.revision;
            }
        }
    }
    /// A tool reply may change only the exact document version it captured.
    pub(super) fn apply_source_reply(&mut self, id: u64, version: u64, text: &str) -> Option<u64> {
        let doc = self
            .items
            .iter_mut()
            .find(|doc| doc.id == id && doc.version() == version)?;
        if doc.text != text {
            doc.set_source(text.into());
        }
        Some(doc.version())
    }
    /// Clear an existing document’s in-flight save flag, including after failure. This does not
    /// acknowledge contents or change its saved baseline.
    pub(super) fn finish_save(&mut self, id: u64) {
        if let Some(doc) = self.items.iter_mut().find(|doc| doc.id == id) {
            doc.content.saving = false;
        }
    }
    /// Project the selected buffer only when its current view accepts source editing. Structured
    /// notebook cells, binary and rendered previews are excluded.
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
    /// Dirty-version and save-pending facts across every editor host, used by project replacement
    /// and quit guards.
    pub(super) fn changes(&self) -> Changes {
        self.changes_for(None)
    }
    /// Project dirty versions and save-pending state for one optional document filter.
    /// `None` covers all buffers; a specific ID restricts both facts to that document.
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
    /// Current stable document/version pairs, suitable for capturing precise discard or recovery
    /// decisions.
    pub(super) fn versions(&self) -> Vec<(u64, u64)> {
        self.items
            .iter()
            .map(|doc| (doc.id, doc.edit_version))
            .collect()
    }
    /// Whether the addressed notebook still owns submitted work or an unacknowledged lifetime
    /// change.
    pub(super) fn notebook_pending(&self, id: u64) -> bool {
        self.items
            .iter()
            .find(|doc| doc.id == id)
            .and_then(|doc| doc.notebook.as_ref())
            .is_some_and(|view| view.lifetime_pending())
    }

    /// Current presentation context used by rendering convenience accessors; it does not transfer
    /// document ownership.
    pub(super) fn viewport(&self) -> egui::ViewportId {
        self.viewport
    }
    /// Set the presentation context without changing tab ownership or another viewport’s selection.
    pub(super) fn set_viewport(&mut self, viewport: egui::ViewportId) {
        self.viewport = viewport;
    }
    /// Owning tab-group viewport for a live document ID; closed IDs return `None`.
    pub(super) fn owner(&self, id: u64) -> Option<egui::ViewportId> {
        self.owners.get(&id).copied()
    }
    /// Whether a document belongs to the current presentation context.
    pub(super) fn in_viewport(&self, id: u64) -> bool {
        self.owner(id) == Some(self.viewport)
    }
    fn selected_index_for(&self, viewport: egui::ViewportId) -> Option<usize> {
        self.selected
            .get(&viewport)
            .and_then(|id| {
                self.items
                    .iter()
                    .position(|doc| doc.id == *id && self.owner(doc.id) == Some(viewport))
            })
            .or_else(|| {
                self.items
                    .iter()
                    .position(|doc| self.owner(doc.id) == Some(viewport))
            })
    }
    /// Each tab group's selected preview remains visible independently of native focus.
    pub(super) fn selected_ids(&self) -> HashSet<u64> {
        self.owners
            .values()
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .filter_map(|viewport| {
                self.selected_index_for(viewport)
                    .map(|index| self.items[index].id)
            })
            .collect()
    }
    /// Index selected in the current presentation context, or `self.items.len()` without a
    /// selection. This sentinel also equals zero for an empty collection; use `active` when
    /// absence is possible.
    pub(super) fn active_index(&self) -> usize {
        self.selected_index_for(self.viewport)
            .unwrap_or(if self.items.is_empty() {
                0
            } else {
                self.items.len()
            })
    }
    /// Selected document in the current presentation context, or `None` for an empty window.
    pub(super) fn active(&self) -> Option<&Document> {
        self.items.get(self.active_index())
    }
    /// Mutably borrow the selected document’s owner API in the current presentation context.
    pub(super) fn active_mut(&mut self) -> Option<&mut Document> {
        let index = self.active_index();
        self.items.get_mut(index)
    }
    /// Selected stable document ID in the current presentation context.
    pub(super) fn selected_id(&self) -> Option<u64> {
        self.selected_in(self.viewport)
    }
    /// Operation targets are resolved from a specified host, independently of rendering.
    pub(super) fn selected_in(&self, viewport: egui::ViewportId) -> Option<u64> {
        self.selected_index_for(viewport)
            .map(|index| self.items[index].id)
    }
    /// Find a live buffer by stable ID regardless of which editor owns it.
    pub(super) fn by_id(&self, id: u64) -> Option<&Document> {
        self.items.iter().find(|doc| doc.id == id)
    }
    /// Mutably address a live buffer by stable ID without changing selection or rendering context.
    pub(super) fn by_id_mut(&mut self, id: u64) -> Option<&mut Document> {
        self.items.iter_mut().find(|doc| doc.id == id)
    }
    /// Select a live document in its existing owner’s tab group. Return false for a closed ID;
    /// callers decide whether native focus should move.
    pub(super) fn select(&mut self, id: u64) -> bool {
        let Some(viewport) = self.owner(id) else {
            return false;
        };
        self.selected.insert(viewport, id);
        true
    }
    /// Select in the document's existing host; callers decide whether to raise that host.
    pub(super) fn select_index(&mut self, index: usize) -> bool {
        let Some(doc) = self.items.get(index) else {
            return false;
        };
        self.selected.insert(self.owners[&doc.id], doc.id);
        true
    }
    /// Transfer the sole editor presentation without cloning the buffer or its save owner.
    pub(super) fn move_to(&mut self, id: u64, viewport: egui::ViewportId) -> bool {
        let Some(owner) = self.owners.get_mut(&id) else {
            return false;
        };
        *owner = viewport;
        self.selected.insert(viewport, id);
        true
    }
    /// Closing a presentation returns buffers and pending reads to the main editor.
    pub(super) fn reattach(&mut self, viewport: egui::ViewportId) {
        let selected = self
            .selected_index_for(viewport)
            .map(|index| self.items[index].id);
        self.selected.remove(&viewport);
        for owner in self.owners.values_mut().chain(self.opening.values_mut()) {
            if *owner == viewport {
                *owner = egui::ViewportId::ROOT;
            }
        }
        if let Some(id) = selected.filter(|id| self.owners.contains_key(id)) {
            self.selected.insert(egui::ViewportId::ROOT, id);
        }
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .expect("Document identity space exhausted");
        id
    }

    /// Create and select an untitled buffer in the current presentation context.
    pub(super) fn new_file(&mut self) -> u64 {
        self.new_file_in(self.viewport)
    }
    /// Allocate a stable buffer ID and select it in the named viewport without switching rendering
    /// context.
    pub(super) fn new_file_in(&mut self, viewport: egui::ViewportId) -> u64 {
        let id = self.allocate_id();
        self.items.push(Document::untitled(id));
        self.owners.insert(id, viewport);
        self.selected.insert(viewport, id);
        id
    }

    /// Canonical host replies may name a path already opened by a different alias.
    /// Select the existing buffer without replacing edits or allocating a second ID.
    #[cfg(test)]
    pub(super) fn open(
        &mut self,
        response: Response,
        images: &Arc<markdown::Images>,
        shutdowns: &notebook_view::Shutdowns,
    ) -> u64 {
        self.open_in(self.viewport, response, images, shutdowns)
    }

    /// Install a host read into the named viewport, or select an existing canonical-path buffer
    /// without replacing its edits. Return the live buffer ID.
    pub(super) fn open_in(
        &mut self,
        viewport: egui::ViewportId,
        response: Response,
        images: &Arc<markdown::Images>,
        shutdowns: &notebook_view::Shutdowns,
    ) -> u64 {
        if let Some(index) = self
            .items
            .iter()
            .position(|doc| doc.path.as_deref() == Some(&response.path))
        {
            self.select_index(index);
            return self.items[index].id;
        }
        let id = self.allocate_id();
        self.items
            .push(Document::loaded(id, response, images, shutdowns));
        self.owners.insert(id, viewport);
        self.selected.insert(viewport, id);
        id
    }

    /// Remove one buffer and choose an adjacent tab only in its owning group. Return the removed
    /// document so its resources retain normal drop ownership.
    pub(super) fn close(&mut self, id: u64) -> Option<Document> {
        let index = self.items.iter().position(|doc| doc.id == id)?;
        let owner = self.owner(id)?;
        let doc = self.items.remove(index);
        self.owners.remove(&id);
        if self.selected.get(&owner) == Some(&id) {
            let next = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, d)| self.owner(d.id) == Some(owner))
                .find(|(i, _)| *i >= index)
                .or_else(|| {
                    self.items
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(_, d)| self.owner(d.id) == Some(owner))
                })
                .map(|(_, d)| d.id);
            self.selected.remove(&owner);
            if let Some(next) = next {
                self.selected.insert(owner, next);
            }
        }
        Some(doc)
    }

    /// Cycle selection within the current viewport, wrapping in either direction. Other windows’
    /// selections are unchanged.
    pub(super) fn cycle(&mut self, direction: isize) {
        let indices: Vec<_> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, doc)| self.in_viewport(doc.id))
            .map(|(i, _)| i)
            .collect();
        if let Some(index) = indices.iter().position(|i| *i == self.active_index()) {
            self.select_index(
                indices[(index as isize + direction).rem_euclid(indices.len() as isize) as usize],
            );
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
        let document = self.items.remove(from);
        let target = self.items.iter().position(|doc| doc.id == target).unwrap();
        self.items.insert(target + usize::from(after), document);
        true
    }

    /// Drop all buffers, ownership, selections and pending reads while preserving the monotonic
    /// document-ID allocator.
    pub(super) fn clear(&mut self) {
        self.items.clear();
        self.owners.clear();
        self.selected.clear();
        self.opening.clear();
    }

    /// Invalidate pending reads and save flags on a workspace change. Retained buffers keep
    /// identities; discard mode clears the collection.
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

    /// Reserve a test read for the current presentation context; duplicate paths are rejected.
    #[cfg(test)]
    pub(super) fn begin_open(&mut self, path: String) -> bool {
        self.begin_open_in(path, self.viewport)
    }
    /// Capture a new read’s destination viewport. Return false while that path already has a
    /// pending read.
    pub(super) fn begin_open_in(&mut self, path: String, viewport: egui::ViewportId) -> bool {
        if self.opening.contains_key(&path) {
            return false;
        }
        self.opening.insert(path, viewport);
        true
    }
    /// Remove a pending-read marker and return its current destination, which may have been
    /// redirected by window return.
    pub(super) fn finish_open(&mut self, path: &str) -> Option<egui::ViewportId> {
        self.opening.remove(path)
    }
    /// Inspect a pending read’s destination without consuming the marker.
    pub(super) fn open_destination(&self, path: &str) -> Option<egui::ViewportId> {
        self.opening.get(path).copied()
    }
    /// Whether a path has a reserved read awaiting completion or admission.
    pub(super) fn is_opening(&self, path: &str) -> bool {
        self.opening.contains_key(path)
    }
    /// Whether any reserved read is awaiting completion; Explorer mutations must wait for it.
    pub(super) fn has_pending_opens(&self) -> bool {
        !self.opening.is_empty()
    }
    /// Forget all pending reads when their workspace context is invalidated.
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
