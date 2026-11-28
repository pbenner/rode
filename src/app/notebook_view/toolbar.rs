//! Compact notebook actions with bounded, optional kernel settings.
//!
//! The toolbar owns a separate surface and bottom rule. Only the cell body
//! scrolls, so output and editor painting cannot cover the controls.
use super::*;

impl EditorApp {
    /// Lay out one compact action row, with optional settings and find rows below.
    pub(super) fn notebook_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        index: usize,
        view: &mut View,
    ) -> Option<Action> {
        let palette = self.themes.active().palette;
        let id = self.documents[index].id;
        let mut action = None;
        let toolbar = layout::document_toolbar(ui, palette, |ui| {
            ui.horizontal_wrapped(|ui| {
                if view.source_mode {
                    ui.label(palette.tag("JSON SOURCE"));
                    if crate::keyboard::button(ui, "Back to cells").clicked() {
                        action = Some(Action::Source);
                    }
                    return;
                }
                let busy = view.kernel.as_ref().is_some_and(Kernel::busy);
                if ui
                    .add_enabled(!busy, IconButton::new(Icon::Play, "Run cell"))
                    .clicked()
                {
                    action = Some(Action::Run(false));
                }
                if ui
                    .add_enabled(!busy, crate::keyboard::Button::new("Run all"))
                    .clicked()
                {
                    action = Some(Action::Run(true));
                }
                if ui
                    .add_enabled(
                        view.kernel.is_some(),
                        crate::keyboard::Button::new("Interrupt"),
                    )
                    .clicked()
                {
                    action = Some(Action::Interrupt);
                }
                let insert = crate::keyboard::menu_button(ui, "Insert…", |ui| {
                    let insert_at = view
                        .active_index()
                        .map_or(view.book.cells.len(), |at| at + 1);
                    let code = crate::keyboard::button(ui, "Code cell")
                        .on_hover_text("Insert a code cell below the selected cell");
                    ui_probe::button(ui, "notebook-add-code", &code);
                    if code.clicked() {
                        action = Some(Action::Add("code", insert_at));
                        ui.close();
                    }
                    let markdown = crate::keyboard::button(ui, "Markdown cell")
                        .on_hover_text("Insert a text cell below the selected cell");
                    ui_probe::button(ui, "notebook-add-markdown", &markdown);
                    if markdown.clicked() {
                        action = Some(Action::Add("markdown", insert_at));
                        ui.close();
                    }
                });
                ui_probe::button(ui, "notebook-insert", &insert.response);
                crate::keyboard::menu_button(ui, "Notebook…", |ui| {
                    if crate::keyboard::button(ui, "Clear all outputs").clicked() {
                        action = Some(Action::Clear(None));
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            view.kernel.is_some(),
                            crate::keyboard::Button::new("Shut down kernel"),
                        )
                        .clicked()
                    {
                        action = Some(Action::Shutdown);
                        ui.close();
                    }
                    ui.separator();
                    if crate::keyboard::button(ui, "Edit JSON source").clicked() {
                        action = Some(Action::Source);
                        ui.close();
                    }
                });
                let settings = ui.add(
                    crate::keyboard::Button::new("Kernel settings").selected(view.settings_open),
                );
                ui_probe::button(ui, "notebook-settings", &settings);
                if settings.clicked() {
                    view.settings_open = !view.settings_open;
                }
                let status = view
                    .kernel
                    .as_ref()
                    .map(|k| k.status.as_str())
                    .unwrap_or("Not started");
                let execution = view
                    .kernel
                    .as_ref()
                    .map(|k| k.execution.clone())
                    .unwrap_or_else(|| {
                        view.execution_target
                            .as_ref()
                            .map(|t| format!("Slurm job {}", t.id))
                            .unwrap_or_else(|| "Project host".into())
                    });
                if busy {
                    ui.spinner();
                }
                let target = view
                    .execution_target
                    .as_ref()
                    .map(|t| format!("Slurm {}", t.id))
                    .unwrap_or_else(|| "Project host".into());
                ui.add(egui::Label::new(palette.tag(format!("{status} · {target}"))).truncate())
                    .on_hover_text(format!(
                        "{}\n{} · {}\n{}",
                        status, view.kernel_name, execution, view.status
                    ));
            });
            if !view.source_mode {
                if view.settings_open {
                    ui.add_space(6.);
                    // Reserve room for cells even with large fonts or a short viewport.
                    let height = (ui.available_height() - 120.).clamp(1., 240.);
                    egui::ScrollArea::vertical().id_salt(("notebook-settings", id))
                            .max_height(height).auto_shrink([false, true]).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let on = view.kernel.as_ref().map_or(view.persistent, Kernel::persistent);
                        let persistence = ui.add_enabled(
                            view.kernel.as_ref().is_none_or(|k| !k.pending()),
                            crate::keyboard::Button::new(if on { "Persist kernel: on" } else { "Persist kernel: off" }),
                        ).on_hover_text("Keep the kernel, queued cells and outputs on the project host after closing this notebook, exiting RODE or losing SSH. Reattach from Sessions with the same project and sandbox.");
                        ui_probe::button(ui, "notebook-persistence", &persistence);
                        if persistence.clicked() {
                            if !on && self.documents[index].path.is_none() {
                                view.error = "Save this notebook before enabling persistence".into();
                            } else if let Some(kernel) = &mut view.kernel {
                                match kernel.set_persistent(!on) {
                                    Ok(()) => view.persistent = !on,
                                    Err(error) => view.error = error.to_string(),
                                }
                            } else {
                                view.persistent = !on;
                            }
                        }
                        if crate::keyboard::button(ui, "Running kernels…").clicked() {
                            self.open_sessions(ctx);
                        }
                        if view.kernel.is_none() {
                            let reattach = crate::keyboard::button(ui, "Reattach kernel");
                            ui_probe::button(ui, "notebook-reattach", &reattach);
                            if reattach.clicked() {
                            if let Some(path) = &self.documents[index].path {
                                self.find_notebook_runtime(path.clone(), ctx);
                            } else {
                                view.error = "Save this notebook before reattaching a kernel".into();
                            }
                            }
                        }
                        if view.kernel.as_ref().is_some_and(Kernel::persistent)
                            && ui.add_enabled(
                                view.kernel.as_ref().is_none_or(|k| !k.pending()),
                                crate::keyboard::Button::new("Detach kernel"),
                            ).clicked()
                        {
                            view.detach("Kernel detached · Reattach from Sessions");
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Output RAM budget");
                        let mut selected = self.workbench.notebook_output_budget();
                        let previous = selected;
                        let response = crate::keyboard::ComboBox::new(("notebook-output-budget", id), "Notebook output RAM budget")
                            .selected_text(format!("{} MiB", selected.mib()))
                            .show_ui(ui, |ui| {
                                for preset in rode_runtime_core::notebook::outputs::MemoryBudget::PRESETS {
                                    let response = crate::keyboard::selectable_value(ui, &mut selected, preset, format!("{} MiB", preset.mib()));
                                    ui_probe::button(ui, &format!("notebook-output-budget-{}", preset.mib()), &response);
                                }
                            }).response.on_hover_text("All live outputs in this notebook share this RAM allowance. No separate text, image or record-count cap. Saved for this project and applied to the next Run or Run All; existing output is kept. Output is never spooled to disk.");
                        ui_probe::button(ui, "notebook-output-budget", &response);
                        ui_probe::value(ui, "notebook-output-budget-value", &response, &selected.mib().to_string());
                        if selected != previous { self.workbench.save_notebook_output_budget(selected); }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Execution target");
                        let mut selected = view.execution_target.clone();
                        let (queue, _) = self.slurm_programs();
                        let runner = self.workbench.slurm_allocation().runner.clone();
                        let target = crate::keyboard::ComboBox::new(("notebook-execution", id), "Notebook execution target")
                            .selected_text(
                                selected.as_ref()
                                    .map(|t| format!("Slurm job {}", t.id))
                                    .unwrap_or_else(|| "Project host".into()),
                            )
                            .show_ui(ui, |ui| {
                                let host = crate::keyboard::selectable_value(ui, &mut selected, None, "Project host");
                                ui_probe::button(ui, "notebook-target:host", &host);
                                if let Some(snapshot) = &self.sessions.slurm {
                                    for job in &snapshot.jobs {
                                        if job.state == "RUNNING"
                                            && job.id.bytes().all(|b| b.is_ascii_digit())
                                        {
                                            let target = crate::slurm::Target {
                                                id: job.id.clone(),
                                                name: job.name.clone(),
                                                queue: queue.clone(),
                                                runner: runner.clone(),
                                            };
                                            let job_response = crate::keyboard::selectable_value(ui,
                                                &mut selected, Some(target),
                                                format!("Job {} · {} · {}", job.id, job.nodes, job.name),
                                            );
                                            ui_probe::button(ui, &format!("notebook-target:{}", job.id), &job_response);
                                        }
                                    }
                                }
                            }).response.on_hover_text(
                                "Changing execution target stops this kernel and clears its live variables. Run a cell to start a new kernel. Slurm kernels share the selected allocation's resources."
                            );
                        ui_probe::button(ui, "notebook-target", &target);
                        ui_probe::value(ui, "notebook-target-value", &target, selected.as_ref().map(|target| target.id.as_str()).unwrap_or("host"));
                        view.set_execution_target(selected);
                        let refresh = ui.add_enabled(
                            !self.sessions.slurm_busy,
                            crate::keyboard::Button::new("Refresh Slurm targets"),
                        );
                        ui_probe::button(ui, "notebook-refresh-targets", &refresh);
                        if !self.sessions.slurm_busy && self.sessions.slurm.is_some() {
                            ui_probe::button(ui, "notebook-targets-ready", &refresh);
                        }
                        if refresh.clicked() {
                            self.refresh_slurm(None, ctx);
                        }
                        if crate::keyboard::button(ui, "Allocate…").clicked() {
                            self.open_slurm(ctx);
                        }
                        if let Some(kernel) = &view.kernel {
                            ui.small(&kernel.execution);
                        }
                    });
                    if !self.sessions.slurm_error.is_empty() {
                        ui.colored_label(palette.muted, &self.sessions.slurm_error);
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Python environment");
                        let previous = self.terminal_python_selection();
                        ui.push_id(("notebook-python", id), |ui| self.python_environment_picker(ui, ctx));
                        if previous != self.terminal_python_selection() {
                            view.detach("Notebook environment changed");
                            view.kernel_name = "python3".into();
                            action = Some(Action::Kernel);
                        }
                        if ui.add_enabled(!self.python_environment_busy(), crate::keyboard::Button::new("Refresh kernels")).clicked() {
                            self.refresh_environment(false, ctx);
                        }
                        if self.python_environment_busy() { ui.spinner(); }
                    });
                    if !self.python_environment_error().is_empty() {
                        ui.colored_label(palette.error, self.python_environment_error());
                    }
                    if let Some(env) = self.python_environments()
                        && !env.kernel_error.is_empty() {
                        ui.colored_label(palette.muted, &env.kernel_error);
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Kernel");
                        let previous = view.kernel_name.clone();
                        crate::keyboard::ComboBox::new(("notebook-kernel", id), "Notebook kernel")
                            .width(300_f32.min(ui.available_width().max(1.)))
                            .selected_text(if view.kernel_name == "python3" {
                                "Python · selected environment".into()
                            } else {
                                self.python_environments()
                                    .and_then(|env| env.kernels.iter().find(|k| k.name == view.kernel_name))
                                    .map(|k| k.display_name.clone())
                                    .unwrap_or_else(|| view.kernel_name.clone())
                            })
                            .show_ui(ui, |ui| {
                                crate::keyboard::selectable_value(ui, &mut view.kernel_name, "python3".into(), "Python · selected environment");
                                if let Some(env) = self.python_environments() {
                                    for kernel in &env.kernels {
                                        if kernel.name != "python3" {
                                            crate::keyboard::selectable_value(ui, &mut view.kernel_name, kernel.name.clone(), format!("{} ({})", kernel.display_name, kernel.name));
                                        }
                                    }
                                }
                            });
                        if previous != view.kernel_name { action = Some(Action::Kernel); }
                    });
                    });
                }
                if self.editor.show_find {
                    ui.horizontal(|ui| {
                        let response = ui.add(crate::keyboard::named(
                            "Find in cell sources…",
                            egui::TextEdit::singleline(&mut self.editor.find)
                                .hint_text("Find in cell sources…")
                                .desired_width(220.),
                        ));
                        if self.editor.find_focus {
                            response.request_focus();
                            self.editor.find_focus = false;
                        }
                        ui_probe::button(ui, "notebook-find", &response);
                        ui_probe::value(ui, "notebook-find-value", &response, &self.editor.find);
                        if (crate::keyboard::button(ui, "Next").clicked()
                            || (response.has_focus() || response.lost_focus())
                                && ui.input(|i| i.key_pressed(Key::Enter)))
                            && !self.editor.find.is_empty()
                        {
                            let start = view.active_index().unwrap_or(0);
                            if let Some((at, range)) = (0..view.book.cells.len())
                                .map(|n| (start + n + 1) % view.book.cells.len())
                                .find_map(|at| {
                                    find_matches(
                                        &view.book.cells[at].source,
                                        &self.editor.find,
                                        self.editor.case_sensitive,
                                    )
                                    .first()
                                    .copied()
                                    .map(|r| (at, r))
                                })
                            {
                                let cell = &view.book.cells[at];
                                let a = cell.source[..range.0].chars().count();
                                let b = cell.source[..range.1].chars().count();
                                view.active = cell.uid;
                                view.selection = Some((cell.uid, a, b));
                                view.focus = Some(cell.uid);
                                view.reveal = true;
                                view.cells.entry(cell.uid).or_default().previewing = false;
                            }
                        }
                        if crate::keyboard::button(ui, "Close").clicked() {
                            self.editor.show_find = false;
                        }
                    });
                }
                if !view.error.is_empty() {
                    ui.colored_label(palette.error, &view.error);
                }
            }
        });
        ui_probe::button(ui, "notebook-toolbar", &toolbar.response);
        action
    }
}
