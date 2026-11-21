//! Compact transfer activity, independent of the two browser layouts.
use super::*;

impl EditorApp {
    pub(super) fn transfer_queue_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut retry = None;
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            let heading = ui.strong("Activity");
            if self.transfers.running.is_none()
                && self.transfers.queue.is_empty()
                && self.transfers.file_operation.is_none()
            {
                ui_probe::button(ui, "transfer-idle", &heading);
            }
            if self.transfers.file_operation.is_some() {
                ui.spinner();
                ui.label("Updating files…");
            }
            ui.weak(if self.transfers.queue.is_empty() {
                "No waiting transfers".into()
            } else {
                format!("{} waiting", self.transfers.queue.len())
            });
            if !self.transfers.history.is_empty()
                && crate::keyboard::button(ui, "Clear finished").clicked()
            {
                self.transfers.history.clear();
            }
            if let Some(running) = &self.transfers.running {
                let response = crate::keyboard::button(ui, "Cancel transfer");
                ui_probe::button(ui, "transfer-cancel", &response);
                if response.clicked() {
                    running.cancel.store(true, Ordering::Release);
                }
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("transfer-queue-scroll")
            // A small tray must scroll rather than enlarge its parent panel.
            .min_scrolled_height(0.)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(running) = &self.transfers.running {
                    let progress = running.progress.lock().unwrap().clone();
                    ui.strong(&running.label);
                    ui.add(egui::Label::new(&progress.current).truncate())
                        .on_hover_text(&progress.current);
                    let fraction = if progress.total == 0 {
                        0.
                    } else {
                        progress.bytes as f32 / progress.total as f32
                    };
                    ui.add(egui::ProgressBar::new(fraction).text(if progress.scanning {
                        "Scanning files…".into()
                    } else {
                        format!(
                            "{} / {} · {}/s",
                            layout::bytes(progress.bytes),
                            layout::bytes(progress.total),
                            layout::bytes(
                                (progress.bytes as f64
                                    / progress.started.elapsed().as_secs_f64().max(0.1))
                                    as u64
                            )
                        )
                    }));
                    if progress.conflict.is_some() {
                        ui.label("Waiting for a collision decision…");
                    }
                }
                for (index, task) in self.transfers.queue.iter().enumerate() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(&task.label);
                        ui.add(egui::Label::new(&task.job.directory).truncate());
                        if crate::keyboard::button(ui, "Remove").clicked() {
                            remove = Some(index);
                        }
                    });
                }
                for (index, finished) in self.transfers.history.iter().enumerate() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(&finished.label);
                        ui.colored_label(
                            if finished.failed {
                                self.themes.active().palette.error
                            } else {
                                self.themes.active().palette.muted
                            },
                            &finished.result,
                        );
                        if finished.failed && crate::keyboard::button(ui, "Retry…").clicked() {
                            retry = Some(index);
                        }
                    });
                }
                if self.transfers.running.is_none()
                    && self.transfers.file_operation.is_none()
                    && self.transfers.queue.is_empty()
                    && self.transfers.history.is_empty()
                {
                    ui.weak("Ready to transfer · drag files to the other pane, or select files and use Upload / Download.");
                }
            });
        if let Some(index) = remove {
            self.transfers.queue.remove(index);
        }
        if let Some(index) = retry {
            let finished = &self.transfers.history[index];
            let mut job = finished.job.clone();
            job.protected = self.transfers.protected.clone();
            self.transfers.enqueue(Task {
                job,
                label: finished.label.clone(),
                _work: self.vaults.work(),
            });
        }
        self.transfer_tick(ctx);
    }
}
