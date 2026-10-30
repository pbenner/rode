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
}

impl Default for State {
    fn default() -> Self {
        Self {
            height: DEFAULT_HEIGHT,
            drag_start: None,
        }
    }
}

pub(super) struct Layout {
    pub content: Rect,
    pub input: Rect,
    pub stretch: f32,
}

impl State {
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
                if old != Some(viewport.size())
                    && output.response.has_focus()
                    && let Some(range) = output.cursor_range
                {
                    let cursor = output
                        .galley
                        .pos_from_cursor(range.primary)
                        .translate(output.galley_pos.to_vec2());
                    ui.scroll_to_rect(cursor.expand(2.), Some(Align::BOTTOM));
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
