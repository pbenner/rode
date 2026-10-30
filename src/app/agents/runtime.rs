//! Explicit desktop runtime setup. Rendering reads only cached worker state;
//! installation never starts a chat, changes credentials or widens project access.
use crate::{bundled::install, ssh};
use eframe::egui;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct Runtime {
    job: Option<install::Job>,
    retired: Vec<install::Job>,
    error: String,
}

impl Runtime {
    /// Retain cancelled workers until their temporary files and shared locks are released.
    pub fn reset(&mut self) {
        self.poll();
        if let Some(job) = self.job.take() {
            job.cancel();
            if matches!(job.state(), install::State::Working { .. }) {
                self.retired.push(job);
            }
        }
        self.error.clear();
    }

    pub fn poll(&mut self) {
        self.retired
            .retain(|job| matches!(job.state(), install::State::Working { .. }));
    }

    pub fn pending(&self) -> bool {
        self.installing()
            || self
                .retired
                .iter()
                .any(|job| matches!(job.state(), install::State::Working { .. }))
    }

    pub fn cancel_all(&self) {
        for job in self.job.iter().chain(&self.retired) {
            job.cancel();
        }
    }

    /// Prevent starting a conversation/history browser before setup finishes.
    pub fn installing(&self) -> bool {
        self.job
            .as_ref()
            .is_some_and(|job| matches!(job.state(), install::State::Working { .. }))
    }

    /// Keep setup progress/cancellation reachable after chat notices arrive.
    pub fn started(&self) -> bool {
        self.job.is_some() || !self.error.is_empty()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, remote: Option<&ssh::Remote>, enabled: bool) {
        let state = self.job.as_ref().map(install::Job::state);
        let working = matches!(state, Some(install::State::Working { .. }));
        ui.horizontal(|ui| {
            if ui.add_enabled(enabled && !working && self.retired.len() < 4, egui::Button::new("Install Codex").small())
                .on_hover_text(format!("Install pinned Codex {} for your user. No root, Python or npm required.\nLocation: {}\n{}",
                    install::version(), install::directory().map(|p| p.display().to_string()).unwrap_or_else(|| "Set HOME or XDG_DATA_HOME".into()),
                    if remote.is_some() {"Downloads on this computer for the SSH host's architecture. Chat deploys it over the existing SSH connection."}
                    else {"Downloads only when you click. Existing user runtime files are verified and reused."}))
                .on_disabled_hover_text("Use executable auto without RODE_CODEX_BUNDLE, and wait for the project/current turn before installing.")
                .clicked()
            {
                let ctx = ui.ctx().clone();
                match install::Job::start(remote.cloned(), Arc::new(move || ctx.request_repaint())) {
                    Ok(job) => {self.job = Some(job); self.error.clear();}
                    Err(error) => self.error = error.to_string(),
                }
            }
            if let Some(install::State::Working { .. }) = &state {
                let cancelling = self.job.as_ref().is_some_and(install::Job::cancelling);
                if ui.add_enabled(!cancelling, egui::Button::new(if cancelling {"Cancelling…"} else {"Cancel"}).small()).clicked()
                    && let Some(job) = &self.job {job.cancel();}
            } else if let Some(install::State::Finished(Ok(installed))) = &state {
                ui.add(egui::Label::new(format!("Codex ready · {}", installed.architecture)).truncate())
                    .on_hover_text(installed.directory.display().to_string());
            } else {
                ui.add(egui::Label::new(if self.retired.len() >= 4 {"Waiting for cancelled installers…"} else {"Optional · installs for your user"}).truncate());
            }
        });
        match state {
            Some(install::State::Working { label, done, total }) => {
                if total > 0 {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total as f32).text(format!(
                            "{label} {:.1} / {:.1} MiB",
                            done as f64 / 1_048_576.,
                            total as f64 / 1_048_576.
                        )),
                    );
                } else {
                    ui.add(egui::Label::new(label).wrap());
                }
            }
            Some(install::State::Finished(Err(error))) => {
                ui.add(egui::Label::new(error).wrap().selectable(true));
            }
            _ => {}
        }
        if !self.error.is_empty() {
            ui.add(egui::Label::new(&self.error).wrap().selectable(true));
        }
    }
}
