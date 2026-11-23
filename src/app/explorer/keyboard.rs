//! Keyboard selection over the explorer's cached flattened directory rows.
//!
//! Expand directories lazily and retain selection by host path across refreshes. Navigation
//! requests existing guarded open actions rather than doing filesystem work from key
//! handling.

use super::*;
use navigation::Pane;
enum TreeIntent {
    List(String),
    Open(String),
    FocusEditor,
}

#[derive(Default)]
/// Explorer keyboard selection, bounded type-ahead prefix and scroll-reveal state.
pub(in crate::app) struct Selection {
    pub path: Option<String>,
    prefix: String,
    typed: Option<Instant>,
    typed_viewport: Option<egui::ViewportId>,
    pub reveal: bool,
    pub scroll: HashMap<egui::ViewportId, (f32, f32)>,
}
impl Selection {
    fn reset_typeahead(&mut self) {
        self.prefix.clear();
        self.typed = None;
    }
}
impl EditorApp {
    /// Handle explorer keys only while its pane owns focus, selecting cached rows
    /// and delegating opens/listings through the ordinary host-aware app paths.
    pub(in crate::app) fn tree_keyboard(&mut self, ctx: &egui::Context) {
        let id = Pane::Explorer.id_in(ctx.viewport_id());
        let focused = ctx.input(|input| input.focused) && ctx.memory(|m| m.has_focus(id));
        if !focused && self.explorer_state.selection.typed_viewport == Some(ctx.viewport_id()) {
            self.explorer_state.selection.reset_typeahead();
        }
        if !focused
            || self.explorer_state.tree_rows.is_empty()
            || egui::Popup::is_any_open(ctx)
            || self.dialog != Dialog::None
            || self.pending.modal()
            || self.workbench_modal()
        {
            return;
        }
        for intent in self.explorer_state.keyboard(ctx) {
            match intent {
                TreeIntent::List(path) => self.list(path, ctx),
                TreeIntent::Open(path) => self.open_file(path, ctx),
                TreeIntent::FocusEditor => self.focus_pane(Pane::Editor, ctx),
            }
        }
    }
}

impl State {
    /// Consume keys only after the coordinator has established native pane focus.
    fn keyboard(&mut self, ctx: &egui::Context) -> Vec<TreeIntent> {
        let mut intents = Vec::new();
        let id = Pane::Explorer.id_in(ctx.viewport_id());
        if self.selection.typed_viewport != Some(ctx.viewport_id()) {
            self.selection.reset_typeahead();
            self.selection.typed_viewport = Some(ctx.viewport_id());
        }
        ctx.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    tab: false,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
        let mut index = self
            .selection
            .path
            .as_ref()
            .and_then(|path| self.tree_rows.iter().position(|r| r.entry.path == *path))
            .unwrap_or(0);
        let old = index;
        let mut open = false;
        let mut left = false;
        let mut right = false;
        let mut escape = false;
        ctx.input_mut(|input| {
            for event in &input.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                    && modifiers.is_none()
                {
                    match key {
                        Key::ArrowDown => index = (index + 1).min(self.tree_rows.len() - 1),
                        Key::ArrowUp => index = index.saturating_sub(1),
                        Key::Home => index = 0,
                        Key::End => index = self.tree_rows.len() - 1,
                        Key::PageDown => index = (index + 10).min(self.tree_rows.len() - 1),
                        Key::PageUp => index = index.saturating_sub(10),
                        Key::ArrowLeft => left = true,
                        Key::ArrowRight => right = true,
                        Key::Enter => open = true,
                        Key::Escape => escape = true,
                        _ => {}
                    }
                } else if let egui::Event::Text(text) = event
                    && !input.modifiers.ctrl
                    && !input.modifiers.alt
                    && !input.modifiers.command
                {
                    if self
                        .selection
                        .typed
                        .is_none_or(|at| at.elapsed() > Duration::from_millis(800))
                    {
                        self.selection.prefix.clear();
                    }
                    self.selection.typed = Some(Instant::now());
                    if self.selection.prefix.len() + text.len() <= 128 {
                        self.selection.prefix.push_str(&text.to_lowercase());
                    }
                    if let Some(found) = (0..self.tree_rows.len())
                        .map(|n| (index + n) % self.tree_rows.len())
                        .find(|i| {
                            self.tree_rows[*i]
                                .entry
                                .name
                                .to_lowercase()
                                .starts_with(&self.selection.prefix)
                        })
                    {
                        index = found;
                    }
                }
            }
            input.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Text(_)
                        | egui::Event::Key {
                            pressed: true,
                            key: Key::ArrowDown
                                | Key::ArrowUp
                                | Key::ArrowLeft
                                | Key::ArrowRight
                                | Key::Home
                                | Key::End
                                | Key::PageUp
                                | Key::PageDown
                                | Key::Enter
                                | Key::Escape,
                            ..
                        }
                )
            });
        });
        let entry = self.tree_rows[index].entry.clone();
        let expanded = self.expanded.contains(&entry.path);
        if left {
            if entry.dir && expanded {
                self.expanded.remove(&entry.path);
                self.tree_dirty = true;
            } else if let Some(parent) = (0..index)
                .rev()
                .find(|i| self.tree_rows[*i].depth < self.tree_rows[index].depth)
            {
                index = parent;
            }
        }
        if right || (open && entry.dir) {
            if entry.dir {
                if expanded && right {
                    if index + 1 < self.tree_rows.len()
                        && self.tree_rows[index + 1].depth > self.tree_rows[index].depth
                    {
                        index += 1;
                    }
                } else if expanded {
                    self.expanded.remove(&entry.path);
                    self.tree_dirty = true;
                } else {
                    self.expanded.insert(entry.path.clone());
                    self.tree_dirty = true;
                    if !self.directories.contains_key(&entry.path) {
                        intents.push(TreeIntent::List(entry.path.clone()));
                    }
                }
            }
        } else if open {
            intents.push(TreeIntent::Open(entry.path));
        }
        if self.selection.path.is_none() {
            self.selection.reveal = true;
        }
        self.selection.path = Some(self.tree_rows[index].entry.path.clone());
        self.selection.reveal |= index != old || left || right;
        // Activating a row or leaving the tree completes this search. A quick
        // return must start a new prefix without waiting for the idle timeout.
        if open || escape {
            self.selection.reset_typeahead();
        }
        if escape {
            intents.push(TreeIntent::FocusEditor);
        }
        intents
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_file_ends_typeahead_before_an_immediate_new_search() {
        let ctx = egui::Context::default();
        let mut state = State {
            tree_rows: ["b-first.txt", "c-second.txt"]
                .into_iter()
                .map(|name| TreeRow {
                    entry: Entry {
                        name: name.into(),
                        path: format!("/project/{name}"),
                        dir: false,
                    },
                    depth: 0,
                })
                .collect(),
            ..Default::default()
        };
        let _ = ctx.run(egui::RawInput {
            events: vec![egui::Event::Text("b".into()), egui::Event::Key {
                key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default(),
            }],
            ..Default::default()
        }, |ctx| {
            assert!(matches!(&state.keyboard(ctx)[..], [TreeIntent::Open(path)] if path == "/project/b-first.txt"));
        });
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Text("c".into())],
                ..Default::default()
            },
            |ctx| {
                state.keyboard(ctx);
            },
        );
        assert_eq!(
            state.selection.path.as_deref(),
            Some("/project/c-second.txt")
        );
    }
}
