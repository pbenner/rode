//! Drag intent is a listing snapshot, never filesystem authority. Drops revalidate
//! the source generation/token and use the same bounded queue as keyboard actions.
use super::*;

#[derive(Clone)]
pub(super) struct Files {
    pub side: usize,
    pub generation: u64,
    pub token: u64,
    pub directory: String,
    pub paths: Vec<String>,
}
impl Files {
    pub(super) fn valid(&self, state: &State, target: usize) -> bool {
        if self.side >= 2 || target >= 2 {
            return false;
        }
        let paths: HashSet<_> = state.panes[self.side]
            .browser
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        self.side != target
            && self.generation == state.generation
            && state.panes[self.side].ready()
            && state.panes[target].ready()
            && self.token == state.panes[self.side].browser.token
            && self.directory == state.panes[self.side].browser.directory
            && !self.paths.is_empty()
            && self
                .paths
                .iter()
                .all(|path| path == &self.directory || paths.contains(path.as_str()))
    }
}

/// Desktop drops contain real local paths; reject unsupported virtual/invalid
/// paths before admission. Reading metadata and walking folders stay off the GUI thread.
pub(super) fn desktop_paths(files: &[egui::DroppedFile]) -> Result<Vec<String>, String> {
    if files.is_empty() || files.len() > 10_000 {
        return Err("Drop at most 10,000 local files or folders.".into());
    }
    let mut bytes = 0;
    files
        .iter()
        .map(|file| {
            let path = file
                .path
                .as_ref()
                .filter(|path| path.is_absolute())
                .and_then(|path| path.to_str())
                .ok_or("Drop files or folders from this computer's file manager.")?;
            bytes += path.len();
            if bytes > 16 * 1024 * 1024 {
                return Err("The dropped selection is too large.".into());
            }
            Ok(path.to_owned())
        })
        .collect()
}

impl EditorApp {
    fn transfer_drop_allowed(&self) -> bool {
        self.panel_enabled(Panel::Transfers)
            && self.dialog == Dialog::None
            && !self.pending.modal()
            && !self.workbench_modal()
            && !self.exit_pending()
            && !self
                .transfers
                .running
                .as_ref()
                .is_some_and(|running| running.progress.lock().unwrap().conflict.is_some())
    }

    pub(super) fn transfer_drop_ui(
        &mut self,
        ui: &mut egui::Ui,
        side: usize,
        rect: egui::Rect,
        folder: Option<String>,
        ctx: &egui::Context,
    ) {
        if !rect.is_positive() || !self.transfer_drop_allowed() {
            return;
        }
        // Observation only: a full-pane interaction placed above the rows would
        // steal their hover acknowledgement and break clicking/dragging.
        let mut zone = ui.response();
        zone.rect = rect;
        zone.interact_rect = rect;
        ui_probe::button(ui, &format!("transfer-drop:{side}"), &zone);
        if !ui.rect_contains_pointer(rect) {
            return;
        }
        let payload = egui::DragAndDrop::payload::<Files>(ctx);
        let external = ui.input(|input| {
            !input.raw.hovered_files.is_empty() || !input.raw.dropped_files.is_empty()
        });
        let valid = payload
            .as_ref()
            .is_some_and(|files| files.valid(&self.transfers, side))
            || external && side == 1 && self.transfers.panes[side].ready();
        if payload.is_none() && !external {
            return;
        }
        ctx.set_cursor_icon(if valid {
            egui::CursorIcon::Copy
        } else {
            egui::CursorIcon::NotAllowed
        });
        let destination =
            folder.unwrap_or_else(|| self.transfers.panes[side].browser.directory.clone());
        if valid {
            let palette = self.themes.active().palette;
            let painter = ui.painter().with_clip_rect(rect);
            painter.rect_filled(rect.shrink(1.), 7, palette.accent.gamma_multiply(0.07));
            painter.rect_stroke(
                rect.shrink(1.),
                7,
                Stroke::new(2_f32, palette.accent),
                egui::StrokeKind::Inside,
            );
            let banner = egui::Rect::from_min_max(
                egui::pos2(rect.left() + 3., (rect.bottom() - 34.).max(rect.top())),
                rect.max - egui::vec2(3., 3.),
            );
            painter.rect_filled(banner, 5, palette.raised);
            let name = ssh::display_name(&destination);
            painter.with_clip_rect(banner.shrink(4.)).text(
                banner.center(),
                egui::Align2::CENTER_CENTER,
                format!("Copy to {name}"),
                FontId::proportional(13.),
                palette.text,
            );
            ui_probe::button(ui, &format!("transfer-drop-ready:{side}"), &zone);
        }
        if valid && payload.is_some() && ctx.input(|i| i.pointer.any_released()) {
            if let Some(files) = egui::DragAndDrop::take_payload::<Files>(ctx) {
                self.queue_transfer_to(files.side, files.paths.clone(), destination, ctx);
            }
        } else if external && side == 1 && ctx.input(|i| !i.raw.dropped_files.is_empty()) {
            let files = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
            if valid {
                match desktop_paths(&files) {
                    Ok(paths) => self.queue_transfer_to(0, paths, destination, ctx),
                    Err(error) => self.transfers.error = Some(error),
                }
            } else {
                self.transfers.error =
                    Some("Choose a ready remote destination before dropping files.".into());
            }
        }
    }

    pub(super) fn transfer_drag_preview(&self, ctx: &egui::Context) {
        let Some(files) = egui::DragAndDrop::payload::<Files>(ctx) else {
            return;
        };
        let Some(pointer) = ctx.pointer_interact_pos() else {
            return;
        };
        egui::Area::new(egui::Id::new("transfer-drag-preview"))
            .order(egui::Order::Tooltip)
            .interactable(false)
            .fixed_pos(pointer + egui::vec2(15., 18.))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(260.);
                    let text = if files.paths.len() == 1 {
                        ssh::display_name(&files.paths[0]).to_owned()
                    } else {
                        format!("{} items", files.paths.len())
                    };
                    let label = ui.add(egui::Label::new(RichText::new(text).strong()).truncate());
                    ui_probe::button(ui, "transfer-dragging", &label);
                    ui.weak(if files.side == 0 {
                        "Copy to remote"
                    } else {
                        "Copy to this computer"
                    });
                });
            });
    }
}
