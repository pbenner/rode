//! Terminal-header allocation workflow. Scheduler polling is demand driven;
//! terminal creation captures the host/policy through the normal session worker.
use super::*;

#[cfg(test)]
mod tests;

impl EditorApp {
    /// Open the allocation form using this project's saved scheduler defaults.
    pub(in crate::app) fn open_slurm(&mut self, ctx: &egui::Context) {
        self.sessions.allocation_draft = self.workbench.slurm_allocation().clone();
        self.sessions.allocation_sandbox = self.workbench.slurm_sandbox();
        self.sessions.allocation_error.clear();
        self.sessions.confirm = None;
        self.set_dialog(Dialog::Slurm);
        self.refresh_slurm(None, ctx);
    }

    /// A sandbox choice must never silently degrade to an unrestricted allocation.
    /// The returned plan is captured by the same controller used for ordinary terminals.
    fn slurm_allocation_plan(&self) -> Result<sandbox::Plan, String> {
        let plan = self.terminal_plan(self.sessions.allocation_sandbox);
        if self.sessions.allocation_sandbox && !plan.active {
            return Err(if plan.requested && !plan.reason.is_empty() {
                format!("Project sandbox unavailable: {}", plan.reason)
            } else {
                "Enable the project sandbox in Project settings to use Sandboxed.".into()
            });
        }
        Ok(plan)
    }
    fn current_allocation(&self) -> Option<&slurm::Job> {
        let name = self
            .sessions
            .active
            .as_ref()?
            .native
            .as_ref()?
            .allocation
            .as_ref()?;
        self.sessions
            .slurm
            .as_ref()?
            .jobs
            .iter()
            .find(|job| &job.name == name)
    }
    /// Keep persistence at the far right and allocation immediately beside it.
    /// Only a fresh RUNNING scheduler state gets the active fill; terminal liveness
    /// and query errors must not leave a misleading green allocation indicator.
    pub(in crate::app) fn slurm_terminal_button(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let height = layout::compact_control_height(ui);
        let managed = self
            .sessions
            .active
            .as_ref()
            .and_then(|m| m.native.as_ref())
            .is_some_and(|info| info.allocation.is_some());
        let alive = self.terminal.as_ref().is_some_and(Terminal::is_alive);
        let stale = !self.sessions.slurm_error.is_empty()
            || self
                .sessions
                .slurm_last
                .is_none_or(|time| time.elapsed() > Duration::from_secs(25));
        let job = self.current_allocation().cloned();
        let active =
            managed && alive && !stale && job.as_ref().is_some_and(|job| job.state == "RUNNING");
        let label = if managed && !alive {
            "Slurm: offline".into()
        } else if managed && !self.sessions.slurm_error.is_empty() {
            "Slurm: error".into()
        } else if managed && stale {
            "Slurm: checking".into()
        } else if let Some(job) = &job {
            if active {
                format!(
                    "Slurm: {} GPU",
                    job.gpus
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "?".into())
                )
            } else {
                format!(
                    "Slurm: {}",
                    if job.state == "PENDING" {
                        "pending"
                    } else {
                        &job.state
                    }
                )
            }
        } else if managed {
            "Slurm: pending".into()
        } else {
            "Slurm…".into()
        };
        let mut button = crate::keyboard::Button::new(RichText::new(label).color(if active {
            palette.background
        } else {
            palette.text
        }))
        .small()
        .min_size(egui::vec2(100., height));
        if active {
            button = button
                .fill(palette.accent)
                .stroke(egui::Stroke::new(1.0_f32, palette.accent));
        }
        let ready = self
            .workspace
            .as_ref()
            .is_none_or(|w| w.ready && w.connected);
        let mut detail = job
            .as_ref()
            .map(|j| format!("Job {} · {} · {}\n{}", j.id, j.state, j.nodes, j.tres))
            .unwrap_or_else(|| {
                "Request an interactive allocation in a new terminal on this host.".into()
            });
        if managed && !alive {
            detail = "Terminal view ended; the allocation may still be running. Open Sessions to reattach or use this control to query/release it.".into();
        }
        if !self.sessions.slurm_error.is_empty() {
            detail.push('\n');
            detail.extend(self.sessions.slurm_error.chars().take(320));
        }
        let response = ui.add_enabled_ui(ready && !self.session_busy(), |ui| ui.add_sized([112., height], button.truncate())).inner
            .on_hover_text(format!("{detail}\nAllocate resources, inspect progress or release the allocation. Persistence is controlled separately."));
        super::super::ui_probe::button(ui, "terminal-allocation", &response);
        if response.clicked() {
            self.open_slurm(ctx);
        }
        if let Some(job) = job.filter(|job| job.state == "PENDING")
            && ui
                .add_enabled(
                    ready && !self.sessions.slurm_busy,
                    crate::keyboard::Button::new("Cancel queued")
                        .small()
                        .min_size(egui::vec2(0., height)),
                )
                .on_hover_text("Cancel this terminal's queued Slurm allocation.")
                .clicked()
        {
            self.open_slurm(ctx);
            self.sessions.confirm = Some(Confirm::Release(job));
        }
    }

    /// Validate requested resources and queue an explicit allocation on the selected project host.
    pub(in crate::app) fn slurm_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let top = ui.cursor().top();
        layout::heading(
            ui,
            "Slurm allocation",
            "Choose compute resources for a terminal or notebook.",
            palette,
        );
        let host = self
            .session_remote()
            .map(|r| format!("SSH · {}", r.profile.endpoint()))
            .unwrap_or_else(|| "LOCAL HOST".into());
        ui.add(egui::Label::new(palette.tag(&host)).truncate())
            .on_hover_text(&host);
        ui.add_space(8.);
        let ready = self.terminal_creation_ready();
        let labels = ["Allocate in new terminal", "Reserve for notebooks", "Close"];
        let spacing = ui.spacing().item_spacing;
        let stacked = labels
            .iter()
            .map(|label| layout::button_width(ui, label))
            .sum::<f32>()
            + 2. * spacing.x
            > ui.available_width();
        let control_height = layout::control_height(ui);
        // Reserve the actual action rows before sizing the sole scroll area.
        // Errors, expanded options and short viewports cannot displace the footer.
        let actions_height = if stacked {
            3. * control_height + 2. * spacing.y
        } else {
            control_height
        };
        let footer_height = actions_height + 3. * spacing.y + 8.;
        let body_height =
            (ctx.content_rect().height() - 64. - (ui.cursor().top() - top) - footer_height).max(1.);
        let form = egui::ScrollArea::vertical()
            .id_salt("allocation-form")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .auto_shrink([false, true])
            .min_scrolled_height(body_height)
            .max_height(body_height)
            .show(ui, |ui| self.slurm_allocation_form(ui, ctx));
        ui.separator();
        ui.add_space(8.);
        let plan = self.slurm_allocation_plan();
        let action_width = if stacked { ui.available_width() } else { 0. };
        let action_layout = if stacked {
            Layout::top_down(Align::Min)
        } else {
            Layout::left_to_right(Align::Center)
        };
        // A horizontal scope otherwise centres controls in all remaining height.
        // Bound both footer variants to their measured rows, even in a tall modal.
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), actions_height), action_layout, |ui| {
            let interactive = ui.add_enabled(ready && plan.is_ok(),
                crate::keyboard::Button::new(RichText::new(labels[0]).color(palette.background).strong())
                    .fill(palette.accent)
                    .min_size(egui::vec2(action_width, control_height)))
                .on_hover_text("Wait for resources, then open a login shell on the first allocated node. Your current terminal keeps running. Use Persist to keep the new terminal after closing RODE.");
            super::super::ui_probe::button(ui, "slurm-allocate", &interactive);
            let reserve = ui.add_enabled(
                ready && plan.is_ok(),
                crate::keyboard::Button::new(labels[1])
                    .min_size(egui::vec2(action_width, control_height)),
            ).on_hover_text("Reserve resources for notebook kernels. Starts with Persist on; turn it off in the allocation's progress view to release resources when its last view closes or SSH disconnects. Persist kernel is separate.");
            super::super::ui_probe::button(ui, "slurm-reserve", &reserve);
            if interactive.clicked() || reserve.clicked() {
                let settings = self.sessions.allocation_draft.clone();
                match settings.validate() {
                    Ok(()) => {
                        let Ok(plan) = plan else {
                            return;
                        };
                        self.workbench.save_slurm_allocation(settings.clone());
                        self.workbench
                            .save_slurm_sandbox(self.sessions.allocation_sandbox);
                        self.create_terminal(
                            reserve.clicked(),
                            Some(settings),
                            reserve.clicked(),
                            plan,
                            ctx,
                        );
                        if self.sessions.starting {
                            self.dialog = Dialog::None;
                        }
                    }
                    Err(error) => {
                        self.sessions.allocation_error = error.to_string();
                        // Feedback is at the end of the scrollable form. Reveal it
                        // after a rejected submission, including expanded forms.
                        let mut state = form.state;
                        state.offset.y = form.content_size.y;
                        state.store(ctx, form.id);
                        ctx.request_repaint();
                    }
                }
            }
            let close = ui.add(crate::keyboard::Button::new(labels[2])
                .min_size(egui::vec2(action_width, control_height)));
            super::super::ui_probe::button(ui, "slurm-close", &close);
            if close.clicked() {
                self.dialog = Dialog::None;
            }
        });
    }

    /// Scrollable fields and feedback; all creation actions live in the fixed footer.
    fn slurm_allocation_form(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let project = self.session_plan();
        let ready = self.terminal_creation_ready();
        if let Some(job) = self.current_allocation().cloned() {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("Job {} · {} · {}", job.id, job.state, job.nodes));
                if ui
                    .add_enabled(
                        !self.sessions.slurm_busy,
                        crate::keyboard::Button::new(if job.state == "PENDING" {
                            "Cancel pending allocation"
                        } else {
                            "Release allocation"
                        }),
                    )
                    .clicked()
                {
                    self.sessions.confirm = Some(Confirm::Release(job));
                }
            });
        }
        self.session_confirmation(ui, ctx);
        if !self.sessions.slurm_error.is_empty() {
            ui.colored_label(palette.error, &self.sessions.slurm_error);
        }
        ui.horizontal_wrapped(|ui| {
            ui.strong("Execution mode");
            for (sandboxed, title, probe) in [
                (true, "Sandboxed", "slurm-mode-sandbox"),
                (false, "Outside sandbox", "slurm-mode-host"),
            ] {
                let response = ui
                    .add_enabled_ui(ready && (!sandboxed || project.active), |ui| {
                        crate::keyboard::selectable_value(
                            ui,
                            &mut self.sessions.allocation_sandbox,
                            sandboxed,
                            title,
                        )
                    })
                    .inner;
                super::super::ui_probe::button(ui, probe, &response);
                if sandboxed && !project.active {
                    response.on_disabled_hover_text(
                        if project.requested && !project.reason.is_empty() {
                            project.reason.as_str()
                        } else {
                            "Enable the project sandbox in Project settings."
                        },
                    );
                }
            }
        });
        let warning = ui.add(egui::Label::new(RichText::new(
                    if self.sessions.allocation_sandbox {
                        "GPU tools need Device access grants and readable driver files in the project sandbox rules. Compute nodes must support the same sandbox backend."
                    } else {
                        "Outside sandbox: commands have your normal access to host files, devices and network."
                    }
                ).small().color(palette.accent)).wrap());
        super::super::ui_probe::value(
            ui,
            "slurm-execution-mode",
            &warning,
            if self.sessions.allocation_sandbox {
                "sandboxed"
            } else {
                "outside_sandbox"
            },
        );
        ui.small(
            "Notebook kernels use the project sandbox independently of this allocation's mode.",
        );
        layout::section(ui, "Resources", palette, |ui| {
            let draft = &mut self.sessions.allocation_draft;
            let compact = ui.available_width() < 460.;
            egui::Grid::new("slurm-resources")
                .num_columns(if compact { 1 } else { 2 })
                .spacing([12., 6.])
                .show(ui, |ui| {
                    layout::field_label(ui, "GPUs (total)", compact);
                    ui.add(crate::keyboard::named(
                        "GPUs (total)",
                        egui::DragValue::new(&mut draft.gpus).range(0..=65536),
                    ));
                    ui.end_row();
                    for (label, value, hint) in [
                        (
                            "Time (H or H:MM)",
                            &mut draft.time,
                            "2 or 2:30; full Slurm time also accepted",
                        ),
                        (
                            "Memory (GB/node)",
                            &mut draft.memory,
                            "80; explicit units such as 80000M also accepted",
                        ),
                    ] {
                        layout::field_label(ui, label, compact);
                        ui.add(crate::keyboard::named(
                            label,
                            layout::text_input(ui, value)
                                .hint_text(hint)
                                .char_limit(1024)
                                .desired_width(ui.available_width().max(1.)),
                        ));
                        ui.end_row();
                    }
                });
            let advanced =
                crate::keyboard::collapsing(ui, "Advanced resources / site settings", |ui| {
                    egui::Grid::new("slurm-advanced")
                        .num_columns(if compact { 1 } else { 2 })
                        .spacing([12., 6.])
                        .show(ui, |ui| {
                            for (label, value, hint) in [
                                ("Partition", &mut draft.partition, "Site default"),
                                ("Account", &mut draft.account, "Site default"),
                                ("QoS", &mut draft.qos, "Site default"),
                                ("Reservation", &mut draft.reservation, "None"),
                                (
                                    "GRES per node",
                                    &mut draft.gres,
                                    "Optional gpu:a100:2; replaces the total GPU field",
                                ),
                                (
                                    "Node features",
                                    &mut draft.constraint,
                                    "Optional --constraint value",
                                ),
                            ] {
                                layout::field_label(ui, label, compact);
                                ui.add(crate::keyboard::named(
                                    label,
                                    layout::text_input(ui, value)
                                        .hint_text(hint)
                                        .char_limit(1024)
                                        .desired_width(ui.available_width().max(1.)),
                                ));
                                ui.end_row();
                            }
                            layout::field_label(ui, "Nodes", compact);
                            ui.add(crate::keyboard::named(
                                "Nodes",
                                egui::DragValue::new(&mut draft.nodes).range(1..=4096),
                            ));
                            ui.end_row();
                            layout::field_label(ui, "CPUs per task", compact);
                            ui.add(crate::keyboard::named(
                                "CPUs per task",
                                egui::DragValue::new(&mut draft.cpus).range(1..=65536),
                            ));
                            ui.end_row();
                        });
                    ui.label("Launch executables / site wrappers");
                    ui.label("salloc");
                    ui.add(crate::keyboard::named(
                        "salloc executable",
                        layout::text_input(ui, &mut draft.allocator),
                    ));
                    ui.label("srun");
                    ui.add(crate::keyboard::named(
                        "srun executable",
                        layout::text_input(ui, &mut draft.runner),
                    ));
                });
            super::super::ui_probe::button(ui, "slurm-advanced", &advanced.header_response);
            self.slurm_settings_ui(ui);
        });
        let lifetime = crate::keyboard::collapsing(ui, "Session lifetime", |ui| {
            ui.label("Persist controls whether an allocation survives its last view closing or SSH disconnecting. Notebook reservations start with Persist on; change it in their progress view. Persist kernel controls only the notebook kernel. Release retained allocations explicitly when finished.");
        });
        super::super::ui_probe::button(ui, "slurm-lifetime", &lifetime.header_response);
        let plan = self.slurm_allocation_plan();
        if let Err(reason) = &plan {
            ui.colored_label(palette.error, reason);
        }
        if !self.sessions.allocation_error.is_empty() {
            ui.colored_label(palette.error, &self.sessions.allocation_error);
        }
    }
}
