//! Shared, focusable resize grips. Callers own panel geometry and preferences;
//! this interaction never changes project, queue or transport state.
use eframe::egui::{self, Key, Rect, Response, Sense, Stroke};
use std::ops::RangeInclusive;

/// The movable edge determines both the drag axis and the keyboard direction.
pub(in crate::app) enum Edge {
    Right,
    Top,
    Bottom,
}

pub(in crate::app) struct Handle<'a> {
    pub rect: Rect,
    pub id: egui::Id,
    pub label: &'a str,
    pub edge: Edge,
    pub size: f32,
    pub default: f32,
    pub range: RangeInclusive<f32>,
    pub enabled: bool,
}

pub(in crate::app) struct Resized {
    pub response: Response,
    pub size: f32,
    /// Reset is distinct from resizing to the current default: automatic panel
    /// sizes must continue following their content after Home/double-click.
    pub reset: bool,
}

impl Handle<'_> {
    /// Keep the initial drag size outside frame geometry so moving the grip
    /// under the pointer cannot compound its displacement on subsequent frames.
    pub fn show(self, ui: &mut egui::Ui, drag_start: &mut Option<f32>) -> Resized {
        let horizontal = matches!(self.edge, Edge::Right);
        let mut response = ui
            .interact(
                self.rect,
                self.id,
                if self.enabled { Sense::click_and_drag() } else { Sense::hover() },
            )
            .on_hover_cursor(if horizontal {
                egui::CursorIcon::ResizeHorizontal
            } else {
                egui::CursorIcon::ResizeVertical
            })
            .on_hover_text(if horizontal {
                "Drag to resize · Double-click to reset\nWhen focused: Left/Right to resize, Home to reset"
            } else {
                "Drag to resize · Double-click to reset\nWhen focused: Up/Down to resize, Home to reset"
            });
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        crate::keyboard::target(ui, self.label, &response);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::DragValue, self.enabled, self.label)
        });
        if response.has_focus() {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        horizontal_arrows: horizontal,
                        vertical_arrows: !horizontal,
                        ..Default::default()
                    },
                );
            });
        }
        let clamp = |value: f32| value.clamp(*self.range.start(), *self.range.end());
        let mut size = clamp(self.size);
        let mut reset = false;
        if self.enabled {
            if response.drag_started() {
                *drag_start = Some(size);
            }
            if response.dragged() {
                let delta = response.total_drag_delta().unwrap_or_default();
                let delta = match self.edge {
                    Edge::Right => delta.x,
                    Edge::Top => -delta.y,
                    Edge::Bottom => delta.y,
                };
                size = clamp(drag_start.unwrap_or(size) + delta);
            }
            reset = response.double_clicked()
                || response.has_focus()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Home));
            if reset {
                size = clamp(self.default);
                *drag_start = None;
            } else if response.has_focus() {
                let keys = match self.edge {
                    Edge::Right => [(Key::ArrowLeft, -20.), (Key::ArrowRight, 20.)],
                    Edge::Top => [(Key::ArrowUp, 20.), (Key::ArrowDown, -20.)],
                    Edge::Bottom => [(Key::ArrowUp, -20.), (Key::ArrowDown, 20.)],
                };
                for (key, step) in keys {
                    let presses =
                        ui.input_mut(|i| i.count_and_consume_key(egui::Modifiers::NONE, key));
                    if presses > 0 {
                        // egui queues spatial navigation before widgets run.
                        // A newly focused grip cannot install its lock until
                        // this frame, so also cancel the consumed arrow's move.
                        ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
                    }
                    size = clamp(size + step * presses as f32);
                }
            }
            if response.drag_stopped() {
                *drag_start = None;
            }
        } else {
            *drag_start = None;
        }
        if size != self.size || reset {
            response.mark_changed();
            ui.ctx().request_repaint();
        }
        let active = response.hovered() || response.dragged() || response.has_focus();
        let color = if active {
            ui.visuals().selection.stroke.color
        } else {
            ui.visuals().weak_text_color()
        };
        let center = self.rect.center();
        let extent = if horizontal {
            egui::vec2(0., 18_f32.min(self.rect.height() * 0.5))
        } else {
            egui::vec2(18_f32.min(self.rect.width() * 0.5), 0.)
        };
        let painter = ui
            .painter()
            .with_clip_rect(self.rect.intersect(ui.clip_rect()));
        if active {
            painter.rect_filled(self.rect.shrink(1.), 2., color.gamma_multiply(0.12));
        }
        painter.line_segment(
            [center - extent, center + extent],
            Stroke::new(2_f32, color),
        );
        Resized {
            response,
            size,
            reset,
        }
    }
}
