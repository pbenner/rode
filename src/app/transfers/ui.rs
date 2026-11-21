//! Transfer workspace: two calm browser surfaces above a compact, resizable activity tray.
use super::*;
use layout::resize::{Edge, Handle};

const DIVIDER: f32 = 8.;

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
        let view_height = ui.available_height().max(80.);
        let compact = view_height < 430.;
        let queue_id = ui.id().with("transfer-queue");
        let local_id = ui.id().with("transfer-local");
        layout::header(
            ui,
            "File transfers",
            if compact {
                ""
            } else if self.transfers.sharing.open {
                "Mount selected folders over SSH. Save their settings with a project bookmark."
            } else {
                "Drag files between panes to copy. Your project stays open."
            },
            palette,
            |ui| {
                let button = ui.add(crate::keyboard::Button::selectable(
                    self.transfers.sharing.open,
                    "Shared folders",
                ));
                ui_probe::button(ui, "shared-folders-toggle", &button);
                if button.clicked() {
                    self.transfers.sharing.open = true;
                }
                let files = ui.add(crate::keyboard::Button::selectable(
                    !self.transfers.sharing.open,
                    "Files",
                ));
                if files.clicked() {
                    self.transfers.sharing.open = false;
                }
            },
        );
        if self.transfers.sharing.open {
            egui::ScrollArea::vertical()
                .id_salt("shared-folder-page")
                .show(ui, |ui| self.shared_folders_ui(ui, ctx));
            return;
        }
        if let Some(error) = &self.transfers.error {
            ui.colored_label(palette.error, error);
        }
        for side in Side::ALL {
            if self.transfers.panes[side].browser.location.is_empty() {
                self.transfers.panes[side].browser.location = if side == Side::Local {
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
                // A completion refreshes the listed directory; it must not
                // submit a different path while the user is still typing it.
                && (self.transfers.panes[side].browser.directory.is_empty()
                    || self.transfers.panes[side].browser.location
                        == self.transfers.panes[side].browser.directory)
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
        let mut queue_resize = None;
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
                        if self.transfers.running.is_none()
                            && self.transfers.queue.is_empty()
                            && self.transfers.file_operation.is_none()
                        {
                            ui_probe::button(ui, "transfer-idle", &menu.response);
                        }
                        ui.weak(if self.transfers.file_operation.is_some() {
                            "Updating files…".into()
                        } else if self.transfers.running.is_some() {
                            "Transferring…".into()
                        } else {
                            format!("{} finished", self.transfers.history.len())
                        });
                    });
                });
        } else {
            let maximum = (ui.available_height() * 0.45).max(65.);
            let queue = egui::TopBottomPanel::bottom(queue_id)
                .frame(surface(palette).outer_margin(egui::Margin {
                    top: DIVIDER as i8,
                    ..Default::default()
                }))
                .resizable(false)
                .default_height(100. + DIVIDER)
                .height_range(65. ..=maximum.max(65. + DIVIDER))
                .show_separator_line(false)
                .show_inside(ui, |ui| self.transfer_queue_ui(ui, ctx));
            queue_resize = Some((queue.response.rect, maximum.max(65. + DIVIDER)));
        }
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
        let local = egui::SidePanel::left(local_id)
            .frame(surface(palette).outer_margin(egui::Margin {
                right: DIVIDER as i8,
                ..Default::default()
            }))
            .resizable(false)
            .default_width((width + DIVIDER) * 0.5)
            // egui adds outer margins to the panel's minimum content size.
            .width_range((minimum - DIVIDER)..=(width - minimum - DIVIDER).max(minimum))
            .show_separator_line(false)
            .show_inside(ui, |ui| self.transfer_pane_ui(ui, Side::Local, ctx));
        egui::CentralPanel::default()
            .frame(surface(palette))
            .show_inside(ui, |ui| self.transfer_pane_ui(ui, Side::Remote, ctx));
        // These hit regions occupy reserved outer margins, never file rows,
        // pane borders or queue buttons. Render after both browsers so neither
        // ScrollArea can claim the divider's drag/focus interaction.
        let rect = local.response.rect;
        resize_panel(
            ui,
            local_id,
            "transfer-browser-resize",
            Handle {
                rect: egui::Rect::from_min_max(
                    egui::pos2(rect.right() - DIVIDER, rect.top()),
                    rect.max,
                ),
                id: local_id.with("handle"),
                label: "Local browser width",
                edge: Edge::Right,
                size: rect.width(),
                default: (width + DIVIDER) * 0.5,
                range: minimum..=(width - minimum - DIVIDER).max(minimum),
                enabled: true,
            },
        );
        if let Some((rect, maximum)) = queue_resize {
            resize_panel(
                ui,
                queue_id,
                "transfer-queue-resize",
                Handle {
                    rect: egui::Rect::from_min_max(
                        rect.min,
                        egui::pos2(rect.right(), rect.top() + DIVIDER),
                    ),
                    id: queue_id.with("handle"),
                    label: "Transfer queue height",
                    edge: Edge::Top,
                    size: rect.height(),
                    default: 100. + DIVIDER,
                    range: (65. + DIVIDER)..=maximum,
                    enabled: true,
                },
            );
        }
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

/// Keep egui's panel sizes (including existing user preferences) while replacing
/// its mouse-only edge with the shared keyboard-accessible grip. PanelState is
/// keyed by the containing UI, so detached windows keep independent geometry.
fn resize_panel(ui: &mut egui::Ui, panel: egui::Id, name: &str, handle: Handle<'_>) {
    let drag_id = panel.with("drag-start");
    let mut drag_start = ui
        .ctx()
        .data(|data| data.get_temp::<Option<f32>>(drag_id))
        .flatten();
    let horizontal = matches!(handle.edge, Edge::Right);
    let resized = handle.show(ui, &mut drag_start);
    ui.ctx()
        .data_mut(|data| data.insert_temp(drag_id, drag_start));
    ui_probe::button(ui, name, &resized.response);
    if resized.response.changed()
        && let Some(mut state) = egui::containers::panel::PanelState::load(ui.ctx(), panel)
    {
        if horizontal {
            state.rect.max.x = state.rect.min.x + resized.size;
        } else {
            state.rect.min.y = state.rect.max.y - resized.size;
        }
        ui.ctx()
            .data_mut(|data| data.insert_persisted(panel, state));
        ui.ctx().request_repaint();
    }
}
