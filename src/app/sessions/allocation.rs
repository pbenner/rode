//! Terminal-header allocation workflow. Scheduler polling is demand driven;
//! terminal creation captures the host/policy through the normal session worker.
use super::*;

impl EditorApp {
    pub(in crate::app) fn open_slurm(&mut self, ctx: &egui::Context) {
        self.sessions.allocation_draft = self.workbench.slurm_allocation().clone();
        self.sessions.allocation_error.clear();
        self.sessions.confirm = None;
        self.set_dialog(Dialog::Slurm);
        self.refresh_slurm(None, ctx);
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
        let job = self.current_allocation();
        let active = managed && alive && !stale && job.is_some_and(|job| job.state == "RUNNING");
        let label = if managed && !alive {
            "Slurm: offline".into()
        } else if managed && !self.sessions.slurm_error.is_empty() {
            "Slurm: error".into()
        } else if managed && stale {
            "Slurm: checking".into()
        } else if let Some(job) = job {
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
        let mut button = egui::Button::new(RichText::new(label).color(if active {
            palette.background
        } else {
            palette.text
        }))
        .small()
        .min_size(egui::vec2(100., 0.));
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
        if ui.add_enabled_ui(ready && !self.session_busy(), |ui| ui.add_sized([112., 18.], button.truncate())).inner
            .on_hover_text(format!("{detail}\nAllocate resources, inspect progress or release the allocation. Persistence is controlled separately."))
            .clicked()
        {
            self.open_slurm(ctx);
        }
    }

    pub(in crate::app) fn slurm_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.heading("Interactive Slurm allocation");
        let host = self
            .session_remote()
            .map(|r| format!("SSH · {}", r.profile.endpoint()))
            .unwrap_or_else(|| "LOCAL HOST".into());
        ui.label(palette.tag(host));
        if let Some(job) = self.current_allocation().cloned() {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("Job {} · {} · {}", job.id, job.state, job.nodes));
                if ui
                    .add_enabled(
                        !self.sessions.slurm_busy,
                        egui::Button::new(if job.state == "PENDING" {
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
            egui::ScrollArea::vertical()
                .id_salt("slurm-allocation-error")
                .max_height(72.)
                .show(ui, |ui| {
                    ui.colored_label(palette.error, &self.sessions.slurm_error);
                });
        }
        ui.label("A new terminal waits for resources, then opens your login shell on the first allocated node. Your current terminal keeps running.");
        egui::ScrollArea::vertical()
            .id_salt("allocation-form")
            .max_height((ctx.content_rect().height() - 320.).clamp(140., 460.))
            .show(ui, |ui| {
                let draft = &mut self.sessions.allocation_draft;
                egui::Grid::new("slurm-resources")
                    .num_columns(2)
                    .spacing([12., 6.])
                    .show(ui, |ui| {
                        ui.label("GPUs (total)");
                        ui.add(egui::DragValue::new(&mut draft.gpus).range(0..=65536));
                        ui.end_row();
                        for (label, value, hint) in [
                            (
                                "Time (H or H:MM)",
                                &mut draft.time,
                                "2 or 2:30; full Slurm time also accepted",
                            ),
                            (
                                "Memory per node (GB)",
                                &mut draft.memory,
                                "80; explicit units such as 80000M also accepted",
                            ),
                        ] {
                            ui.label(label);
                            ui.add(
                                egui::TextEdit::singleline(value)
                                    .hint_text(hint)
                                    .char_limit(1024)
                                    .desired_width(300.),
                            );
                            ui.end_row();
                        }
                    });
                ui.collapsing("Advanced resources / site settings", |ui| {
                    egui::Grid::new("slurm-advanced")
                        .num_columns(2)
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
                                ui.label(label);
                                ui.add(
                                    egui::TextEdit::singleline(value)
                                        .hint_text(hint)
                                        .char_limit(1024)
                                        .desired_width(320.),
                                );
                                ui.end_row();
                            }
                            ui.label("Nodes");
                            ui.add(egui::DragValue::new(&mut draft.nodes).range(1..=4096));
                            ui.end_row();
                            ui.label("CPUs per task");
                            ui.add(egui::DragValue::new(&mut draft.cpus).range(1..=65536));
                            ui.end_row();
                        });
                    ui.label("Launch executables / site wrappers");
                    ui.label("salloc");
                    ui.text_edit_singleline(&mut draft.allocator);
                    ui.label("srun");
                    ui.text_edit_singleline(&mut draft.runner);
                });
                self.slurm_settings_ui(ui);
            });
        ui.label("Slurm supplies SLURM_JOB_ID, node/resource variables and GPU visibility to the shell. Enable Persist in that terminal if it should survive closing RODE.");
        if self.session_plan().active {
            ui.colored_label(palette.muted, "The active sandbox is also applied on the compute node. It must support the same paths and bubblewrap; GPU device access requires explicit grants.");
        }
        if !self.sessions.allocation_error.is_empty() {
            ui.colored_label(palette.error, &self.sessions.allocation_error);
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.session_busy(),
                    egui::Button::new("Allocate in new terminal"),
                )
                .clicked()
            {
                let settings = self.sessions.allocation_draft.clone();
                match settings.validate() {
                    Ok(()) => {
                        self.workbench.save_slurm_allocation(settings.clone());
                        self.create_terminal(false, Some(settings), ctx);
                        if self.sessions.starting {
                            self.dialog = Dialog::None;
                        }
                    }
                    Err(error) => self.sessions.allocation_error = error.to_string(),
                }
            }
            if ui.button("Close").clicked() {
                self.dialog = Dialog::None;
            }
        });
    }
}
