//! Transfer workspace: two calm browser surfaces above a compact, resizable activity tray.
use super::*;

pub(super) fn surface(palette: theme::Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(1_f32, palette.border))
        .corner_radius(10)
        .inner_margin(10)
}

impl EditorApp {
    pub(in crate::app) fn transfers_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let view_width = ui.available_width().max(90.);
        let view_height = ui.available_height().max(80.);
        let compact = view_height < 430.;
        let queue_id = ui.id().with("transfer-queue");
        let local_id = ui.id().with("transfer-local");
        layout::header(
            ui,
            "File transfers",
            if compact {
                ""
            } else {
                "Drag files between panes to copy. Your project stays open."
            },
            palette,
            |ui| {
                crate::keyboard::menu_button(ui, "Layout…", |ui| {
                    for (id, label, default, maximum, horizontal) in [
                        (
                            local_id,
                            "Local browser width",
                            view_width * 0.5,
                            view_width * 0.7,
                            true,
                        ),
                        (
                            queue_id,
                            "Transfer queue height",
                            100.,
                            (view_height * 0.45).max(70.),
                            false,
                        ),
                    ] {
                        let mut state = egui::containers::panel::PanelState::load(ctx, id)
                            .unwrap_or(egui::containers::panel::PanelState {
                                rect: egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(default, default),
                                ),
                            });
                        let mut size = if horizontal {
                            state.rect.width()
                        } else {
                            state.rect.height()
                        };
                        ui.label(label);
                        if ui
                            .add(crate::keyboard::named(
                                label,
                                egui::DragValue::new(&mut size)
                                    .range(70. ..=maximum.max(70.))
                                    .speed(5.),
                            ))
                            .changed()
                        {
                            if horizontal {
                                state.rect.max.x = state.rect.min.x + size;
                            } else {
                                state.rect.max.y = state.rect.min.y + size;
                            }
                            ctx.data_mut(|data| data.insert_persisted(id, state));
                        }
                    }
                });
            },
        );
        if let Some(error) = &self.transfers.error {
            ui.colored_label(palette.error, error);
        }
        for side in 0..2 {
            if self.transfers.panes[side].browser.location.is_empty() {
                self.transfers.panes[side].browser.location = if side == 0 {
                    files::expand_home("~").to_string_lossy().into()
                } else {
                    let Some(root) = self.transfer_host_root() else {
                        continue;
                    };
                    root
                };
                self.transfers.panes[side].browser.needs_load = true;
            }
            if self.transfers.panes[side].browser.needs_load
                && !self.transfers.panes[side].browser.loading
            {
                self.transfers.panes[side].browser.needs_load = false;
                self.transfer_list(
                    side,
                    self.transfers.panes[side].browser.location.clone(),
                    None,
                    ctx,
                );
            }
        }
        if compact {
            egui::TopBottomPanel::bottom(queue_id.with("compact"))
                .frame(surface(palette))
                .exact_height(layout::control_height(ui) + 22.)
                .show_inside(ui, |ui| {
                    ui.horizontal(|ui| {
                        let menu = crate::keyboard::menu_button(ui, "Activity…", |ui| {
                            ui.set_width(350_f32.min(ctx.content_rect().width() - 50.));
                            ui.set_max_height(260.);
                            self.transfer_queue_ui(ui, ctx);
                        });
                        if self.transfers.running.is_none() && self.transfers.queue.is_empty() {
                            ui_probe::button(ui, "transfer-idle", &menu.response);
                        }
                        ui.weak(if self.transfers.running.is_some() {
                            "Transferring…".into()
                        } else {
                            format!("{} finished", self.transfers.history.len())
                        });
                    });
                });
        } else {
            let maximum = (ui.available_height() * 0.45).max(65.);
            egui::TopBottomPanel::bottom(queue_id)
                .frame(surface(palette))
                .resizable(true)
                .default_height(100.)
                .height_range(65. ..=maximum)
                .show_inside(ui, |ui| self.transfer_queue_ui(ui, ctx));
        }
        ui.add_space(8.);
        let width = ui.available_width();
        // Persist the user's split proportion across native window resizes.
        // Keeping an absolute desktop width would crush the remote pane on a laptop.
        if let Some(previous) =
            ctx.data(|data| data.get_temp::<f32>(local_id.with("container-width")))
            && (width - previous).abs() > 1.
            && let Some(mut state) = egui::containers::panel::PanelState::load(ctx, local_id)
        {
            state.rect.max.x = state.rect.min.x + state.rect.width() * width / previous.max(1.);
            ctx.data_mut(|data| data.insert_persisted(local_id, state));
        }
        ctx.data_mut(|data| data.insert_temp(local_id.with("container-width"), width));
        let minimum = (width * 0.35).clamp(100., 230.);
        egui::SidePanel::left(local_id)
            .frame(surface(palette).outer_margin(egui::Margin {
                right: 8,
                ..Default::default()
            }))
            .resizable(true)
            .default_width((width - 8.) * 0.5)
            .width_range(minimum..=(width - minimum - 8.).max(minimum))
            .show_inside(ui, |ui| self.transfer_pane_ui(ui, 0, ctx));
        egui::CentralPanel::default()
            .frame(surface(palette))
            .show_inside(ui, |ui| self.transfer_pane_ui(ui, 1, ctx));
        self.transfer_drag_preview(ctx);
    }
    /// Conflicts remain operable at compact sizes, independent of queue height
    /// and which workspace tool is currently visible. Other app modals take priority.
    pub(in crate::app) fn transfer_windows(&mut self, ctx: &egui::Context) {
        if self.dialog != Dialog::None || self.pending.modal() || self.workbench_modal() {
            return;
        }
        let Some(running) = self.transfers.running.as_ref().filter(|running| {
            running.generation == self.transfers.generation
                && !running.cancel.load(Ordering::Acquire)
        }) else {
            return;
        };
        let Some(conflict) = running.progress.lock().unwrap().conflict.clone() else {
            return;
        };
        let cancel = running.cancel.clone();
        let mut decision = None;
        let modal = egui::Modal::new(egui::Id::new("transfer-conflict")).show(ctx, |ui| {
            ui.set_max_width((ctx.content_rect().width() - 64.).clamp(140., 540.));
            ui.heading("File already exists");
            ui.label(&conflict.path);
            crate::keyboard::checkbox(
                ui,
                &mut self.transfers.remaining,
                "Apply to remaining conflicts",
            );
            let rename = ui.add(crate::keyboard::named(
                "New name",
                egui::TextEdit::singleline(&mut self.transfers.rename)
                    .desired_width(f32::INFINITY)
                    .hint_text("New name (blank chooses a free name)"),
            ));
            ui_probe::button(ui, "transfer-rename-name", &rename);
            ui_probe::value(
                ui,
                "transfer-rename-name-value",
                &rename,
                &self.transfers.rename,
            );
            ui.horizontal_wrapped(|ui| {
                for (label, action, enabled) in [
                    ("Skip", ConflictAction::Skip, true),
                    ("Replace", ConflictAction::Replace, conflict.regular),
                    (
                        "Rename",
                        ConflictAction::Rename(self.transfers.rename.clone()),
                        self.transfers.rename.is_empty()
                            || explorer::valid_name(&self.transfers.rename),
                    ),
                ] {
                    let button = ui.add_enabled(enabled, crate::keyboard::Button::new(label));
                    ui_probe::button(ui, &format!("transfer-conflict-{label}"), &button);
                    if button.clicked() {
                        decision = Some(Decision {
                            action,
                            remaining: self.transfers.remaining,
                        });
                    }
                }
                let button = crate::keyboard::button(ui, "Cancel transfer");
                ui_probe::button(ui, "transfer-conflict-cancel", &button);
                if button.clicked() {
                    cancel.store(true, Ordering::Release);
                }
            });
        });
        if modal.should_close() {
            cancel.store(true, Ordering::Release);
        }
        if let Some(decision) = decision {
            let _ = conflict.answer.try_send(decision);
            self.transfers.rename.clear();
        }
    }
}
