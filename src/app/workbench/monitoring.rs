//! Themed host charts with bounded, timestamped samples. Poll only while visible;
//! preserve gaps and unavailable readings instead of plotting invented zeroes.
use super::*;
use std::collections::VecDeque;

mod dashboard;

const HISTORY_LIMIT: usize = 120;

#[derive(Default)]
/// Bounded successful telemetry snapshots and derived rate baselines. Request timing
/// is separate from sample timing so failed polls do not corrupt CPU/network deltas.
pub(super) struct State {
    pub(super) monitor: Option<project::Monitor>,
    pub(super) monitor_busy: bool,
    /// Last request completion for scheduling, including failures; not the rate baseline.
    pub(super) monitor_last: Option<Instant>,
    pub(super) monitor_error: String,
    cpu_usage: Vec<Option<f32>>,
    pub(super) history: VecDeque<Reading>,
    /// Received/sent bytes per second from two recent successful counter snapshots.
    network_rate: Option<(f32, f32)>,
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
    /// Derive CPU percentages and network bytes/second from a recent successful sample,
    /// then append bounded history. Counter resets, zero deltas and long pauses produce
    /// unavailable rates instead of invented zeroes; callers provide increasing sample times.
    pub(super) fn record(&mut self, sample: project::Monitor, now: Instant, interval: Duration) {
        // Request scheduling also advances on errors. Rates must use the last successful sample.
        let elapsed = self.history.back().map(|r| now.duration_since(r.at));
        let previous = self.monitor.as_ref().filter(|_| {
            elapsed.is_some_and(|elapsed| !elapsed.is_zero() && elapsed <= interval * 3)
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
        if self.history.len() > HISTORY_LIMIT {
            self.history.pop_front();
        }
        self.monitor = Some(sample);
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
    /// Render cached host metadata, responsive chart cards and expandable details;
    /// sampling is scheduled separately by the visible-workbench tick.
    pub(in crate::app) fn monitor_ui(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        let state = &self.workbench.monitoring;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Host overview").size(18.).color(palette.text));
            if state.monitor_busy {
                ui.spinner();
            }
        });
        ui.label(
            RichText::new("Live telemetry · sampled while this panel is visible")
                .color(palette.muted),
        );
        if self.workspace.as_ref().is_some_and(|w| w.plan.active) {
            ui.label("Host and GPU readings are collected outside the project sandbox.");
        }
        if !state.monitor_error.is_empty() {
            ui.colored_label(palette.error, &state.monitor_error);
        }
        let (Some(sample), Some(latest)) = (&state.monitor, state.history.back()) else {
            ui.label("Acquiring host telemetry…");
            return;
        };
        ui.add(egui::Label::new(RichText::new(&sample.hostname).strong()).truncate())
            .on_hover_text(&sample.hostname);
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
        let interval = Duration::from_secs(self.workbench.prefs.monitor_interval.clamp(1, 30));
        ui.add_space(12.);
        dashboard::show(ui, state, interval, palette);
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
                            if self.project_ready() {
                                "Project"
                            } else {
                                "Host"
                            },
                            bytes(sample.disk_free),
                            bytes(sample.disk_total)
                        )),
                );
            }
        });
        ui.collapsing("Per-core activity", |ui| {
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
        ui.collapsing("Processes · CPU lifetime average", |ui| {
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

/// Frame content within the available column width; the caller owns spacing between cards.
fn card(ui: &mut egui::Ui, palette: theme::Palette, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(1_f32, palette.border))
        .corner_radius(6.)
        .inner_margin(12.)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            contents(ui);
        });
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
    let (Some(&start), Some(&end)) = (times.first(), times.last()) else {
        return;
    };
    let span = end
        .duration_since(start)
        .as_secs_f32()
        .max(interval.as_secs_f32());
    let position = |time: Instant, value: f32| {
        egui::pos2(
            plot.right() - plot.width() * end.duration_since(time).as_secs_f32() / span,
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
            "Latest".into()
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
    if !series.iter().any(|s| s.values.iter().any(Option::is_some)) {
        painter.text(
            plot.center(),
            egui::Align2::CENTER_CENTER,
            "Waiting for readings",
            egui::FontId::proportional(13.),
            palette.text,
        );
    }
    if let Some(pointer) = response.hover_pos().filter(|p| plot.contains(*p)) {
        let index = times
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (position(**a, 0.).x - pointer.x)
                    .abs()
                    .total_cmp(&(position(**b, 0.).x - pointer.x).abs())
            })
            .map(|(i, _)| i)
            .unwrap();
        let x = position(times[index], 0.).x;
        painter.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            Stroke::new(1_f32, palette.muted),
        );
        response.on_hover_ui(|ui| {
            ui.label(format!(
                "{} before latest sample",
                duration_label(end.duration_since(times[index]).as_secs_f32())
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
mod tests {
    use super::*;

    #[test]
    fn dashboard_reflows_without_stretching_plots_or_misaligning_cards() {
        for gpu_count in [0, 4] {
            let mut state = State::default();
            let mut reading = sample(100);
            reading.gpus = (0..gpu_count)
                .map(|index| project::Gpu {
                    name: format!("GPU {index}: a long descriptive device name"),
                    usage: (index != 1).then_some(60.),
                    memory_used: Some(200),
                    memory_total: Some(1000),
                    ..Default::default()
                })
                .collect();
            let interval = Duration::from_secs(2);
            let now = Instant::now() - interval;
            state.record(reading, now, interval);
            let mut next = state.monitor.clone().unwrap();
            next.cpus = sample(if gpu_count == 0 { 100 } else { 200 }).cpus;
            state.record(next, now + interval, interval);
            let palette = theme::Palette::default();
            for (width, columns) in [(320., 1), (780., 2), (1200., 3), (2200., 3)] {
                let ctx = egui::Context::default();
                theme::install(&ctx, palette);
                let origin = std::cell::Cell::new(egui::Pos2::ZERO);
                let frame = || {
                    ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 2600.),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                let bounds = ui.max_rect();
                                origin.set(ui.next_widget_position());
                                dashboard::show(ui, &state, interval, palette);
                                assert!(
                                    ui.min_rect().right() <= bounds.right() + 1.,
                                    "Dashboard overflow at {width}"
                                );
                            });
                        },
                    )
                };
                frame();
                let output = frame();
                let plots: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect)
                            if rect.fill == palette.background && rect.rect.height() < 600. =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(plots.len(), gpu_count + 2, "Missing plots at {width}");
                for plot in &plots {
                    assert!(
                        (plot.aspect_ratio() - 16. / 9.).abs() < 0.01,
                        "Stretched plot at {width}: {plot:?}"
                    );
                    assert!(plot.width() <= 560.);
                }
                let mut starts: Vec<f32> = vec![];
                for plot in &plots {
                    if !starts.iter().any(|x| (x - plot.left()).abs() < 1.) {
                        starts.push(plot.left());
                    }
                }
                let columns = columns.min(gpu_count + 2);
                assert_eq!(
                    starts.len(),
                    columns,
                    "Wrong column count at {width}: {starts:?}"
                );
                for row in plots.chunks(columns) {
                    assert!(
                        row.iter()
                            .all(|plot| (plot.top() - row[0].top()).abs() < 1.),
                        "Charts in the same row are misaligned at {width}: {row:?}"
                    );
                }
                let cards: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect) if rect.fill == palette.panel => Some(rect.rect),
                        _ => None,
                    })
                    .collect();
                assert_eq!(cards.len(), gpu_count + 2);
                assert!(
                    (cards[0].top() - origin.get().y).abs() < 1.,
                    "Gap above the dashboard at {width}: {:?}",
                    cards[0]
                );
                for row in cards.chunks(columns) {
                    assert!(
                        row.iter()
                            .all(|card| (card.height() - row[0].height()).abs() < 1.),
                        "Cards in the same row have uneven heights at {width}: {row:?}"
                    );
                }
            }
        }
    }

    fn sample(ticks: u64) -> project::Monitor {
        project::Monitor {
            cpus: vec![
                project::Cpu {
                    name: "cpu".into(),
                    total: ticks,
                    idle: ticks / 2,
                },
                project::Cpu {
                    name: "cpu0".into(),
                    total: ticks,
                    idle: ticks / 4,
                },
            ],
            memory_total: 1000,
            memory_available: 250,
            net_received: ticks * 10,
            net_sent: ticks * 2,
            ..Default::default()
        }
    }

    #[test]
    fn rates_need_recent_successful_samples_and_history_is_bounded() {
        let mut state = State::default();
        let now = Instant::now();
        let interval = Duration::from_secs(2);
        state.record(sample(100), now, interval);
        assert!(state.history[0].cpu.is_none());
        assert!(state.network_rate.is_none());
        assert_eq!(state.history[0].memory, Some(75.));
        // An intervening failed request updated the scheduler, not the counter timestamp.
        state.monitor_last = Some(now + Duration::from_secs(1));
        state.record(sample(200), now + interval, interval);
        assert_eq!(state.history[1].cpu, Some(50.));
        assert_eq!(state.history[1].busiest, Some(75.));
        assert_eq!(state.network_rate, Some((500., 100.)));
        // After a long pause, establish a fresh baseline rather than averaging over the gap.
        state.record(sample(300), now + interval * 10, interval);
        assert!(state.history[2].cpu.is_none());
        assert!(state.network_rate.is_none());
        for index in 11..150 {
            state.record(sample(index as u64 * 100), now + interval * index, interval);
        }
        assert_eq!(state.history.len(), HISTORY_LIMIT);
        assert_eq!(state.history.back().unwrap().at, now + interval * 149);
        // Host counter resets and zero deltas do not create artificial idle readings.
        state.record(sample(1), now + interval * 150, interval);
        assert!(state.history.back().unwrap().cpu.is_none());
        assert!(state.network_rate.is_none());
        state.record(sample(1), now + interval * 151, interval);
        assert!(state.history.back().unwrap().cpu.is_none());
    }

    #[test]
    fn identical_gpus_and_missing_measurements_stay_independent() {
        let mut state = State::default();
        let now = Instant::now();
        let mut sample = sample(100);
        sample.gpus = vec![
            project::Gpu {
                name: "Same model".into(),
                usage: Some(0.),
                memory_used: Some(200),
                memory_total: Some(1000),
                ..Default::default()
            },
            project::Gpu {
                name: "Same model".into(),
                usage: Some(83.),
                memory_used: Some(800),
                memory_total: Some(1000),
                ..Default::default()
            },
        ];
        state.record(sample.clone(), now, Duration::from_secs(2));
        let reading = state.history.back().unwrap();
        assert_eq!(reading.gpus[0].usage, Some(0.));
        assert_eq!(reading.gpus[1].usage, Some(83.));
        assert_eq!(reading.gpus[0].memory, Some(20.));
        assert_eq!(reading.gpus[1].memory, Some(80.));
        sample.gpus[0].usage = None;
        sample.gpus[1].usage = Some(f32::NAN);
        sample.gpus[1].memory_total = Some(0);
        sample.memory_total = 0;
        state.record(sample, now + Duration::from_secs(2), Duration::from_secs(2));
        let reading = state.history.back().unwrap();
        assert!(reading.gpus.iter().all(|g| g.usage.is_none()));
        assert!(reading.gpus[1].memory.is_none());
        assert!(reading.memory.is_none());
    }

    #[test]
    fn curves_use_real_times_and_break_at_missing_values_and_pauses() {
        let now = Instant::now();
        let times: Vec<_> = [0, 2, 4, 6, 8, 40, 42]
            .map(|s| now + Duration::from_secs(s))
            .into();
        let values = [
            Some(1.),
            Some(2.),
            None,
            Some(3.),
            Some(4.),
            Some(5.),
            Some(6.),
        ];
        let curves = segments(&times, &values, Duration::from_secs(6));
        assert_eq!(curves.len(), 3);
        assert_eq!(curves[0], vec![(times[0], 1.), (times[1], 2.)]);
        assert_eq!(curves[1], vec![(times[3], 3.), (times[4], 4.)]);
        assert_eq!(curves[2], vec![(times[5], 5.), (times[6], 6.)]);
    }

    #[test]
    fn charts_fit_compact_widths_and_use_the_active_palette() {
        let ctx = egui::Context::default();
        let now = Instant::now();
        let times = [
            now,
            now + Duration::from_secs(2),
            now + Duration::from_secs(5),
        ];
        let light = theme::Palette {
            background: egui::Color32::WHITE,
            panel: egui::Color32::from_gray(245),
            text: egui::Color32::BLACK,
            accent: egui::Color32::from_rgb(20, 65, 160),
            border: egui::Color32::from_gray(180),
            ..theme::Palette::default()
        };
        for palette in [
            theme::Palette::default(),
            theme::Theme::carbon().palette,
            light,
        ] {
            for width in [250., 420., 1100.] {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 600.),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        theme::install(ctx, palette);
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let bounds = ui.max_rect();
                            card(ui, palette, |ui| {
                                heading(
                                    ui,
                                    "GPU with a very long descriptive device name",
                                    Some(75.),
                                    palette,
                                );
                                legend(
                                    ui,
                                    &[
                                        Series {
                                            name: "GPU usage",
                                            color: palette.accent,
                                            values: vec![None],
                                        },
                                        Series {
                                            name: "VRAM",
                                            color: secondary_color(palette),
                                            values: vec![Some(50.)],
                                        },
                                    ],
                                );
                                chart(
                                    ui,
                                    &times,
                                    &[Series {
                                        name: "GPU",
                                        color: palette.accent,
                                        values: vec![Some(10.), Some(75.), Some(45.)],
                                    }],
                                    Duration::from_secs(2),
                                    palette,
                                );
                            });
                            assert!(
                                ui.min_rect().right() <= bounds.right() + 1.,
                                "Overflow at width {width}: {:?}",
                                ui.min_rect()
                            );
                        });
                    },
                );
                assert!(output.shapes.iter().any(
                    |s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == palette.background)
                ));
                assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Path(p) if p.stroke.color == egui::epaint::ColorMode::Solid(palette.accent))));
                assert!(output.shapes.iter().any(
                    |s| matches!(&s.shape, egui::Shape::Text(t) if t.fallback_color == palette.text)
                ));
            }
        }
    }
}
