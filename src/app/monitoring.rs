//! Themed host charts with bounded, timestamped samples. Collection is explicitly started;
//! preserve gaps and unavailable readings instead of plotting invented zeroes.
use super::*;
use crate::app::layout::card;
use crate::project;
use layout::bytes;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

mod dashboard;
mod timeline;
pub(in crate::app) use timeline::View as ChartView;

// One sample per second for the longest selectable window plus its boundary.
const HISTORY_LIMIT: usize = timeline::MAX_SECONDS as usize + 2;

#[derive(Default)]
/// Bounded successful telemetry snapshots and derived rate baselines. Request timing
/// is separate from sample timing so failed polls do not corrupt CPU/network deltas.
pub(super) struct State {
    /// Transient user intent, independent of panel visibility and never restored on startup.
    pub(super) active: bool,
    pub(super) monitor: Option<project::Monitor>,
    pub(super) monitor_busy: bool,
    /// Last request completion for scheduling, including failures; not the rate baseline.
    pub(super) monitor_last: Option<Instant>,
    pub(super) monitor_error: String,
    cpu_usage: Vec<Option<f32>>,
    pub(super) history: VecDeque<Reading>,
    /// Received/sent bytes per second from two recent successful counter snapshots.
    network_rate: Option<(f32, f32)>,
    /// A reset keeps ownership of an in-flight poll but rejects its old result.
    discard_pending: bool,
    /// Freeze a stopped scrolling chart at the moment collection stopped.
    stopped_at: Option<Instant>,
    /// A resumed collector needs a fresh counter baseline, even after a short pause.
    rate_baseline: bool,
}

/// One monotonic-time chart sample; utilization values are percentages in 0–100
/// and None represents missing data, not measured zero activity.
pub(super) struct Reading {
    /// Successful collection time, used directly for chart spacing and rate deltas.
    at: Instant,
    cpu: Option<f32>,
    busiest: Option<f32>,
    /// Used physical RAM as a percentage, rather than a byte count.
    memory: Option<f32>,
    gpus: Vec<GpuReading>,
}

/// One device-order sample identified by its name; duplicate model names remain
/// independent through their device index.
struct GpuReading {
    name: String,
    usage: Option<f32>,
    memory: Option<f32>,
}

impl State {
    /// Start/stop scheduling without releasing an outstanding request's busy guard.
    /// A quick Stop/Start must drain and discard that old reply before collecting again.
    pub(super) fn set_active(&mut self, active: bool) {
        if self.active == active {
            return;
        }
        self.active = active;
        self.discard_pending |= self.monitor_busy;
        self.rate_baseline = false;
        self.stopped_at = (!active).then(Instant::now);
        if active {
            self.monitor_last = None;
            self.monitor_error.clear();
        }
    }

    /// Start fresh without releasing the busy guard for an outstanding request.
    /// Keep the user's Start/Stop choice; CPU/network counters need two new samples.
    pub(super) fn reset_history(&mut self) {
        self.monitor = None;
        self.history.clear();
        self.cpu_usage.clear();
        self.network_rate = None;
        self.rate_baseline = false;
        self.monitor_last = None;
        self.monitor_error.clear();
        self.discard_pending |= self.monitor_busy;
    }

    /// Consume the one old reply rejected by a reset; its completion must not
    /// postpone the fresh poll or repopulate the cleared chart with old data.
    pub(super) fn discard_reply(&mut self) -> bool {
        std::mem::take(&mut self.discard_pending)
    }

    /// Derive CPU percentages and network bytes/second from a recent successful sample,
    /// then append bounded history. Counter resets, zero deltas and long pauses produce
    /// unavailable rates instead of invented zeroes; callers provide increasing sample times.
    pub(super) fn record(&mut self, sample: project::Monitor, now: Instant, interval: Duration) {
        // Request scheduling also advances on errors. Rates must use the last successful sample.
        let elapsed = self.history.back().map(|r| now.duration_since(r.at));
        let previous = self.monitor.as_ref().filter(|_| {
            self.rate_baseline
                && elapsed.is_some_and(|elapsed| !elapsed.is_zero() && elapsed <= interval * 3)
        });
        self.cpu_usage = sample
            .cpus
            .iter()
            .map(|cpu| {
                let old = previous?.cpus.iter().find(|c| c.name == cpu.name)?;
                let total = cpu.total.checked_sub(old.total)?;
                let idle = cpu.idle.checked_sub(old.idle)?;
                (total > 0 && idle <= total).then(|| 100. * (total - idle) as f32 / total as f32)
            })
            .collect();
        self.network_rate = previous.and_then(|old| {
            let seconds = elapsed?.as_secs_f32();
            Some((
                sample.net_received.checked_sub(old.net_received)? as f32 / seconds,
                sample.net_sent.checked_sub(old.net_sent)? as f32 / seconds,
            ))
        });
        self.history.push_back(Reading {
            at: now,
            cpu: self.cpu_usage.first().copied().flatten(),
            busiest: self
                .cpu_usage
                .iter()
                .skip(1)
                .flatten()
                .copied()
                .reduce(f32::max),
            memory: percentage(
                sample.memory_total.saturating_sub(sample.memory_available),
                sample.memory_total,
            ),
            gpus: sample
                .gpus
                .iter()
                .map(|gpu| GpuReading {
                    name: gpu.name.clone(),
                    usage: gpu.usage.and_then(valid_percentage),
                    memory: gpu
                        .memory_used
                        .zip(gpu.memory_total)
                        .and_then(|(used, total)| percentage(used, total)),
                })
                .collect(),
        });
        while self.history.len() > HISTORY_LIMIT
            || self.history.len() > 1
                && now.duration_since(self.history[1].at)
                    > Duration::from_secs(timeline::MAX_SECONDS)
        {
            self.history.pop_front();
        }
        self.monitor = Some(sample);
        self.rate_baseline = true;
    }
}

/// Convert a byte/count ratio to a clamped percentage; a zero total means unavailable.
fn percentage(used: u64, total: u64) -> Option<f32> {
    (total > 0).then(|| (100. * used as f64 / total as f64).clamp(0., 100.) as f32)
}

/// Reject nonfinite measurements and clamp valid driver values to the chart range.
fn valid_percentage(value: f32) -> Option<f32> {
    value.is_finite().then(|| value.clamp(0., 100.))
}

/// Format a percentage or explicit unavailable text for both legends and current readings.
fn percent_label(value: Option<f32>) -> String {
    value.map_or_else(|| "Unavailable".into(), |v| format!("{v:.1}%"))
}

impl EditorApp {
    /// Schedule only an explicitly started collector, independently of project
    /// readiness or panel visibility. The busy guard admits one captured host poll.
    pub(super) fn monitor_tick(&mut self, ctx: &egui::Context) {
        if self.host_tools.monitoring.active {
            self.initialize_host_tool(Panel::Monitor);
        }
        if self.feature(crate::extensions::Feature::Monitor)
            && self.host_tools.monitoring.active
            && !self.exit_pending()
            && self.monitor_host_ready()
        {
            let interval = Duration::from_secs(self.areas.prefs.monitor_interval.clamp(1, 30));
            if !self.host_tools.monitoring.monitor_busy
                && self
                    .host_tools
                    .monitoring
                    .monitor_last
                    .is_none_or(|time| time.elapsed() >= interval)
            {
                self.host_tools.monitoring.monitor_busy = true;
                self.dispatch_host_monitor(ctx);
            }
            ctx.request_repaint_after(interval);
        }
    }

    /// Complete the captured host request after the host owner admits its generation.
    /// Project changes do not participate in this channel; Stop/Reset still drain old replies.
    pub(in crate::app) fn complete_host_monitor(
        &mut self,
        result: Result<project::Response, String>,
        ctx: &egui::Context,
    ) {
        self.host_tools.monitoring.monitor_busy = false;
        let discard = self.host_tools.monitoring.discard_reply();
        if !self.feature(crate::extensions::Feature::Monitor)
            || !self.host_tools.monitoring.active
            || discard
        {
            ctx.request_repaint();
            return;
        }
        self.host_tools.monitoring.monitor_error.clear();
        match result {
            Ok(project::Response::Monitor(sample)) => {
                self.host_tools.monitoring.record(
                    sample,
                    Instant::now(),
                    Duration::from_secs(self.areas.prefs.monitor_interval.clamp(1, 30)),
                );
            }
            Err(error) => self.host_tools.monitoring.monitor_error = error,
            _ => self.host_tools.monitoring.monitor_error = "Invalid host monitor response".into(),
        }
        self.host_tools.monitoring.monitor_last = Some(Instant::now());
    }

    /// Render cached host metadata, responsive chart cards and expandable details;
    /// collection is scheduled separately and does not depend on visibility.
    pub(in crate::app) fn monitor_ui(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        layout::header(
            ui,
            "Host overview",
            if self.host_tools.monitoring.active {
                "Running · collection continues while this panel is hidden"
            } else {
                "Stopped · start monitoring to collect host readings"
            },
            palette,
            |ui| {
                if self.host_tools.monitoring.monitor_busy {
                    ui.spinner();
                }
                let state = &mut self.host_tools.monitoring;
                let toggle = crate::keyboard::button(
                    ui,
                    if state.active {
                        "Stop monitoring"
                    } else {
                        "Start monitoring"
                    },
                );
                ui_probe::button(
                    ui,
                    if state.active {
                        "monitor-stop"
                    } else {
                        "monitor-start"
                    },
                    &toggle,
                );
                if toggle.clicked() {
                    state.set_active(!state.active);
                    ui.ctx().request_repaint();
                }
                ui_probe::button(
                    ui,
                    if state.active {
                        "monitor-running"
                    } else {
                        "monitor-stopped"
                    },
                    &toggle,
                );
                let reset = ui
                    .add_enabled(
                        !state.history.is_empty() || !state.monitor_error.is_empty(),
                        crate::keyboard::Button::new("Reset history"),
                    )
                    .on_hover_text("Clear past readings; keep monitoring started or stopped.");
                ui_probe::button(ui, "monitor-reset-history", &reset);
                ui_probe::value(
                    ui,
                    "monitor-history-count",
                    &reset,
                    &state.history.len().to_string(),
                );
                if reset.clicked() {
                    state.reset_history();
                    ui.ctx().request_repaint();
                }
            },
        );
        let mut view = self.areas.prefs.monitor_chart;
        view.controls(ui);
        if view != self.areas.prefs.monitor_chart {
            self.areas.prefs.monitor_chart = view;
            self.areas.pending = true;
        }
        let state = &self.host_tools.monitoring;
        if !state.monitor_error.is_empty() {
            ui.colored_label(palette.error, &state.monitor_error);
        }
        let (Some(sample), Some(latest)) = (&state.monitor, state.history.back()) else {
            ui.label(if state.active {
                "Acquiring host telemetry…"
            } else {
                "Press Start monitoring to collect readings."
            });
            return;
        };
        let hostname = ui
            .add(egui::Label::new(RichText::new(&sample.hostname).strong()).truncate())
            .on_hover_text(&sample.hostname);
        ui_probe::button(ui, "monitor-sample", &hostname);
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Uptime {}d {}h",
                sample.uptime as u64 / 86400,
                sample.uptime as u64 / 3600 % 24
            ));
            ui.label(format!(
                "Load {}",
                sample
                    .load
                    .iter()
                    .map(|v| format!("{v:.2}"))
                    .collect::<Vec<_>>()
                    .join(" / ")
            ));
            ui.label(format!(
                "Updated {} ago",
                duration_label(latest.at.elapsed().as_secs_f32())
            ));
        });
        let interval = Duration::from_secs(self.areas.prefs.monitor_interval.clamp(1, 30));
        ui.add_space(12.);
        dashboard::show(ui, state, interval, view, palette);
        if state.active && view.mode == timeline::Mode::Scrolling {
            // Animate cached samples only. Host collection remains interval-based
            // and continues while hidden; animation runs only for a visible panel.
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if sample.gpus.is_empty() || !sample.gpu_note.is_empty() {
            ui.add_space(12.);
            card(ui, palette, |ui| {
                if sample.gpus.is_empty() {
                    ui.label("No GPU telemetry available on this host.");
                }
                if !sample.gpu_note.is_empty() {
                    ui.label(&sample.gpu_note);
                }
            });
        }
        ui.add_space(12.);
        card(ui, palette, |ui| {
            ui.label(RichText::new("Network & storage").size(15.));
            ui.label(state.network_rate.map_or_else(
                || "Network · sampling…".into(),
                |(received, sent)| {
                    format!(
                        "Network ↓ {}/s    ↑ {}/s",
                        bytes(received as u64),
                        bytes(sent as u64)
                    )
                },
            ));
            if sample.disk_total > 0 {
                ui.add(
                    egui::ProgressBar::new(1. - sample.disk_free as f32 / sample.disk_total as f32)
                        .fill(palette.accent)
                        .text(format!(
                            "{} disk · {} free / {}",
                            "Host",
                            bytes(sample.disk_free),
                            bytes(sample.disk_total)
                        )),
                );
            }
        });
        crate::keyboard::collapsing(ui, "Per-core activity", |ui| {
            let columns = if ui.available_width() >= 650. {
                4
            } else if ui.available_width() >= 330. {
                2
            } else {
                1
            };
            let width = (ui.available_width() - (columns - 1) as f32 * 8.) / columns as f32;
            egui::Grid::new("core-meters")
                .num_columns(columns)
                .spacing([8., 6.])
                .show(ui, |ui| {
                    for (index, value) in state.cpu_usage.iter().skip(1).enumerate() {
                        ui.add(
                            egui::ProgressBar::new(value.unwrap_or(0.) / 100.)
                                .fill(palette.accent)
                                .desired_width(width)
                                .text(format!("CPU {index} · {}", percent_label(*value))),
                        );
                        if (index + 1) % columns == 0 {
                            ui.end_row();
                        }
                    }
                });
        });
        crate::keyboard::collapsing(ui, "Processes · CPU lifetime average", |ui| {
            let command_width = (ui.available_width() - 165.).max(30.);
            egui::Grid::new("process-table")
                .striped(true)
                .show(ui, |ui| {
                    for value in ["PID", "CPU %", "RAM %", "Command"] {
                        ui.label(value);
                    }
                    ui.end_row();
                    for process in &sample.processes {
                        ui.monospace(&process.pid);
                        ui.monospace(&process.cpu);
                        ui.monospace(&process.memory);
                        ui.add_sized(
                            [command_width, 18.],
                            egui::Label::new(&process.name).truncate(),
                        )
                        .on_hover_text(&process.name);
                        ui.end_row();
                    }
                });
        });
    }
}

/// Reserve equal-height title/value regions so long device names and unavailable
/// measurements cannot shift chart baselines across a row.
fn heading(ui: &mut egui::Ui, title: &str, value: Option<f32>, palette: theme::Palette) {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 128.).max(20.);
        ui.allocate_ui_with_layout(
            egui::vec2(width, 34.),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_width(width);
                ui.set_min_height(34.);
                ui.add(
                    egui::Label::new(RichText::new(title).size(15.).color(palette.text))
                        .truncate()
                        .show_tooltip_when_elided(false),
                )
                .on_hover_text(title);
            },
        );
        ui.allocate_ui_with_layout(
            egui::vec2(120., 34.),
            Layout::right_to_left(Align::Center),
            |ui| {
                ui.set_min_height(34.);
                ui.label(
                    RichText::new(percent_label(value))
                        .size(if value.is_some() { 26. } else { 16. })
                        .color(palette.text),
                );
            },
        );
    });
}

/// Named percentage samples aligned positionally with the chart timestamps;
/// None breaks a curve rather than interpolating an unavailable reading.
struct Series<'a> {
    name: &'a str,
    color: egui::Color32,
    values: Vec<Option<f32>>,
}

// Pick a distinct secondary series from the theme's own cool colours.
fn secondary_color(palette: theme::Palette) -> egui::Color32 {
    [palette.ansi[4], palette.ansi[5], palette.ansi[6]]
        .into_iter()
        .max_by_key(|color| {
            color.to_array()[..3]
                .iter()
                .zip(palette.accent.to_array())
                .map(|(&a, b)| (a as i32 - b as i32).pow(2))
                .sum::<i32>()
        })
        .unwrap()
}

/// Render equal-width series labels with full tooltips when compact columns elide text.
fn legend(ui: &mut egui::Ui, series: &[Series<'_>]) {
    let count = series.len().max(1);
    let width =
        (ui.available_width() - (count - 1) as f32 * ui.spacing().item_spacing.x) / count as f32;
    ui.horizontal(|ui| {
        for series in series {
            ui.allocate_ui_with_layout(
                egui::vec2(width, 20.),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_min_width(width);
                    ui.set_min_height(20.);
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(16., 14.), Sense::hover());
                    ui.painter().line_segment(
                        [rect.left_center(), rect.right_center()],
                        Stroke::new(2_f32, series.color),
                    );
                    let label = format!(
                        "{} {}",
                        series.name,
                        percent_label(series.values.last().copied().flatten())
                    );
                    ui.add(
                        egui::Label::new(RichText::new(&label).size(12.))
                            .truncate()
                            .show_tooltip_when_elided(false),
                    )
                    .on_hover_text(label);
                },
            );
        }
    });
}

/// Format elapsed seconds into compact seconds/minutes/hours labels.
fn duration_label(seconds: f32) -> String {
    let seconds = seconds.round() as u64;
    if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, seconds / 60 % 60)
    } else if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

// Return separate segments so unavailable telemetry and pauses remain visible gaps.
fn segments(times: &[Instant], values: &[Option<f32>], gap: Duration) -> Vec<Vec<(Instant, f32)>> {
    let mut result: Vec<Vec<_>> = vec![];
    let mut previous = None;
    for (&time, &value) in times.iter().zip(values) {
        if let Some(value) = value.and_then(valid_percentage) {
            if previous.is_none_or(|last| time.duration_since(last) > gap) {
                result.push(vec![]);
            }
            result.last_mut().unwrap().push((time, value));
            previous = Some(time);
        } else {
            previous = None;
        }
    }
    result
}

/// Paint a 16:9 percentage plot with fixed axis gutters and real timestamp spacing.
/// Missing values and long sampling gaps split curves; hovering inspects the nearest sample.
fn chart(
    ui: &mut egui::Ui,
    times: &[Instant],
    series: &[Series<'_>],
    interval: Duration,
    axis: timeline::Axis,
    palette: theme::Palette,
) {
    // The plot itself is 16:9; reserve the same axis gutters at every width.
    let width = ui.available_width();
    let plot_height = (width - 50.).max(1.) * 9. / 16.;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, plot_height + 32.), Sense::hover());
    let plot = egui::Rect::from_min_max(
        rect.min + egui::vec2(42., 8.),
        rect.max - egui::vec2(8., 24.),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(plot, 0., palette.background);
    let font = egui::FontId::proportional(11.);
    for tick in 0..=4 {
        let y = plot.bottom() - plot.height() * tick as f32 / 4.;
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            Stroke::new(1_f32, palette.border),
        );
        painter.text(
            egui::pos2(plot.left() - 7., y),
            egui::Align2::RIGHT_CENTER,
            format!("{}%", tick * 25),
            font.clone(),
            palette.text,
        );
    }
    let span = axis.span.as_secs_f32();
    let position = |time: Instant, value: f32| {
        egui::pos2(
            plot.left() + plot.width() * axis.fraction(time),
            plot.bottom() - plot.height() * value / 100.,
        )
    };
    for (index, series) in series.iter().enumerate() {
        let plot_painter = painter.with_clip_rect(plot);
        for segment in segments(times, &series.values, interval * 3) {
            let points: Vec<_> = segment
                .into_iter()
                .map(|(time, value)| position(time, value))
                .collect();
            if index == 0 {
                // Fill individual trapezoids; a concave area polygon cannot be passed to convex_polygon.
                for pair in points.windows(2) {
                    plot_painter.add(egui::Shape::convex_polygon(
                        vec![
                            egui::pos2(pair[0].x, plot.bottom()),
                            pair[0],
                            pair[1],
                            egui::pos2(pair[1].x, plot.bottom()),
                        ],
                        series.color.gamma_multiply(0.20),
                        Stroke::NONE,
                    ));
                }
            }
            if points.len() == 1 {
                plot_painter.circle_filled(points[0], 2.5, series.color);
            } else {
                plot_painter.add(egui::Shape::line(
                    points,
                    Stroke::new(if index == 0 { 2_f32 } else { 1.3_f32 }, series.color),
                ));
            }
        }
    }
    for (fraction, align) in [
        (0., egui::Align2::LEFT_TOP),
        (0.5, egui::Align2::CENTER_TOP),
        (1., egui::Align2::RIGHT_TOP),
    ] {
        if fraction == 0.5 && plot.width() < 260. {
            continue;
        }
        let x = plot.left() + plot.width() * fraction;
        let label = if fraction == 1. {
            if axis.live {
                "Now".into()
            } else {
                "Latest".into()
            }
        } else {
            format!("−{}", duration_label(span * (1. - fraction)))
        };
        painter.text(
            egui::pos2(x, plot.bottom() + 7.),
            align,
            label,
            font.clone(),
            palette.text,
        );
    }
    if !series.iter().any(|s| {
        times
            .iter()
            .zip(&s.values)
            .any(|(&time, value)| axis.contains(time) && value.is_some())
    }) {
        painter.text(
            plot.center(),
            egui::Align2::CENTER_CENTER,
            "Waiting for readings",
            egui::FontId::proportional(13.),
            palette.text,
        );
    }
    if let Some(pointer) = response.hover_pos().filter(|p| plot.contains(*p)) {
        let nearest = times
            .iter()
            .enumerate()
            .filter(|(_, time)| axis.contains(**time))
            .min_by(|(_, a), (_, b)| {
                (position(**a, 0.).x - pointer.x)
                    .abs()
                    .total_cmp(&(position(**b, 0.).x - pointer.x).abs())
            })
            .map(|(i, _)| i);
        let Some(index) = nearest else {
            return;
        };
        let first_x = times
            .iter()
            .find(|&&time| axis.contains(time))
            .map(|&time| position(time, 0.).x)
            .unwrap();
        let last_x = times.last().map(|&time| position(time, 0.).x).unwrap();
        // Empty startup history and time since the last poll are genuinely
        // blank: hovering them must not imply that a reading was collected.
        if pointer.x < first_x - 6. || pointer.x > last_x + 6. {
            return;
        }
        let x = position(times[index], 0.).x;
        painter.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            Stroke::new(1_f32, palette.muted),
        );
        response.on_hover_ui(|ui| {
            ui.label(format!(
                "{} {}",
                duration_label(axis.end.duration_since(times[index]).as_secs_f32()),
                if axis.live {
                    "ago"
                } else {
                    "before latest sample"
                }
            ));
            for series in series {
                ui.label(format!(
                    "{}: {}",
                    series.name,
                    percent_label(series.values[index])
                ));
            }
        });
    }
}

#[cfg(test)]
mod tests;
