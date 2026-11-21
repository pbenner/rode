//! Window-local sizing for bottom-anchored chat panels. Resizing changes only
//! presentation; viewport clamps never overwrite the user's preferred height.
use crate::app::layout::resize::{Edge, Handle};
use eframe::egui::{self, Rect};

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
        let resized = Handle {
            rect: handle,
            id: ui.id().with(id),
            label,
            edge: Edge::Top,
            size: height,
            default: size.default + size.extra,
            range: minimum..=maximum,
            enabled,
        }
        .show(ui, &mut self.drag_start);
        crate::app::ui_probe::button(ui, id, &resized.response);
        if resized.response.is_pointer_button_down_on() || resized.response.dragged() {
            crate::app::ui_probe::button(ui, &format!("{id}-active"), &resized.response);
        }
        if resized.reset {
            self.preferred = None;
        } else if resized.response.changed() {
            self.preferred = Some(resized.size - size.extra);
        }
        let panel = Rect::from_min_max(
            egui::pos2(bounds.left(), bounds.bottom() - resized.size),
            bounds.max,
        );
        Layout {
            content: Rect::from_min_max(
                bounds.min,
                egui::pos2(bounds.right(), (panel.top() - 6.).max(bounds.top())),
            ),
            panel,
            handle: resized.response,
        }
    }
}
