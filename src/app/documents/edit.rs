//! Scoped source editing; the owner publishes metadata before any caller can observe the result.
use super::Document;
use eframe::egui;

/// A scoped native/programmatic edit. Mutations publish one source version,
/// dirty state and line count when the scope ends, including during unwinding.
/// No mutable `String` escapes this owner, and unchanged frames do no copying.
#[must_use]
pub(in crate::app) struct Edit<'a> {
    document: &'a mut Document,
    changed: bool,
}
impl<'a> Edit<'a> {
    /// Begin a scoped source edit; Drop publishes metadata if any operation mutated the text.
    /// A mutation followed by its inverse still advances the source version once.
    pub(super) fn new(document: &'a mut Document) -> Self {
        Self {
            document,
            changed: false,
        }
    }
    /// Replace UTF-8 byte offsets captured from this same source snapshot.
    pub(in crate::app) fn replace_range(&mut self, range: std::ops::Range<usize>, text: &str) {
        if &self.document.content.text[range.clone()] != text {
            self.document.content.text.replace_range(range, text);
            self.changed = true;
        }
    }
}
impl egui::TextBuffer for Edit<'_> {
    fn is_mutable(&self) -> bool {
        true
    }
    fn as_str(&self) -> &str {
        &self.document.content.text
    }
    fn insert_text(&mut self, text: &str, index: usize) -> usize {
        let inserted = egui::TextBuffer::insert_text(&mut self.document.content.text, text, index);
        self.changed |= inserted != 0;
        inserted
    }
    fn delete_char_range(&mut self, range: std::ops::Range<usize>) {
        let length = self.document.content.text.len();
        egui::TextBuffer::delete_char_range(&mut self.document.content.text, range);
        self.changed |= length != self.document.content.text.len();
    }
    fn clear(&mut self) {
        self.changed |= !self.document.content.text.is_empty();
        self.document.content.text.clear();
    }
    fn replace_with(&mut self, text: &str) {
        if self.document.content.text != text {
            text.clone_into(&mut self.document.content.text);
            self.changed = true;
        }
    }
    fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.document.content.text);
        self.changed |= !text.is_empty();
        text
    }
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Edit<'static>>()
    }
}
impl Drop for Edit<'_> {
    fn drop(&mut self) {
        if self.changed {
            self.document.source_changed();
        }
    }
}

/// The byte buffer tracks real mutations independently of presentation caches.
pub(super) struct BinaryEdit<'a> {
    document: &'a mut Document,
    revision: u64,
}
impl<'a> BinaryEdit<'a> {
    /// Capture the binary view’s current edit revision so Drop can publish real byte changes once.
    pub(super) fn new(document: &'a mut Document, revision: u64) -> Self {
        Self { document, revision }
    }
    /// Borrow the captured binary view only inside this closure. The view must still exist; its
    /// final revision determines whether document metadata advances.
    pub(super) fn apply<R>(
        &mut self,
        edit: impl FnOnce(&mut super::binary_view::HexView) -> R,
    ) -> R {
        edit(
            self.document
                .content
                .hex
                .as_mut()
                .expect("captured binary view"),
        )
    }
}
impl Drop for BinaryEdit<'_> {
    fn drop(&mut self) {
        let view = self
            .document
            .content
            .hex
            .as_ref()
            .expect("captured binary view");
        if view.buffer.revision() != self.revision {
            let dirty = view.buffer.dirty();
            self.document.binary_changed(dirty);
        }
    }
}
