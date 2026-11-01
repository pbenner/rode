//! Responsive metric cards. Plot proportions are independent of panel width;
//! cards keep a bounded reading width with aligned rows at every window size.
use super::*;

const GAP: f32 = 16.;
const MIN_CARD_WIDTH: f32 = 360.;
const MAX_CARD_WIDTH: f32 = 560.;

#[derive(Clone, Copy, Hash)]
/// Stable metric kind; GPU indices preserve independence when several devices have identical names.
enum Metric {
    Cpu,
    Memory,
    Gpu(usize),
}

/// One-to-three-column geometry with minimum target and maximum reading widths in logical points.
struct GridLayout {
    columns: usize,
    card_width: f32,
    width: f32,
}

impl GridLayout {
    /// Choose the column count from available width and metric count, then cap each card
    /// without stretching a sparse dashboard across a wide detached window.
    fn new(available: f32, count: usize) -> Self {
        let columns = (((available + GAP) / (MIN_CARD_WIDTH + GAP)).floor() as usize)
            .clamp(1, count.clamp(1, 3));
        let card_width =
            ((available - GAP * (columns - 1) as f32) / columns as f32).clamp(1., MAX_CARD_WIDTH);
        Self {
            columns,
            card_width,
            width: card_width * columns as f32 + GAP * (columns - 1) as f32,
        }
    }
}

/// Render CPU, memory and available GPUs as top-aligned equal-height rows;
/// all curves come from cached history and rendering performs no telemetry collection.
pub(super) fn show(ui: &mut egui::Ui, state: &State, interval: Duration, palette: theme::Palette) {
    let (Some(sample), Some(latest)) = (&state.monitor, state.history.back()) else {
        return;
    };
    let dashboard = Dashboard {
        state,
        sample,
        latest,
        interval,
        palette,
        times: state.history.iter().map(|reading| reading.at).collect(),
    };
    let metrics: Vec<_> = [Metric::Cpu, Metric::Memory]
        .into_iter()
        .chain((0..sample.gpus.len()).map(Metric::Gpu))
        .collect();
    let layout = GridLayout::new(ui.available_width(), metrics.len());
    ui.allocate_ui_with_layout(
        egui::vec2(layout.width, 0.),
        Layout::top_down(Align::Min),
        |ui| {
            ui.set_width(layout.width);
            let card_spacing = ui.spacing().item_spacing;
            ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
            // Content determines row height; keep the cards anchored at the top.
            for row in metrics.chunks(layout.columns) {
                ui.horizontal_top(|ui| {
                    for &metric in row {
                        ui.allocate_ui_with_layout(
                            egui::vec2(layout.card_width, 0.),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.set_width(layout.card_width);
                                // Row spacing should not affect a card's contents.
                                ui.spacing_mut().item_spacing = card_spacing;
                                ui.push_id(metric, |ui| dashboard.metric(ui, metric));
                            },
                        );
                    }
                });
            }
        },
    );
}

/// Borrowed current sample/history and shared chart styling for a single render pass.
struct Dashboard<'a> {
    state: &'a State,
    sample: &'a project::Monitor,
    latest: &'a Reading,
    times: Vec<Instant>,
    interval: Duration,
    palette: theme::Palette,
}

impl Dashboard<'_> {
    /// Build the selected metric's curves and fixed-height heading/legend/footer around its proportional plot.
    fn metric(&self, ui: &mut egui::Ui, metric: Metric) {
        let palette = self.palette;
        let secondary = secondary_color(palette);
        card(ui, palette, |ui| {
            let (title, value, curves) = match metric {
                Metric::Cpu => (
                    "CPU",
                    self.latest.cpu,
                    vec![
                        Series {
                            name: "Overall",
                            color: palette.accent,
                            values: self.state.history.iter().map(|r| r.cpu).collect(),
                        },
                        Series {
                            name: "Busiest core",
                            color: secondary,
                            values: self.state.history.iter().map(|r| r.busiest).collect(),
                        },
                    ],
                ),
                Metric::Memory => (
                    "Memory",
                    self.latest.memory,
                    vec![Series {
                        name: "RAM",
                        color: palette.accent,
                        values: self.state.history.iter().map(|r| r.memory).collect(),
                    }],
                ),
                Metric::Gpu(index) => {
                    let gpu = &self.sample.gpus[index];
                    // Device order keeps identical models independent, as in collection.
                    let readings: Vec<_> = self
                        .state
                        .history
                        .iter()
                        .map(|r| r.gpus.get(index).filter(|g| g.name == gpu.name))
                        .collect();
                    (
                        gpu.name.as_str(),
                        gpu.usage.and_then(valid_percentage),
                        vec![
                            Series {
                                name: "GPU usage",
                                color: palette.accent,
                                values: readings.iter().map(|g| g.and_then(|g| g.usage)).collect(),
                            },
                            Series {
                                name: "VRAM",
                                color: secondary,
                                values: readings.iter().map(|g| g.and_then(|g| g.memory)).collect(),
                            },
                        ],
                    )
                }
            };
            heading(ui, title, value, palette);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 22.),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_min_height(22.);
                    legend(ui, &curves);
                },
            );
            chart(ui, &self.times, &curves, self.interval, palette);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 42.),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_min_height(42.);
                    ui.spacing_mut().item_spacing.y = 4.;
                    self.details(ui, metric);
                },
            );
        });
    }

    /// Render compact current-resource summaries, retaining complete values in hover text.
    fn details(&self, ui: &mut egui::Ui, metric: Metric) {
        let detail = |ui: &mut egui::Ui, text: String| {
            ui.add(
                egui::Label::new(RichText::new(&text).size(12.).color(self.palette.muted))
                    .truncate()
                    .show_tooltip_when_elided(false),
            )
            .on_hover_text(text);
        };
        match metric {
            Metric::Cpu => detail(
                ui,
                format!("{} logical cores", self.sample.cpus.len().saturating_sub(1)),
            ),
            Metric::Memory => {
                detail(
                    ui,
                    format!(
                        "RAM {} / {}",
                        bytes(
                            self.sample
                                .memory_total
                                .saturating_sub(self.sample.memory_available)
                        ),
                        bytes(self.sample.memory_total)
                    ),
                );
                if self.sample.swap_total > 0 {
                    detail(
                        ui,
                        format!(
                            "Swap {} / {}",
                            bytes(self.sample.swap_total.saturating_sub(self.sample.swap_free)),
                            bytes(self.sample.swap_total)
                        ),
                    );
                }
            }
            Metric::Gpu(index) => {
                let gpu = &self.sample.gpus[index];
                detail(
                    ui,
                    match gpu.memory_used.zip(gpu.memory_total) {
                        Some((used, total)) if total > 0 => {
                            format!("VRAM {} / {}", bytes(used), bytes(total))
                        }
                        _ => "VRAM unavailable".into(),
                    },
                );
                let temperature = gpu.temperature.filter(|v| v.is_finite()).map_or_else(
                    || "Temperature unavailable".into(),
                    |v| format!("{v:.0} °C"),
                );
                let power = gpu
                    .power
                    .filter(|v| v.is_finite())
                    .map_or_else(|| "Power unavailable".into(), |v| format!("{v:.1} W"));
                detail(ui, format!("{temperature} · {power}"));
            }
        }
    }
}
