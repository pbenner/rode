//! Keyboard selection over the explorer's cached flattened directory rows.
//!
//! Expand directories lazily and retain selection by host path across refreshes. Navigation
//! requests existing guarded open actions rather than doing filesystem work from key
//! handling.

use super::*;
#[derive(Default)]
pub(in crate::app) struct Selection {
    pub path: Option<String>,
    prefix: String,
    typed: Option<Instant>,
    pub reveal: bool,
    pub offset: f32,
    pub height: f32,
}
impl EditorApp {
    pub(in crate::app) fn tree_keyboard(&mut self, ctx: &egui::Context) {
        let id = Pane::Explorer.id();
        if !ctx.memory(|m| m.has_focus(id)) || self.tree_rows.is_empty() {
            return;
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
            .navigation
            .tree
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
                        .navigation
                        .tree
                        .typed
                        .is_none_or(|at| at.elapsed() > Duration::from_millis(800))
                    {
                        self.navigation.tree.prefix.clear();
                    }
                    self.navigation.tree.typed = Some(Instant::now());
                    if self.navigation.tree.prefix.len() + text.len() <= 128 {
                        self.navigation.tree.prefix.push_str(&text.to_lowercase());
                    }
                    if let Some(found) = (0..self.tree_rows.len())
                        .map(|n| (index + n) % self.tree_rows.len())
                        .find(|i| {
                            self.tree_rows[*i]
                                .entry
                                .name
                                .to_lowercase()
                                .starts_with(&self.navigation.tree.prefix)
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
                        self.list(entry.path.clone(), ctx);
                    }
                }
            }
        } else if open {
            self.editor_intent();
            self.open_file(entry.path, ctx);
        }
        if self.navigation.tree.path.is_none() {
            self.navigation.tree.reveal = true;
        }
        self.navigation.tree.path = Some(self.tree_rows[index].entry.path.clone());
        self.navigation.tree.reveal |= index != old || left || right;
        if escape {
            self.focus_pane(Pane::Editor, ctx);
        }
    }
}
