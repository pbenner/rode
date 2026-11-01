//! Explicit desktop runtime setup. Rendering reads only cached worker state;
//! installation never starts a chat, changes credentials or widens project access.
use crate::{bundled::install, ssh};
use eframe::egui;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
/// Explicit runtime setup and cached availability probe; cancelled workers remain
/// owned until temporary files and cross-window installation locks are released.
pub(super) struct Runtime {
    job: Option<install::Job>,
    retired: Vec<install::Job>,
    error: String,
    available: Option<bool>,
    checked: Option<Instant>,
    probe: Option<(mpsc::Receiver<bool>, Arc<AtomicBool>)>,
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
        if let Some((_, cancel)) = self.probe.take() {
            cancel.store(true, Ordering::Release);
        }
        self.available = None;
        self.checked = None;
    }

    /// Release retired setup jobs only after they leave their working state.
    pub fn poll(&mut self) {
        self.retired
            .retain(|job| matches!(job.state(), install::State::Working { .. }));
    }

    /// Include both current and cancelled-but-retiring installation jobs in the exit guard.
    pub fn pending(&self) -> bool {
        self.installing()
            || self
                .retired
                .iter()
                .any(|job| matches!(job.state(), install::State::Working { .. }))
    }

    /// Signal setup/probe cancellation without waiting on their worker threads.
    pub fn cancel_all(&self) {
        if let Some((_, cancel)) = &self.probe {
            cancel.store(true, Ordering::Release);
        }
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

    /// Cache host-specific discovery; hashing bundles and SSH stay off the UI thread.
    pub fn check(&mut self, ui: &egui::Ui, remote: Option<&ssh::Remote>) -> Option<bool> {
        if let Some((rx, _)) = &self.probe {
            match rx.try_recv() {
                Ok(available) => {
                    self.available = Some(available);
                    self.checked = Some(Instant::now());
                    self.probe = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.available = Some(false);
                    self.checked = Some(Instant::now());
                    self.probe = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self
            .job
            .as_ref()
            .is_some_and(|job| matches!(job.state(), install::State::Finished(Ok(_))))
        {
            self.available = Some(true);
            self.checked = Some(Instant::now());
            self.job = None;
        }
        if self.probe.is_none()
            && !self.installing()
            && self
                .checked
                .is_none_or(|time| time.elapsed() >= Duration::from_secs(30))
        {
            let (tx, rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            self.probe = Some((rx, cancel.clone()));
            let remote = remote.cloned();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let available =
                    crate::bundled::available(remote.as_ref(), &cancel).unwrap_or(false);
                let _ = tx.send(available);
                ctx.request_repaint();
            });
        }
        ui.ctx().request_repaint_after(Duration::from_secs(30));
        self.available
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        remote: Option<&ssh::Remote>,
        enabled: bool,
        available: Option<bool>,
    ) {
        let state = self.job.as_ref().map(install::Job::state);
        let working = matches!(state, Some(install::State::Working { .. }));
        if available == Some(true) && !working {
            return;
        }
        if available.is_none() && !working && !self.started() {
            ui.label("Checking Codex availability…");
            return;
        }
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
                    Ok(job) => {
                        if let Some((_, cancel)) = self.probe.take() { cancel.store(true, Ordering::Release); }
                        self.job = Some(job); self.error.clear(); self.available = None; self.checked = None;
                    }
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

impl Drop for Runtime {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_is_hidden_when_codex_is_available() {
        for available in [false, true] {
            let ctx = egui::Context::default();
            let mut runtime = Runtime::default();
            let mut height = 0.;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    height = ui
                        .scope(|ui| runtime.ui(ui, None, true, Some(available)))
                        .response
                        .rect
                        .height();
                });
            });
            assert_eq!(height > 0., !available);
        }
    }

    #[test]
    fn reset_discards_previous_host_probe_results() {
        let mut runtime = Runtime::default();
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        runtime.probe = Some((rx, cancel.clone()));
        runtime.available = Some(true);
        tx.send(true).unwrap();
        runtime.reset();
        assert!(cancel.load(Ordering::Acquire));
        assert!(runtime.probe.is_none());
        assert!(runtime.available.is_none());
    }
}
