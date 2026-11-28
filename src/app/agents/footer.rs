//! Measure the composer toolbar before sizing its editor. Keep settings beside
//! Persist on a shared row, wrapping whole groups when the native pane is narrow.
use super::*;

const MAX_CONTROLS_WIDTH: f32 = 380.;

#[derive(Clone, Copy)]
pub(super) enum Group {
    Actions,
    Controls,
    Persistence,
}

pub(super) struct Layout {
    groups: [(Group, egui::Rect); 3],
    height: f32,
    row_height: f32,
    pub extra: f32,
}

impl Layout {
    pub fn new(ui: &egui::Ui, width: f32, chat: &Chat, queued: bool) -> Self {
        let gap = ui.spacing().item_spacing;
        let row = layout::control_height(ui)
            .max(ui.spacing().icon_width + 2. * ui.spacing().button_padding.y)
            .ceil();
        let active = if chat.busy {
            Some(if chat.stopping() {
                "Close process"
            } else {
                "Stop"
            })
        } else if chat.connected() {
            Some(if chat.persistent() {
                "Detach"
            } else {
                "Disconnect"
            })
        } else {
            None
        };
        let mut actions = 0.;
        let mut used = 0.;
        let mut rows = 1;
        for label in [
            Some(if queued { "Queue" } else { "Send" }),
            active,
            Some("Attach…"),
        ]
        .into_iter()
        .flatten()
        {
            let button = layout::button_width(ui, label);
            if used > 0. && used + gap.x + button > width {
                rows += 1;
                used = 0.;
            }
            if used > 0. {
                used += gap.x;
            }
            used += button;
            if actions > 0. {
                actions += gap.x;
            }
            actions += button;
        }
        let persistence = (layout::button_width(ui, "Persist: Auto")
            + ui.spacing().icon_width
            + ui.spacing().icon_spacing)
            .max(100.);
        let minimum_controls = 2. * controls::MIN_BUTTON_WIDTH + gap.x;
        let settings = minimum_controls + gap.x + persistence;
        let rect = |x, y, w, h| egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h));
        let (actions_rect, controls_rect, persistence_rect, height) =
            if width >= actions + gap.x + settings {
                let controls = (width - actions - persistence - 2. * gap.x).min(MAX_CONTROLS_WIDTH);
                let x = width - persistence - gap.x - controls;
                (
                    rect(0., 0., x - gap.x, row),
                    rect(x, 0., controls, row),
                    rect(width - persistence, 0., persistence, row),
                    row,
                )
            } else {
                let actions_height = rows as f32 * row + (rows - 1) as f32 * gap.y;
                let y = actions_height + gap.y;
                if width >= settings {
                    let controls = (width - persistence - gap.x).min(MAX_CONTROLS_WIDTH);
                    (
                        rect(0., 0., width, actions_height),
                        rect(width - persistence - gap.x - controls, y, controls, row),
                        rect(width - persistence, y, persistence, row),
                        y + row,
                    )
                } else {
                    (
                        rect(0., 0., width, actions_height),
                        rect(0., y, width, row),
                        rect(
                            (width - persistence).max(0.),
                            y + row + gap.y,
                            persistence.min(width),
                            row,
                        ),
                        y + 2. * row + gap.y,
                    )
                }
            };
        Self {
            groups: [
                (Group::Actions, actions_rect),
                (Group::Controls, controls_rect),
                (Group::Persistence, persistence_rect),
            ],
            height,
            row_height: row,
            extra: height - layout::control_height(ui),
        }
    }

    pub fn show(&self, ui: &mut egui::Ui, mut add: impl FnMut(&mut egui::Ui, Group)) {
        let (bounds, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), self.height),
            Sense::hover(),
        );
        for (group, rect) in &self.groups {
            let mut area = rect.translate(bounds.min.to_vec2());
            if matches!(group, Group::Actions) {
                // Cross-axis centering belongs to one row, not the full height
                // reserved for wrapped actions. Subsequent rows grow downward.
                area.max.y = area.min.y + self.row_height;
            }
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(area).layout(
                    egui::Layout::left_to_right(Align::Center)
                        .with_main_wrap(matches!(group, Group::Actions)),
                ),
                |ui| add(ui, *group),
            );
        }
    }
}
