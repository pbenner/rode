//! Snapshot-bound hunk restoration. Git remains read-only; the editor adapter
//! records one native undo step and leaves disk/index writes to explicit Save/stage.
use super::*;
use std::ops::Range;

/// Only the inspector can mint a revert for the document and baseline it displayed.
pub(in crate::app) struct Request {
    key: Key,
    version: u64,
    index: usize,
    baseline: Arc<str>,
}

impl State {
    pub(super) fn revert_request(&self, doc: &Document, index: usize) -> Option<Request> {
        self.current(doc)?.hunks.get(index)?;
        Some(Request {
            key: self.key.clone()?,
            version: doc.version(),
            index,
            baseline: self.baseline.as_ref()?.text.clone(),
        })
    }

    /// Reject edits, document switches, renames and baseline refreshes since the click.
    fn replacement(&self, doc: &Document, request: &Request) -> Option<(Range<usize>, String)> {
        if doc.version() != request.version
            || self.key.as_ref() != Some(&request.key)
            || !Arc::ptr_eq(&self.baseline.as_ref()?.text, &request.baseline)
        {
            return None;
        }
        let hunk = self.current(doc)?.hunks.get(request.index)?;
        Some(replacement(&request.baseline, &doc.text, hunk))
    }
}

/// Translate logical line ranges back into original UTF-8 bytes. Restore final
/// newlines exactly and use the buffer's line endings without rewriting its context.
fn replacement(old: &str, new: &str, hunk: &diff::Hunk) -> (Range<usize>, String) {
    let offset = |text: &str, line| text.split_inclusive('\n').take(line).map(str::len).sum();
    let range = offset(new, hunk.new.start)..offset(new, hunk.new.end);
    let original = &old[offset(old, hunk.old.start)..offset(old, hunk.old.end)];
    let endings = |text: &str| {
        text.find('\n')
            .map(|at| at > 0 && text.as_bytes()[at - 1] == b'\r')
    };
    let crlf = endings(&new[range.clone()])
        .or_else(|| endings(new))
        .or_else(|| endings(old))
        .unwrap_or(false);
    let mut text = original.replace("\r\n", "\n");
    if crlf {
        text = text.replace('\n', "\r\n");
    }
    (range, text)
}

impl EditorApp {
    /// Restore only the inspected block, retaining ordinary native undo/redo history.
    pub(in crate::app) fn revert_git_change(&mut self, request: Request, ctx: &egui::Context) {
        let Some(doc) = self.documents.active_mut() else {
            return;
        };
        let Some((range, replacement)) = self.git_gutter.replacement(doc, &request) else {
            return;
        };
        let id = egui::Id::new(("document", doc.id));
        let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        let cursor = state.cursor.char_range().unwrap_or_else(|| {
            egui::text::CCursorRange::one(egui::text::CCursor::new(doc.last_cursor))
        });
        let mut undo = state.undoer();
        undo.add_undo(&(cursor, doc.text.clone()));
        let point = doc.text[..range.start].chars().count();
        let row = doc.text[..range.start]
            .bytes()
            .filter(|b| *b == b'\n')
            .count()
            + 1;
        doc.edit_source(|source| source.replace_range(range, &replacement));
        let cursor = egui::text::CCursorRange::one(egui::text::CCursor::new(point));
        let after = (cursor, doc.text.clone());
        undo.feed_state(ctx.input(|input| input.time), &after);
        undo.add_undo(&after);
        state.set_undoer(undo);
        state.cursor.set_char_range(Some(cursor));
        state.store(ctx, id);
        doc.selection = Some((point, point));
        doc.last_cursor = point;
        doc.cursor = (row, 1);
        self.git_gutter.peek = None;
        self.focus_editor(ctx);
        self.status = "Change reverted in editor · Save to update the file".into();
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests;
