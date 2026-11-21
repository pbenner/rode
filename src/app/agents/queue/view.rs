//! Window-local queue geometry and rendering. Host queue commands remain in
//! the parent module; resizing, scrolling and theme changes never submit work.
use super::{Action, Edit, Prompt, Queue, valid};
use crate::app::{
    agents::{splitter, text_submit},
    icons::{Icon, IconButton},
    layout, ui_probe,
};
use crate::theme::Palette;
use eframe::egui::{self, Align, Layout, Rect, RichText};

const COMPACT_WIDTH: f32 = 600.;

#[derive(Default)]
pub(super) struct State {
    pub height: splitter::State,
    reveal: Option<u64>,
}

impl State {
    pub fn clear(&mut self) {
        self.height.cancel_drag();
        self.reveal = None;
    }
}

impl Queue {
    /// Keep queue/composer sizing independent and retain the preferred queue
    /// height across empty queues, hidden history and temporarily small windows.
    pub fn layout(
        &mut self,
        ui: &mut egui::Ui,
        bounds: Rect,
        visible: bool,
        enabled: bool,
    ) -> (Rect, Rect) {
        if !visible || self.is_empty() {
            self.view.height.cancel_drag();
            return (bounds, Rect::from_min_max(bounds.max, bounds.max));
        }
        let control = layout::control_height(ui).max(24.);
        let spacing = ui.spacing().item_spacing.y;
        let compact = bounds.width() < COMPACT_WIDTH;
        let rows: f32 = self
            .prompts
            .iter()
            .take(3)
            .map(|prompt| {
                control
                    + 16.
                    + spacing
                    + if compact { control + spacing } else { 0. }
                    + if prompt.files.is_empty() {
                        0.
                    } else {
                        ui.text_style_height(&egui::TextStyle::Small) + spacing
                    }
            })
            .sum();
        let header = control + 16. + spacing;
        let minimum = header + control;
        let maximum = bounds.height() * 0.65;
        let default = (header + rows + if self.edit.is_some() { 150. } else { 0. })
            .min(bounds.height() * 0.45)
            .max(minimum.min(maximum));
        let split = self.view.height.bottom(
            ui,
            bounds,
            splitter::Size {
                default,
                minimum,
                maximum,
                extra: 0.,
            },
            "agent-queue-resize",
            "Queued prompts height",
            enabled,
        );
        (split.content, split.panel)
    }

    /// Align prompt previews before their actions. Only controls are disabled
    /// while offline/busy; the queued text remains readable and scrollable.
    pub fn ui(&mut self, ui: &mut egui::Ui, palette: Palette, enabled: bool, can_resume: bool) {
        let mut action = None;
        let reveal = self.view.reveal.take();
        let panel = egui::Frame::new()
            .fill(palette.panel)
            .stroke(egui::Stroke::new(1_f32, palette.border))
            .corner_radius(palette.ui.unwrap_or_default().control_radius)
            .inner_margin(6)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size().max(egui::Vec2::ZERO));
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let toggle = ui.add_enabled(
                            enabled && (!self.paused || can_resume),
                            IconButton::toolbar(
                                if self.paused { Icon::Play } else { Icon::Pause },
                                if self.paused { "Resume queue" } else { "Pause queue" },
                            ),
                        );
                        ui_probe::button(
                            ui,
                            if self.paused { "agent-queue-resume" } else { "agent-queue-pause" },
                            &toggle,
                        );
                        if toggle.clicked() {
                            self.set_paused(!self.paused);
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("Queued ({})", self.prompts.len())).strong(),
                                ).truncate(),
                            ).on_hover_text("Prompts run in order after a successful reply. Stop, errors and editing hold pending work.");
                            if self.edit.is_some() {
                                ui.weak("Editing");
                            } else if self.paused {
                                ui.weak("Paused");
                            }
                        });
                    });
                });
                egui::ScrollArea::vertical()
                    .id_salt("queued-prompts")
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .auto_shrink([false, false])
                    .min_scrolled_height(0.)
                    .max_height(ui.available_height().max(1.))
                    .show(ui, |ui| {
                        for (index, prompt) in self.prompts.iter().enumerate() {
                            ui.push_id(prompt.id, |ui| {
                                let (row, row_action) = row(
                                    ui, prompt, index, self.prompts.len(),
                                    &mut self.edit, palette, enabled,
                                );
                                if row_action.is_some() {
                                    action = row_action;
                                }
                                if reveal == Some(prompt.id) {
                                    ui.scroll_to_rect(row, Some(Align::Min));
                                }
                            });
                            ui.add_space(6.);
                        }
                    });
            });
        ui_probe::button(ui, "agent-queue-panel", &panel.response);
        if let Some(action) = action {
            self.view.reveal = match &action {
                Action::Edit(id) | Action::Move(id, _) => Some(*id),
                Action::Save | Action::Cancel => self.edit.as_ref().map(|edit| edit.id),
                Action::Remove(_) => None,
            };
            self.apply(action);
            ui.ctx().request_repaint();
        }
    }
}

fn row(
    ui: &mut egui::Ui,
    prompt: &Prompt,
    index: usize,
    count: usize,
    editing: &mut Option<Edit>,
    palette: Palette,
    enabled: bool,
) -> (Rect, Option<Action>) {
    let mut action = None;
    let compact = ui.available_width() < COMPACT_WIDTH;
    let frame = layout::card_frame(palette).inner_margin(6).show(ui, |ui| {
        let controls = |ui: &mut egui::Ui, action: &mut Option<Action>| {
            ui.add_enabled_ui(enabled, |ui| {
                // Right-to-left keeps the same visual order while reserving
                // the actual themed control widths before truncating text.
                let delete = ui.add(IconButton::toolbar(Icon::Close, "Delete queued prompt"));
                ui_probe::button(ui, &format!("agent-queue-{index}-delete"), &delete);
                if delete.clicked() {
                    *action = Some(Action::Remove(prompt.id));
                }
                let edit = ui.add_enabled(
                    editing.is_none(),
                    crate::keyboard::Button::new("Edit")
                        .small()
                        .min_size(egui::vec2(0., 24.)),
                );
                ui_probe::button(ui, &format!("agent-queue-{index}-edit"), &edit);
                if edit.clicked() {
                    *action = Some(Action::Edit(prompt.id));
                }
                let later = ui.add_enabled(
                    index + 1 < count,
                    IconButton::toolbar(Icon::ChevronDown, "Move later"),
                );
                ui_probe::button(ui, &format!("agent-queue-{index}-later"), &later);
                if later.clicked() {
                    *action = Some(Action::Move(prompt.id, false));
                }
                let earlier = ui.add_enabled(
                    index > 0,
                    IconButton::toolbar(Icon::ChevronUp, "Move earlier"),
                );
                ui_probe::button(ui, &format!("agent-queue-{index}-earlier"), &earlier);
                if earlier.clicked() {
                    *action = Some(Action::Move(prompt.id, true));
                }
            });
        };
        if compact {
            preview(ui, prompt, index);
            ui.horizontal(|ui| {
                ui.with_layout(
                    Layout::right_to_left(Align::Center).with_main_wrap(true),
                    |ui| controls(ui, &mut action),
                );
            });
        } else {
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    controls(ui, &mut action);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        preview(ui, prompt, index)
                    });
                });
            });
        }
        if !prompt.files.is_empty() {
            ui.add(
                egui::Label::new(palette.tag(format!("{} attachments", prompt.files.len())))
                    .truncate(),
            )
            .on_hover_text(prompt.files.join("\n"));
        }
        if let Some(edit) = editing.as_mut().filter(|edit| edit.id == prompt.id) {
            ui.add_enabled_ui(enabled, |ui| {
                let mut save_by_key = false;
                // Long edits scroll inside the row, keeping Save/Cancel outside
                // the editor's clip. The outer list can still reveal the row.
                egui::ScrollArea::vertical()
                    .id_salt("queued-edit-scroll")
                    .max_height(120.)
                    .show(ui, |ui| {
                        let id = ui.make_persistent_id(("queued-edit", edit.id));
                        save_by_key = text_submit(ui, id, !enabled, &mut edit.composing);
                        let input = ui
                            .add(crate::keyboard::named(
                                "Queued prompt",
                                egui::TextEdit::multiline(&mut edit.text)
                                    .id(id)
                                    .desired_rows(3)
                                    .desired_width(f32::INFINITY)
                                    .return_key(egui::KeyboardShortcut::new(
                                        egui::Modifiers::SHIFT,
                                        egui::Key::Enter,
                                    ))
                                    .char_limit(crate::agent::PROMPT_LIMIT),
                            ))
                            .on_hover_text("Enter to save · Shift+Enter for a new line");
                        ui_probe::button(ui, "agent-queue-edit-input", &input);
                        ui_probe::value(ui, "agent-queue-edit-input-value", &input, &edit.text);
                    });
                ui.horizontal_wrapped(|ui| {
                    let can_save = valid(&edit.text)
                        || edit.text.len() <= crate::agent::PROMPT_LIMIT
                            && !prompt.files.is_empty();
                    let save = ui
                        .add_enabled(
                            can_save,
                            crate::keyboard::Button::new("Save prompt").small(),
                        )
                        .on_hover_text("Save the edited queued prompt · Enter");
                    ui_probe::button(ui, "agent-queue-save", &save);
                    if can_save && (save.clicked() || save_by_key) {
                        action = Some(Action::Save);
                    }
                    let cancel = crate::keyboard::small_button(ui, "Cancel edit");
                    ui_probe::button(ui, "agent-queue-cancel-edit", &cancel);
                    if cancel.clicked() {
                        action = Some(Action::Cancel);
                    }
                });
            });
        }
    });
    ui_probe::button(ui, &format!("agent-queue-row-{index}"), &frame.response);
    (frame.response.rect, action)
}

fn preview(ui: &mut egui::Ui, prompt: &Prompt, index: usize) {
    ui.horizontal(|ui| {
        ui.weak(format!("{}.", index + 1));
        let summary = ui
            .add(
                egui::Label::new(
                    prompt
                        .text
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or_default(),
                )
                .truncate(),
            )
            .on_hover_text(&prompt.text);
        ui_probe::value(
            ui,
            &format!("agent-queue-{index}-value"),
            &summary,
            &prompt.text,
        );
    });
}
