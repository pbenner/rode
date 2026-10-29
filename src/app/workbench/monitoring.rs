//! Bundled monitoring feature implementation.
//!
//! Schedule bounded host snapshots only while this panel is visible and enabled. Compute
//! utilization/rates from consecutive counter samples; clear history on host changes and keep
//! unavailable measurements distinct from zero.
use super::*;

pub(super) struct State {
    pub(super) monitor: Option<project::Monitor>,
    pub(super) monitor_busy: bool,
    pub(super) monitor_last: Option<Instant>,
    pub(super) monitor_error: String,
    pub(super) cpu_usage: Vec<f32>,
    pub(super) cpu_history: VecDeque<f32>,
    pub(super) memory_history: VecDeque<f32>,
    pub(super) gpu_history: HashMap<String, VecDeque<f32>>,
    pub(super) network_rate: (f32, f32),
}
impl Default for State {
    fn default() -> Self {
        Self {
            monitor: None,
            monitor_busy: false,
            monitor_last: None,
            monitor_error: String::new(),
            cpu_usage: vec![],
            cpu_history: VecDeque::new(),
            memory_history: VecDeque::new(),
            gpu_history: HashMap::new(),
            network_rate: (0., 0.),
        }
    }
}

impl EditorApp {
    pub(in crate::app) fn monitor_ui(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("[ SYS // LIVE ]")
                    .monospace()
                    .color(palette.accent)
                    .strong(),
            );
            if self.workbench.monitoring.monitor_busy {
                ui.spinner();
            }
        });
        ui.label(
            RichText::new("Host telemetry · sampled only while this panel is visible")
                .small()
                .color(palette.muted),
        );
        if self.workspace.as_ref().is_some_and(|w| w.plan.active) {
            ui.label(
                RichText::new("Read outside the project sandbox to show the host and GPUs.")
                    .small()
                    .color(palette.muted),
            );
        }
        if !self.workbench.monitoring.monitor_error.is_empty() {
            ui.colored_label(palette.error, &self.workbench.monitoring.monitor_error);
        }
        let Some(sample) = &self.workbench.monitoring.monitor else {
            ui.label("Acquiring host telemetry…");
            return;
        };
        ui.add_space(8.);
        ui.label(RichText::new(&sample.hostname).monospace().strong());
        ui.label(palette.tag(format!(
                "UPTIME {:02}d {:02}h · LOAD {}",
                sample.uptime as u64 / 86400,
                sample.uptime as u64 / 3600 % 24,
                sample
                    .load
                    .iter()
                    .map(|v| format!("{v:.2}"))
                    .collect::<Vec<_>>()
                    .join(" / ")
            )));
        let cpu = self
            .workbench
            .monitoring
            .cpu_usage
            .first()
            .copied()
            .unwrap_or(0.);
        metric(
            ui,
            "CPU",
            cpu,
            if self.workbench.monitoring.cpu_history.is_empty() {
                "Sampling…".into()
            } else {
                format!(
                    "{cpu:.1}% · {} threads",
                    sample.cpus.len().saturating_sub(1)
                )
            },
            &self.workbench.monitoring.cpu_history,
            palette,
        );
        ui.collapsing("CPU cores", |ui| {
            egui::Grid::new("core-meters")
                .num_columns(2)
                .spacing([12., 5.])
                .show(ui, |ui| {
                    for (index, value) in self
                        .workbench
                        .monitoring
                        .cpu_usage
                        .iter()
                        .skip(1)
                        .enumerate()
                    {
                        ui.add(
                            egui::ProgressBar::new(*value / 100.)
                                .text(format!("CPU {index:02}  {value:4.1}%"))
                                .desired_width((ui.available_width() / 2.).max(105.)),
                        );
                        if index % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
        });
        let memory_used = sample.memory_total.saturating_sub(sample.memory_available);
        metric(
            ui,
            "RAM",
            100. * memory_used as f32 / sample.memory_total.max(1) as f32,
            format!("{} / {}", bytes(memory_used), bytes(sample.memory_total)),
            &self.workbench.monitoring.memory_history,
            palette,
        );
        if sample.swap_total > 0 {
            ui.label(palette.tag(format!(
                "SWAP  {} / {}",
                bytes(sample.swap_total.saturating_sub(sample.swap_free)),
                bytes(sample.swap_total)
            )));
        }
        ui.separator();
        ui.label(
            RichText::new("[ GPU // COMPUTE ]")
                .monospace()
                .strong()
                .color(palette.accent),
        );
        if sample.gpus.is_empty() {
            ui.label("No GPU telemetry available on this host.");
        }
        if !sample.gpu_note.is_empty() {
            ui.label(RichText::new(&sample.gpu_note).small().color(palette.muted));
        }
        for gpu in &sample.gpus {
            let history = self
                .workbench
                .monitoring
                .gpu_history
                .get(&gpu.name)
                .cloned()
                .unwrap_or_default();
            metric(
                ui,
                &gpu.name,
                gpu.usage.unwrap_or(0.),
                gpu.usage
                    .map(|p| format!("{p:.1}% GPU"))
                    .unwrap_or_else(|| "Usage not exposed by driver".into()),
                &history,
                palette,
            );
            if let (Some(used), Some(total)) = (gpu.memory_used, gpu.memory_total) {
                ui.add(
                    egui::ProgressBar::new(used as f32 / total.max(1) as f32).text(format!(
                        "VRAM  {} / {}",
                        bytes(used),
                        bytes(total)
                    )),
                );
            }
            ui.label(palette.tag(format!(
                    "TEMP {}    POWER {}",
                    gpu.temperature
                        .map(|v| format!("{v:.0} °C"))
                        .unwrap_or_else(|| "n/a".into()),
                    gpu.power
                        .map(|v| format!("{v:.1} W"))
                        .unwrap_or_else(|| "n/a".into())
                )));
        }
        ui.separator();
        ui.label(
            RichText::new("[ I/O // HOST ]")
                .monospace()
                .color(palette.accent),
        );
        ui.label(palette.tag(format!(
            "NET ↓ {}/s    ↑ {}/s",
            bytes(self.workbench.monitoring.network_rate.0 as u64),
            bytes(self.workbench.monitoring.network_rate.1 as u64)
        )));
        if sample.disk_total > 0 {
            ui.add(
                egui::ProgressBar::new(1. - sample.disk_free as f32 / sample.disk_total as f32)
                    .text(format!(
                        "{} DISK  {} free / {}",
                        if self.project_ready() {
                            "PROJECT"
                        } else {
                            "HOST"
                        },
                        bytes(sample.disk_free),
                        bytes(sample.disk_total)
                    )),
            );
        }
        ui.add_space(10.);
        ui.label(palette.tag("PROCESSES · CPU lifetime average"));
        egui::Grid::new("process-table")
            .striped(true)
            .show(ui, |ui| {
                for value in ["PID", "CPU%", "MEM%", "COMMAND"] {
                    ui.label(palette.tag(value));
                }
                ui.end_row();
                for process in &sample.processes {
                    ui.monospace(&process.pid);
                    ui.monospace(&process.cpu);
                    ui.monospace(&process.memory);
                    ui.label(&process.name);
                    ui.end_row();
                }
            });
    }
}
