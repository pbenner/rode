//! Resizable transfer browsers and a bounded queue, using shared native rows.
use super::*;

impl EditorApp {
    pub(in crate::app) fn transfers_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let host = self
            .workspace
            .as_ref()
            .and_then(|w| w.remote.as_ref())
            .map(|r| r.profile.endpoint())
            .unwrap_or_default();
        let compact = ui.available_height() < 540.;
        let view_width = ui.available_width().max(90.);
        let view_height = ui.available_height().max(80.);
        let queue_id = ui.id().with("transfer-queue");
        let local_id = ui.id().with("transfer-local");
        ui.horizontal_wrapped(|ui| {
            ui.heading("File transfers").on_hover_text(
                "Copies saved files on disk. Browsing here does not change the open project.",
            );
            ui.label(
                self.themes
                    .active()
                    .palette
                    .tag(format!("This computer ↔ {host}")),
            );
            if !compact && let Some(workspace) = self.workspace.as_ref() {
                ui.label(self.themes.active().palette.tag(workspace.plan.label()));
            }
            crate::keyboard::menu_button(ui, "Layout…", |ui| {
                ui.label("Resize by dragging the dividers, or use the keyboard controls below.");
                let mut left = egui::containers::panel::PanelState::load(ctx, local_id).unwrap_or(
                    egui::containers::panel::PanelState {
                        rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 300.)),
                    },
                );
                let mut size = left.rect.width();
                let control = ui.add(crate::keyboard::named(
                    "Local browser width",
                    egui::DragValue::new(&mut size)
                        .range(90. ..=view_width)
                        .speed(5.)
                        .prefix("Local pane: "),
                ));
                if control.changed() {
                    left.rect.max.x = left.rect.min.x + size;
                    ctx.data_mut(|data| data.insert_persisted(local_id, left));
                }
                let mut queue = egui::containers::panel::PanelState::load(ctx, queue_id).unwrap_or(
                    egui::containers::panel::PanelState {
                        rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 190.)),
                    },
                );
                let mut size = queue.rect.height();
                let control = ui.add(crate::keyboard::named(
                    "Transfer queue height",
                    egui::DragValue::new(&mut size)
                        .range(60. ..=(view_height * 0.55).max(80.))
                        .speed(5.)
                        .prefix("Queue height: "),
                ));
                if control.changed() {
                    queue.rect.max.y = queue.rect.min.y + size;
                    ctx.data_mut(|data| data.insert_persisted(queue_id, queue));
                }
            });
        });
        if !compact {
            ui.label(
                "Copies saved files on disk. Browsing here does not change your open project.",
            );
        }
        ui.separator();
        if let Some(error) = &self.transfers.error {
            ui.colored_label(self.themes.active().palette.error, error);
        }
        for side in 0..2 {
            if self.transfers.panes[side].browser.location.is_empty() {
                self.transfers.panes[side].browser.location = if side == 0 {
                    files::expand_home("~").to_string_lossy().into()
                } else {
                    self.workspace.as_ref().unwrap().root.clone()
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
        let maximum = (ui.available_height() - 225.)
            .min(ui.available_height() * 0.55)
            .max(60.);
        egui::TopBottomPanel::bottom(queue_id)
            .resizable(true)
            .default_height(190.)
            .height_range(60. ..=maximum)
            .show_inside(ui, |ui| self.transfer_queue_ui(ui, ctx));
        let width = ui.available_width();
        let minimum = (width * 0.45).clamp(90., 220.);
        egui::SidePanel::left(local_id)
            .frame(egui::Frame::side_top_panel(ui.style()))
            .resizable(true)
            .default_width(width * 0.5)
            .width_range(minimum..=(width - minimum - 12.).max(minimum))
            .show_inside(ui, |ui| self.transfer_pane_ui(ui, 0, ctx));
        egui::CentralPanel::default()
            .frame(egui::Frame::side_top_panel(ui.style()))
            .show_inside(ui, |ui| self.transfer_pane_ui(ui, 1, ctx));
    }
    fn transfer_pane_ui(&mut self, ui: &mut egui::Ui, side: usize, ctx: &egui::Context) {
        let mut browse = None;
        let mut create = None;
        let mut transfer = false;
        let can_transfer = self.transfers.panes.iter().all(Pane::ready);
        ui.push_id(side, |ui| {
            let budget = ui.available_height();
            let compact = budget < 330.;
            egui::ScrollArea::vertical()
                .id_salt("browser-content")
                .auto_shrink([false, false])
                .max_height(budget)
                .show(ui, |ui| {
                    let top = ui.cursor().top();
                    let pane = &mut self.transfers.panes[side];
                    ui.strong(if side == 0 {
                        "Local computer"
                    } else {
                        "Remote SSH host"
                    });
                    let path = ui.add(crate::keyboard::named(
                        "Folder path",
                        egui::TextEdit::singleline(&mut pane.browser.location)
                            .desired_width(f32::INFINITY),
                    ));
                    ui_probe::button(ui, &format!("transfer-path:{side}"), &path);
                    ui_probe::value(
                        ui,
                        &format!("transfer-path:{side}-value"),
                        &path,
                        &pane.browser.location,
                    );
                    if path.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        browse = Some(pane.browser.location.clone());
                    }
                    ui.horizontal_wrapped(|ui| {
                        for (label, destination) in [
                            ("Go", pane.browser.location.clone()),
                            (
                                "Up",
                                std::path::Path::new(&pane.browser.directory)
                                    .parent()
                                    .unwrap_or(std::path::Path::new("/"))
                                    .to_string_lossy()
                                    .into(),
                            ),
                            ("Home", "~".into()),
                            ("Refresh", pane.browser.directory.clone()),
                        ] {
                            if compact && label == "Refresh" {
                                continue;
                            }
                            let response = ui.add_enabled(
                                !pane.browser.loading,
                                crate::keyboard::Button::new(label),
                            );
                            ui_probe::button(ui, &format!("transfer-{label}:{side}"), &response);
                            if response.clicked() {
                                browse = Some(destination);
                            }
                        }
                    });
                    if !compact {
                        ui.horizontal_wrapped(|ui| {
                            let path = std::path::Path::new(&pane.browser.directory);
                            let mut ancestors: Vec<_> = path.ancestors().collect();
                            ancestors.reverse();
                            for (index, ancestor) in ancestors.iter().enumerate() {
                                if index > 0 && index + 3 < ancestors.len() {
                                    continue;
                                }
                                let label =
                                    ancestor.file_name().and_then(|v| v.to_str()).unwrap_or("/");
                                if ui
                                    .add_enabled(
                                        !pane.browser.loading,
                                        crate::keyboard::Button::new(label),
                                    )
                                    .on_hover_text(ancestor.to_string_lossy())
                                    .clicked()
                                {
                                    browse = Some(ancestor.to_string_lossy().into());
                                }
                            }
                        });
                    }
                    let query = ui.add(crate::keyboard::named(
                        "Filter files",
                        egui::TextEdit::singleline(&mut pane.query)
                            .hint_text("Filter files and folders")
                            .desired_width(f32::INFINITY),
                    ));
                    ui_probe::button(ui, &format!("transfer-filter:{side}"), &query);
                    ui_probe::value(
                        ui,
                        &format!("transfer-filter:{side}-value"),
                        &query,
                        &pane.query,
                    );
                    if !compact {
                        ui.horizontal_wrapped(|ui| pane_actions(ui, pane, false));
                    }
                    if pane.creating {
                        ui.add(crate::keyboard::named(
                            "New folder name",
                            egui::TextEdit::singleline(&mut pane.new_folder)
                                .desired_width(f32::INFINITY),
                        ));
                        if ui
                            .add_enabled(
                                explorer::valid_name(&pane.new_folder)
                                    && !pane.browser.loading
                                    && !pane.browser.directory.is_empty(),
                                crate::keyboard::Button::new("Create folder"),
                            )
                            .clicked()
                        {
                            create = Some(
                                std::path::Path::new(&pane.browser.directory)
                                    .join(&pane.new_folder)
                                    .to_string_lossy()
                                    .into(),
                            );
                            pane.creating = false;
                            pane.new_folder.clear();
                        }
                    }
                    ui.horizontal_wrapped(|ui| {
                        let label = if side == 0 {
                            "Upload →"
                        } else {
                            "← Download"
                        };
                        let button = ui.add_enabled(
                            !pane.selected.is_empty() && can_transfer,
                            crate::keyboard::Button::new(label),
                        );
                        ui_probe::button(
                            ui,
                            if side == 0 {
                                "transfer-upload"
                            } else {
                                "transfer-download"
                            },
                            &button,
                        );
                        transfer = button.clicked();
                        if compact {
                            crate::keyboard::menu_button(ui, "Actions…", |ui| {
                                pane_actions(ui, pane, true)
                            });
                        }
                        ui.label(if pane.selected.contains(&pane.browser.directory) {
                            "Current folder selected".into()
                        } else {
                            format!("{} selected", pane.selected.len())
                        });
                    });
                    ui.separator();
                    if pane.browser.loading {
                        ui.spinner();
                    } else if pane.ready() {
                        ui_probe::button(ui, &format!("transfer-ready:{side}"), &ui.response());
                        ui_probe::button(
                            ui,
                            &format!("transfer-listing:{side}:{}", pane.browser.token),
                            &ui.response(),
                        );
                        ui_probe::value(
                            ui,
                            &format!("transfer-directory:{side}"),
                            &ui.response(),
                            &pane.browser.directory,
                        );
                    }
                    if let Some(error) = &pane.browser.error {
                        ui.colored_label(self.themes.active().palette.error, error);
                    }
                    let query = pane.query.to_lowercase();
                    let entries: Vec<_> = pane
                        .browser
                        .entries
                        .iter()
                        .filter(|entry| entry.name.to_lowercase().contains(&query))
                        .cloned()
                        .collect();
                    let choice = file_browser::rows_with_options(
                        ui,
                        &format!("transfer-rows:{side}"),
                        &entries,
                        pane.hidden,
                        &mut pane.list,
                        &pane.query,
                        (0, false),
                        None,
                        !pane.browser.loading,
                        file_browser::RowOptions {
                            height: Some((budget - (ui.cursor().top() - top)).max(70.)),
                            checked: Some(&pane.selected),
                        },
                    );
                    match choice {
                        Some(file_browser::Choice::Select(entry)) => {
                            if !pane.selected.insert(entry.path.clone()) {
                                pane.selected.remove(&entry.path);
                            }
                        }
                        Some(file_browser::Choice::Open(entry)) if entry.dir => {
                            browse = Some(entry.path)
                        }
                        Some(file_browser::Choice::Open(entry)) => {
                            pane.selected.insert(entry.path);
                        }
                        Some(file_browser::Choice::Parent) => {
                            browse = Some(
                                std::path::Path::new(&pane.browser.directory)
                                    .parent()
                                    .unwrap_or(std::path::Path::new("/"))
                                    .to_string_lossy()
                                    .into(),
                            )
                        }
                        None => {}
                    }
                });
        });
        if let Some(path) = create {
            self.transfer_list(
                side,
                self.transfers.panes[side].browser.directory.clone(),
                Some(path),
                ctx,
            );
        }
        if let Some(path) = browse {
            self.transfer_list(side, path, None, ctx);
        }
        if transfer {
            let mut paths: Vec<_> = self.transfers.panes[side]
                .selected
                .iter()
                .cloned()
                .collect();
            paths.sort();
            self.queue_transfer(side, paths, ctx);
        }
    }
    fn transfer_queue_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut retry = None;
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            let heading = ui.strong("Transfer queue");
            if self.transfers.running.is_none() && self.transfers.queue.is_empty() {
                ui_probe::button(ui, "transfer-idle", &heading);
            }
            ui.label(format!("{} waiting", self.transfers.queue.len()));
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
                            workbench::bytes(progress.bytes),
                            workbench::bytes(progress.total),
                            workbench::bytes(
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
                    && self.transfers.queue.is_empty()
                    && self.transfers.history.is_empty()
                {
                    ui.label("Select files or folders above, then Upload or Download.");
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

fn pane_actions(ui: &mut egui::Ui, pane: &mut Pane, compact: bool) {
    crate::keyboard::checkbox(ui, &mut pane.hidden, "Hidden files");
    if crate::keyboard::button(ui, "Select all").clicked() {
        pane.selected = pane
            .browser
            .entries
            .iter()
            .filter(|entry| {
                (pane.hidden || !entry.name.starts_with('.'))
                    && entry
                        .name
                        .to_lowercase()
                        .contains(&pane.query.to_lowercase())
            })
            .map(|entry| entry.path.clone())
            .collect();
    }
    if crate::keyboard::button(ui, "Clear").clicked() {
        pane.selected.clear();
    }
    if pane.ready() && crate::keyboard::button(ui, "Select this folder").clicked() {
        pane.selected = HashSet::from([pane.browser.directory.clone()]);
    }
    if compact && pane.ready() && crate::keyboard::button(ui, "Refresh folder").clicked() {
        pane.browser.location = pane.browser.directory.clone();
        pane.browser.needs_load = true;
    }
    if crate::keyboard::button(ui, "New folder…").clicked() {
        pane.creating = !pane.creating;
    }
}
