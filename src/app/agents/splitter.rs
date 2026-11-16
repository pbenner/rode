//! Window-local sizing for bottom-anchored chat panels. Resizing changes only
//! presentation; viewport clamps never overwrite the user's preferred height.
use eframe::egui::{self, Key, Rect, Sense, Stroke};

#[derive(Default)]
pub(super) struct State {
    preferred: Option<f32>,
    drag_start: Option<f32>,
}

/// Current panel budget, including temporary warning/attachment rows.
pub(super) struct Size {
    pub default: f32,
    pub minimum: f32,
    pub maximum: f32,
    pub extra: f32,
}

pub(super) struct Layout {
    pub content: Rect,
    pub panel: Rect,
    pub handle: egui::Response,
}

impl State {
    pub fn cancel_drag(&mut self) {
        self.drag_start = None;
    }

    /// Drag upward or use Up/Down to expand. Double-click/Home restores the
    /// caller's automatic default; an empty or hidden panel retains its preference.
    pub fn bottom(
        &mut self,
        ui: &mut egui::Ui,
        bounds: Rect,
        size: Size,
        id: &str,
        label: &str,
        enabled: bool,
    ) -> Layout {
        let maximum = size.maximum.min(bounds.height()).max(0.);
        let minimum = size.minimum.min(maximum).max(0.);
        let height = (self.preferred.unwrap_or(size.default) + size.extra).clamp(minimum, maximum);
        let top = bounds.bottom() - height;
        let handle = Rect::from_min_max(
            egui::pos2(bounds.left(), (top - 6.).max(bounds.top())),
            egui::pos2(bounds.right(), top + 2.),
        );
        let response = ui.interact(
            handle,
            ui.id().with(id),
            if enabled { Sense::click_and_drag() } else { Sense::hover() },
        ).on_hover_cursor(egui::CursorIcon::ResizeVertical)
            .on_hover_text("Drag to resize · Double-click to reset\nWhen focused: Up/Down to resize, Home to reset");
        crate::app::ui_probe::button(ui, id, &response);
        crate::keyboard::target(ui, label, &response);
        if response.is_pointer_button_down_on() || response.dragged() {
            crate::app::ui_probe::button(ui, &format!("{id}-active"), &response);
        }
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, enabled, label));
        if response.has_focus() {
            // These arrows resize the panel instead of invoking spatial focus
            // navigation to a neighbouring chat control.
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        vertical_arrows: true,
                        ..Default::default()
                    },
                );
            });
        }
        let mut resized = height;
        if enabled {
            if response.drag_started() {
                self.drag_start = Some(height);
            }
            if response.dragged() {
                resized = (self.drag_start.unwrap_or(height)
                    - response.total_drag_delta().unwrap_or_default().y)
                    .clamp(minimum, maximum);
                self.preferred = Some(resized - size.extra);
            }
            if response.double_clicked()
                || response.has_focus()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Home))
            {
                self.preferred = None;
                resized = (size.default + size.extra).clamp(minimum, maximum);
            } else if response.has_focus() {
                for (key, step) in [(Key::ArrowUp, 20.), (Key::ArrowDown, -20.)] {
                    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                        resized = (resized + step).clamp(minimum, maximum);
                        self.preferred = Some(resized - size.extra);
                    }
                }
            }
            if response.drag_stopped() {
                self.cancel_drag();
            }
        } else {
            self.cancel_drag();
        }
        let panel = Rect::from_min_max(
            egui::pos2(bounds.left(), bounds.bottom() - resized),
            bounds.max,
        );
        let center = panel.center_top() - egui::vec2(0., 2.);
        let color = if response.hovered() || response.dragged() || response.has_focus() {
            ui.visuals().selection.stroke.color
        } else {
            ui.visuals().weak_text_color()
        };
        ui.painter().line_segment(
            [center - egui::vec2(18., 0.), center + egui::vec2(18., 0.)],
            Stroke::new(2_f32, color),
        );
        Layout {
            content: Rect::from_min_max(
                bounds.min,
                egui::pos2(bounds.right(), (panel.top() - 6.).max(bounds.top())),
            ),
            panel,
            handle: response,
        }
    }
}
