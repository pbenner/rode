//! Chat-input layout and scrolling. TextEdit's desired row count is a minimum,
//! so a long draft needs its own bounded viewport rather than a clipped parent.
//! Keep one editor ID for cursor/undo continuity and reserve the action row
//! outside the scroller. Resizing is local to this window and never submits text.

use eframe::egui::{self, Align, Key, Rect, Sense, Stroke, Vec2};

const DEFAULT_HEIGHT: f32 = 140.;
const MIN_HEIGHT: f32 = 118.;
const WARNING_HEIGHT: f32 = 22.;
const TRANSCRIPT_MIN_HEIGHT: f32 = 160.;

pub(super) struct State {
    /// Preferred height without the optional unsaved-file row. Keep it when a
    /// temporarily small window clamps the actual panel or history hides input.
    height: f32,
    drag_start: Option<f32>,
    history: Vec<String>,
    recall: Option<Recall>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            height: DEFAULT_HEIGHT,
            drag_start: None,
            history: Vec::new(),
            recall: None,
        }
    }
}

/// Temporary editable copies of sent prompts, ending with the unsent draft.
struct Recall {
    drafts: Vec<String>,
    position: usize,
}

pub(super) struct Layout {
    pub content: Rect,
    pub input: Rect,
    pub stretch: f32,
}

impl State {
    pub fn reset_recall(&mut self) {
        self.recall = None;
    }

    /// Retain each submitted prompt independently of the rolling transcript.
    pub fn remember_prompt(&mut self, text: String) {
        if !text.trim().is_empty() && text.len() <= crate::agent::PROMPT_LIMIT {
            self.history.push(text);
        }
    }

    pub fn clear_history(&mut self) {
        self.reset_recall();
        self.history.clear();
    }

    pub fn restore_history(&mut self, messages: impl Iterator<Item = String>) {
        self.clear_history();
        for text in messages {
            self.remember_prompt(text);
        }
    }

    /// Only bare arrows at an unselected character zero browse the current chat.
    pub fn recall(&mut self, ui: &mut egui::Ui, draft: &mut String, id: egui::Id, enabled: bool) {
        if !enabled
            || !ui.memory(|memory| memory.has_focus(id) && memory.allows_interaction(ui.layer_id()))
        {
            return;
        }
        let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) else {
            return;
        };
        if !state
            .cursor
            .char_range()
            .is_some_and(|range| range.is_empty() && range.primary.index == 0)
        {
            return;
        }
        let key = ui.input(|input| {
            // Let TextEdit handle batched edits and IME without replacing their buffer.
            if input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Cut
                        | egui::Event::Ime(_)
                )
            }) {
                return None;
            }
            input.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key: key @ (Key::ArrowUp | Key::ArrowDown),
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                    ..
                } => Some(*key),
                _ => None,
            })
        });
        let Some(key) = key else {
            return;
        };
        if self.recall.is_none() {
            if key != Key::ArrowUp {
                return;
            }
            let mut drafts = self.history.clone();
            if drafts.is_empty() {
                return;
            }
            let position = drafts.len();
            drafts.push(draft.clone());
            self.recall = Some(Recall { drafts, position });
        }
        let recall = self.recall.as_mut().unwrap();
        recall.drafts[recall.position].clone_from(draft);
        recall.position = if key == Key::ArrowUp {
            recall.position.saturating_sub(1)
        } else {
            (recall.position + 1).min(recall.drafts.len() - 1)
        };
        draft.clone_from(&recall.drafts[recall.position]);
        if recall.position == recall.drafts.len() - 1 {
            self.recall = None;
        }
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(0),
            )));
        state.store(ui.ctx(), id);
        ui.ctx()
            .data_mut(|data| data.insert_temp(id.with("recall-reveal"), true));
        ui.input_mut(|input| input.events.retain(|event| !matches!(event,
            egui::Event::Key { key: pressed, pressed: true, modifiers: egui::Modifiers::NONE, .. } if *pressed == key)));
    }

    pub fn layout(
        &mut self,
        ui: &mut egui::Ui,
        bounds: Rect,
        warning: bool,
        hidden: bool,
        enabled: bool,
    ) -> Layout {
        if hidden {
            self.drag_start = None;
            return Layout {
                content: bounds,
                input: Rect::from_min_max(bounds.max, bounds.max),
                stretch: 0.,
            };
        }
        let extra = if warning { WARNING_HEIGHT } else { 0. };
        let maximum = (bounds.height() - TRANSCRIPT_MIN_HEIGHT)
            .max(MIN_HEIGHT + extra)
            .min(bounds.height())
            .max(0.);
        let minimum = (MIN_HEIGHT + extra).min(maximum);
        let height = (self.height + extra).clamp(minimum, maximum);
        let top = bounds.bottom() - height;
        let handle = Rect::from_min_max(
            egui::pos2(bounds.left(), (top - 6.).max(bounds.top())),
            egui::pos2(bounds.right(), top + 2.),
        );
        let sense = if enabled {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        };
        let response = ui.interact(handle, ui.id().with("agent-composer-resize"), sense)
            .on_hover_cursor(egui::CursorIcon::ResizeVertical)
            .on_hover_text("Drag to resize message input · Double-click to reset\nWhen focused: Up/Down to resize, Home to reset");
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::DragValue, enabled, "Message input height")
        });
        let mut resized = height;
        if enabled {
            if response.drag_started() {
                self.drag_start = Some(height);
            }
            if response.dragged() {
                resized = (self.drag_start.unwrap_or(height)
                    - response.total_drag_delta().unwrap_or_default().y)
                    .clamp(minimum, maximum);
                self.height = resized - extra;
            }
            if response.double_clicked()
                || (response.has_focus()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Home)))
            {
                self.height = DEFAULT_HEIGHT;
                resized = (self.height + extra).clamp(minimum, maximum);
            } else if response.has_focus() {
                for (key, step) in [(Key::ArrowUp, 20.), (Key::ArrowDown, -20.)] {
                    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                        resized = (resized + step).clamp(minimum, maximum);
                        self.height = resized - extra;
                    }
                }
            }
            if response.drag_stopped() {
                self.drag_start = None;
                ui.memory_mut(|m| m.request_focus(egui::Id::new("agent-composer")));
            }
        } else {
            self.drag_start = None;
        }
        let input = Rect::from_min_max(
            egui::pos2(bounds.left(), bounds.bottom() - resized),
            bounds.max,
        );
        let center = egui::pos2(bounds.center().x, input.top() - 2.);
        let color = if response.hovered() || response.dragged() || response.has_focus() {
            ui.visuals().selection.stroke.color
        } else {
            ui.visuals().weak_text_color()
        };
        ui.painter().line_segment(
            [center - egui::vec2(18., 0.), center + egui::vec2(18., 0.)],
            Stroke::new(2.0_f32, color),
        );
        Layout {
            content: Rect::from_min_max(
                bounds.min,
                egui::pos2(bounds.right(), (input.top() - 6.).max(bounds.top())),
            ),
            input,
            stretch: resized - extra - DEFAULT_HEIGHT,
        }
    }
}

/// Scroll only the draft. Text edits and cursor movement use TextEdit's native
/// scroll requests; a size change also reveals the caret, while manual wheel
/// scrolling remains undisturbed on unchanged frames.
pub(super) fn editor(
    ui: &mut egui::Ui,
    text: &mut String,
    id: egui::Id,
    stretch: f32,
) -> egui::containers::scroll_area::ScrollAreaOutput<egui::text_edit::TextEditOutput> {
    let font = egui::FontSelection::Default.resolve(ui.style());
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let available =
        (ui.available_height() - ui.spacing().interact_size.y - ui.spacing().item_spacing.y - 2.)
            .max(1.);
    let height = (3. * row_height + 4. + stretch).clamp(1., available);
    let visual = ui.visuals().widgets.inactive;
    let frame = egui::Frame::new()
        .inner_margin(0)
        .fill(ui.visuals().extreme_bg_color)
        .stroke(visual.bg_stroke)
        .corner_radius(visual.corner_radius);
    let area = frame.show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("agent-composer-scroll")
            .max_height(height)
            .min_scrolled_height(1.)
            .auto_shrink([false, false])
            .animated(false)
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: false,
                ..Default::default()
            })
            .show_viewport(ui, |ui, viewport| {
                let size_id = id.with("viewport-size");
                let old = ui.ctx().data_mut(|d| d.get_temp::<Vec2>(size_id));
                let output = egui::TextEdit::multiline(text)
                    .id(id)
                    .frame(false)
                    .desired_rows(1)
                    .min_size(egui::vec2(0., height))
                    .desired_width(f32::INFINITY)
                    .hint_text("Message the agent…")
                    .return_key(egui::KeyboardShortcut::new(
                        egui::Modifiers::SHIFT,
                        Key::Enter,
                    ))
                    .char_limit(crate::agent::PROMPT_LIMIT)
                    .show(ui);
                let recalled = ui
                    .ctx()
                    .data_mut(|data| data.remove_temp::<bool>(id.with("recall-reveal")))
                    .unwrap_or(false);
                if (recalled || old != Some(viewport.size()))
                    && output.response.has_focus()
                    && let Some(range) = output.cursor_range
                {
                    let cursor = output
                        .galley
                        .pos_from_cursor(range.primary)
                        .translate(output.galley_pos.to_vec2());
                    ui.scroll_to_rect(
                        cursor.expand(2.),
                        Some(if recalled { Align::TOP } else { Align::BOTTOM }),
                    );
                }
                ui.ctx()
                    .data_mut(|d| d.insert_temp(size_id, viewport.size()));
                output
            })
    });
    if area.inner.inner.response.has_focus() {
        ui.painter().rect_stroke(
            area.response.rect,
            visual.corner_radius,
            Stroke::new(1.0_f32, ui.visuals().selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    }
    area.inner
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RecallView {
        ctx: egui::Context,
        state: State,
        draft: String,
        messages: Vec<String>,
    }

    impl RecallView {
        fn new(draft: &str, messages: &[&str]) -> Self {
            let mut view = Self {
                ctx: egui::Context::default(),
                state: State::default(),
                draft: draft.into(),
                messages: messages.iter().map(|text| (*text).into()).collect(),
            };
            view.state.restore_history(view.messages.iter().cloned());
            view.frame(vec![], true, true);
            view.cursor(0, 0);
            view
        }
        fn cursor(&self, start: usize, end: usize) {
            let id = egui::Id::new("agent-composer");
            let mut state = egui::TextEdit::load_state(&self.ctx, id).unwrap();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(start),
                    egui::text::CCursor::new(end),
                )));
            state.store(&self.ctx, id);
        }
        fn frame(
            &mut self,
            events: Vec<egui::Event>,
            enabled: bool,
            focused: bool,
        ) -> egui::text_edit::TextEditOutput {
            let mut output = None;
            let _ = self.ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let id = egui::Id::new("agent-composer");
                        ui.memory_mut(|memory| {
                            memory.request_focus(if focused { id } else { id.with("other") })
                        });
                        self.state.recall(ui, &mut self.draft, id, enabled);
                        output = Some(editor(ui, &mut self.draft, id, 0.).inner);
                    });
                },
            );
            output.unwrap()
        }
        fn arrow(&mut self, key: Key) {
            self.frame(vec![history_key(key, egui::Modifiers::NONE)], true, true);
        }
    }

    fn history_key(key: Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn recall_browses_messages_preserves_edits_and_restores_the_unsent_draft() {
        let mut view = RecallView::new(
            "unfinished λ\nsecond line",
            &["oldest\nmultiline", "newest 🦀"],
        );
        view.arrow(Key::ArrowUp);
        assert_eq!(view.draft, "newest 🦀");
        view.arrow(Key::ArrowUp);
        assert_eq!(view.draft, "oldest\nmultiline");
        view.arrow(Key::ArrowUp);
        assert_eq!(view.draft, "oldest\nmultiline");
        view.frame(vec![egui::Event::Text("edited ".into())], true, true);
        view.cursor(0, 0);
        view.arrow(Key::ArrowDown);
        assert_eq!(view.draft, "newest 🦀");
        view.arrow(Key::ArrowUp);
        assert_eq!(view.draft, "edited oldest\nmultiline");
        view.arrow(Key::ArrowDown);
        view.arrow(Key::ArrowDown);
        assert_eq!(view.draft, "unfinished λ\nsecond line");
        assert!(view.state.recall.is_none());
        assert_eq!(view.messages, ["oldest\nmultiline", "newest 🦀"]);
        view.state.reset_recall();
        view.messages = vec!["another conversation".into()];
        view.state.restore_history(view.messages.iter().cloned());
        view.arrow(Key::ArrowUp);
        assert_eq!(view.draft, "another conversation");
        view.arrow(Key::ArrowDown);
        assert_eq!(view.draft, "unfinished λ\nsecond line");
    }

    #[test]
    fn recall_reaches_every_prompt_beyond_64_and_keeps_duplicate_submissions() {
        let mut view = RecallView::new("unfinished draft", &[]);
        let messages: Vec<_> = (0..100)
            .map(|index| format!("prompt {index}\nsecond line 🦀"))
            .collect();
        for text in &messages {
            view.state.remember_prompt(text.clone());
        }
        view.state.remember_prompt(messages.last().unwrap().clone());
        view.arrow(Key::ArrowUp);
        assert_eq!(view.draft, messages[99]);
        for text in messages.iter().rev() {
            view.arrow(Key::ArrowUp);
            assert_eq!(&view.draft, text);
            // Normal repaint frames must not reset or advance history.
            view.frame(vec![], true, true);
        }
        view.arrow(Key::ArrowUp);
        assert_eq!(view.draft, messages[0]);
        for text in messages.iter().skip(1) {
            view.arrow(Key::ArrowDown);
            assert_eq!(&view.draft, text);
        }
        view.arrow(Key::ArrowDown);
        assert_eq!(view.draft, messages[99]);
        view.arrow(Key::ArrowDown);
        assert_eq!(view.draft, "unfinished draft");
        view.state.clear_history();
        view.arrow(Key::ArrowUp);
        assert_eq!(view.draft, "unfinished draft");
    }

    #[test]
    fn recall_respects_cursor_selection_focus_modifiers_modal_and_ime_guards() {
        for (start, end, enabled, focused, events) in [
            (
                1,
                1,
                true,
                true,
                vec![history_key(Key::ArrowUp, egui::Modifiers::NONE)],
            ),
            (
                0,
                2,
                true,
                true,
                vec![history_key(Key::ArrowUp, egui::Modifiers::NONE)],
            ),
            (
                0,
                0,
                false,
                true,
                vec![history_key(Key::ArrowUp, egui::Modifiers::NONE)],
            ),
            (
                0,
                0,
                true,
                false,
                vec![history_key(Key::ArrowUp, egui::Modifiers::NONE)],
            ),
            (
                0,
                0,
                true,
                true,
                vec![history_key(Key::ArrowUp, egui::Modifiers::SHIFT)],
            ),
            (
                0,
                0,
                true,
                true,
                vec![history_key(Key::ArrowUp, egui::Modifiers::CTRL)],
            ),
            (
                0,
                0,
                true,
                true,
                vec![
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    history_key(Key::ArrowUp, egui::Modifiers::NONE),
                ],
            ),
            (
                0,
                0,
                true,
                true,
                vec![
                    egui::Event::Text("typed ".into()),
                    history_key(Key::ArrowUp, egui::Modifiers::NONE),
                ],
            ),
        ] {
            let mut view = RecallView::new("draft\nnext line", &["history"]);
            view.cursor(start, end);
            view.frame(events, enabled, focused);
            assert!(view.state.recall.is_none());
            assert!(!view.draft.contains("history"));
        }
        let mut view = RecallView::new("draft\nnext line", &["history"]);
        let output = view.frame(
            vec![history_key(Key::ArrowDown, egui::Modifiers::NONE)],
            true,
            true,
        );
        assert_eq!(view.draft, "draft\nnext line");
        assert!(output.cursor_range.unwrap().primary.index > 0);
        let mut view = RecallView::new("draft", &[]);
        view.arrow(Key::ArrowUp);
        assert_eq!(view.draft, "draft");
        assert!(view.state.recall.is_none());
    }

    struct View {
        viewport: Rect,
        caret: Rect,
        offset: f32,
        send: Rect,
        text_height: f32,
    }

    fn frame(
        ctx: &egui::Context,
        draft: &mut String,
        stretch: f32,
        size: Vec2,
        events: Vec<egui::Event>,
    ) -> View {
        let mut view = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let id = egui::Id::new("agent-composer");
                    ui.memory_mut(|m| m.request_focus(id));
                    let output = editor(ui, draft, id, stretch);
                    let cursor = output.inner.cursor_range.unwrap().primary;
                    view = Some(View {
                        viewport: output.inner_rect,
                        caret: output
                            .inner
                            .galley
                            .pos_from_cursor(cursor)
                            .translate(output.inner.galley_pos.to_vec2()),
                        offset: output.state.offset.y,
                        text_height: output.inner.response.rect.height(),
                        send: ui.button("Send").rect,
                    });
                });
            },
        );
        view.unwrap()
    }

    #[test]
    fn long_and_wrapped_drafts_scroll_with_the_caret_without_displacing_send() {
        for prompt in [
            (0..70)
                .map(|line| format!("Line {line}: λ 🦀\n"))
                .collect::<String>(),
            "A long wrapped message λ 🦀 ".repeat(150),
        ] {
            let ctx = egui::Context::default();
            let mut draft = String::new();
            let size = egui::vec2(280., 300.);
            let empty = frame(&ctx, &mut draft, 0., size, vec![]);
            frame(
                &ctx,
                &mut draft,
                0.,
                size,
                vec![egui::Event::Paste(prompt.clone())],
            );
            let mut view = frame(&ctx, &mut draft, 0., size, vec![]);
            assert_eq!(draft, prompt);
            assert!(view.offset > 100.);
            assert!(view.text_height > view.viewport.height());
            assert!(
                view.viewport.expand(2.).contains(view.caret.center()),
                "{:?} {:?}",
                view.viewport,
                view.caret
            );
            assert!((view.send.top() - empty.send.top()).abs() < 1.);
            assert!(view.send.bottom() < size.y);
            let at_end = view.offset;
            frame(
                &ctx,
                &mut draft,
                0.,
                size,
                vec![
                    egui::Event::PointerMoved(view.viewport.center()),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0., 160.),
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            view = frame(&ctx, &mut draft, 0., size, vec![]);
            assert!(
                view.offset < at_end - 20.,
                "Manual scrolling should stay independent of the caret"
            );
            // egui smooths wheel input over several frames. It may keep moving
            // upward, but must never snap back to the focused caret at the end.
            for _ in 0..12 {
                let next = frame(&ctx, &mut draft, 0., size, vec![]);
                assert!(next.offset <= view.offset + 1.);
                view = next;
            }
            frame(
                &ctx,
                &mut draft,
                0.,
                size,
                vec![egui::Event::Text("tail".into())],
            );
            view = frame(&ctx, &mut draft, 0., size, vec![]);
            assert!(draft.ends_with("tail"));
            assert!(view.viewport.expand(2.).contains(view.caret.center()));
            // Resizing can hide the current caret without changing its character
            // index. Keep it visible on both a larger viewport and a narrow one.
            frame(&ctx, &mut draft, 100., size, vec![]);
            view = frame(&ctx, &mut draft, 100., size, vec![]);
            assert!(view.viewport.expand(2.).contains(view.caret.center()));
            let narrow = egui::vec2(180., 200.);
            frame(&ctx, &mut draft, -20., narrow, vec![]);
            view = frame(&ctx, &mut draft, -20., narrow, vec![]);
            assert!(view.viewport.expand(2.).contains(view.caret.center()));
            assert!(view.send.bottom() < narrow.y);
        }
    }

    #[test]
    fn dragging_the_divider_resizes_input_and_small_windows_preserve_the_preferred_height() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        let mut render = |height: f32, hidden: bool, events: Vec<egui::Event>| {
            let mut layout = None;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400., height),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let bounds = ui.available_rect_before_wrap();
                        layout = Some(state.layout(ui, bounds, false, hidden, true));
                    });
                },
            );
            layout.unwrap()
        };
        let original = render(500., false, vec![]);
        let start = original.input.center_top() - egui::vec2(0., 2.);
        let pointer = |position, pressed| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(
            500.,
            false,
            vec![egui::Event::PointerMoved(start), pointer(start, true)],
        );
        let moved = start - egui::vec2(0., 110.);
        let enlarged = render(500., false, vec![egui::Event::PointerMoved(moved)]);
        assert!(enlarged.input.height() > original.input.height() + 100.);
        assert!(enlarged.content.bottom() <= enlarged.input.top());
        render(500., false, vec![pointer(moved, false)]);
        assert!(ctx.memory(|m| m.has_focus(egui::Id::new("agent-composer"))));
        let small = render(260., false, vec![]);
        assert!(small.input.height() < enlarged.input.height());
        assert!(small.content.height() >= 0.);
        assert_eq!(
            render(500., false, vec![]).input.height(),
            enlarged.input.height()
        );
        assert_eq!(render(500., true, vec![]).input.height(), 0.);
        assert_eq!(
            render(500., false, vec![]).input.height(),
            enlarged.input.height()
        );
        let grip = render(500., false, vec![]).input.center_top() - egui::vec2(0., 2.);
        render(
            500.,
            false,
            vec![egui::Event::PointerMoved(grip), pointer(grip, true)],
        );
        render(500., false, vec![pointer(grip, false)]);
        render(500., false, vec![pointer(grip, true)]);
        assert_eq!(
            render(500., false, vec![pointer(grip, false)])
                .input
                .height(),
            DEFAULT_HEIGHT
        );
    }
}
