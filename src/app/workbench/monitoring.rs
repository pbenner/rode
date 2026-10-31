//! Themed host charts with bounded, timestamped samples. Poll only while visible;
//! preserve gaps and unavailable readings instead of plotting invented zeroes.
use super::*;
use std::collections::VecDeque;

const HISTORY_LIMIT: usize = 120;

#[derive(Default)]
pub(super) struct State {
    pub(super) monitor: Option<project::Monitor>,
    pub(super) monitor_busy: bool,
    pub(super) monitor_last: Option<Instant>,
    pub(super) monitor_error: String,
    cpu_usage: Vec<Option<f32>>,
    pub(super) history: VecDeque<Reading>,
    network_rate: Option<(f32, f32)>,
}

pub(super) struct Reading {
    at: Instant,
    cpu: Option<f32>,
    busiest: Option<f32>,
    memory: Option<f32>,
    gpus: Vec<GpuReading>,
}

struct GpuReading {
    name: String,
    usage: Option<f32>,
    memory: Option<f32>,
}

impl State {
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

fn percentage(used: u64, total: u64) -> Option<f32> {
    (total > 0).then(|| (100. * used as f64 / total as f64).clamp(0., 100.) as f32)
}

fn valid_percentage(value: f32) -> Option<f32> {
    value.is_finite().then(|| value.clamp(0., 100.))
}

fn percent_label(value: Option<f32>) -> String {
    value.map_or_else(|| "Unavailable".into(), |v| format!("{v:.1}%"))
}

impl EditorApp {
    pub(in crate::app) fn monitor_ui(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        let state = &self.workbench.monitoring;
        let secondary = secondary_color(palette);
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
        let times: Vec<_> = state.history.iter().map(|r| r.at).collect();
        let series = |name, color, values| Series {
            name,
            color,
            values,
        };
        card(ui, palette, |ui| {
            heading(ui, "CPU", latest.cpu, palette);
            let curves = [
                series(
                    "Overall",
                    palette.accent,
                    state.history.iter().map(|r| r.cpu).collect(),
                ),
                series(
                    "Busiest core",
                    secondary,
                    state.history.iter().map(|r| r.busiest).collect(),
                ),
            ];
            legend(ui, &curves);
            chart(ui, &times, &curves, interval, palette);
            ui.label(format!(
                "{} logical cores",
                sample.cpus.len().saturating_sub(1)
            ));
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
        });
        for (index, gpu) in sample.gpus.iter().enumerate() {
            card(ui, palette, |ui| {
                heading(ui, &gpu.name, gpu.usage.and_then(valid_percentage), palette);
                // Keep identical models independent; the backend's device order identifies each GPU.
                let readings: Vec<_> = state
                    .history
                    .iter()
                    .map(|r| r.gpus.get(index).filter(|g| g.name == gpu.name))
                    .collect();
                let curves = [
                    series(
                        "GPU usage",
                        palette.accent,
                        readings.iter().map(|g| g.and_then(|g| g.usage)).collect(),
                    ),
                    series(
                        "VRAM",
                        secondary,
                        readings.iter().map(|g| g.and_then(|g| g.memory)).collect(),
                    ),
                ];
                legend(ui, &curves);
                chart(ui, &times, &curves, interval, palette);
                ui.horizontal_wrapped(|ui| {
                    ui.label(match gpu.memory_used.zip(gpu.memory_total) {
                        Some((used, total)) if total > 0 => {
                            format!("VRAM {} / {}", bytes(used), bytes(total))
                        }
                        _ => "VRAM unavailable".into(),
                    });
                    ui.label(gpu.temperature.filter(|v| v.is_finite()).map_or_else(
                        || "Temperature unavailable".into(),
                        |v| format!("{v:.0} °C"),
                    ));
                    ui.label(
                        gpu.power
                            .filter(|v| v.is_finite())
                            .map_or_else(|| "Power unavailable".into(), |v| format!("{v:.1} W")),
                    );
                });
            });
        }
        if sample.gpus.is_empty() || !sample.gpu_note.is_empty() {
            card(ui, palette, |ui| {
                if sample.gpus.is_empty() {
                    ui.label("No GPU telemetry available on this host.");
                }
                if !sample.gpu_note.is_empty() {
                    ui.label(&sample.gpu_note);
                }
            });
        }
        card(ui, palette, |ui| {
            heading(ui, "Memory", latest.memory, palette);
            let curves = [series(
                "RAM",
                palette.accent,
                state.history.iter().map(|r| r.memory).collect(),
            )];
            chart(ui, &times, &curves, interval, palette);
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "RAM {} / {}",
                    bytes(sample.memory_total.saturating_sub(sample.memory_available)),
                    bytes(sample.memory_total)
                ));
                if sample.swap_total > 0 {
                    ui.label(format!(
                        "Swap {} / {}",
                        bytes(sample.swap_total.saturating_sub(sample.swap_free)),
                        bytes(sample.swap_total)
                    ));
                }
            });
        });
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

fn card(ui: &mut egui::Ui, palette: theme::Palette, contents: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(10.);
    egui::Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(1., palette.border))
        .corner_radius(6.)
        .inner_margin(12.)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            contents(ui);
        });
}

fn heading(ui: &mut egui::Ui, title: &str, value: Option<f32>, palette: theme::Palette) {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 140.).max(20.);
        ui.add_sized(
            [width, 30.],
            egui::Label::new(RichText::new(title).size(15.).color(palette.text)).truncate(),
        )
        .on_hover_text(title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(percent_label(value))
                    .size(if value.is_some() { 26. } else { 16. })
                    .color(palette.text),
            );
        });
    });
}

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

fn legend(ui: &mut egui::Ui, series: &[Series<'_>]) {
    ui.horizontal_wrapped(|ui| {
        for series in series {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(16., 14.), Sense::hover());
            ui.painter().line_segment(
                [rect.left_center(), rect.right_center()],
                Stroke::new(2., series.color),
            );
            ui.label(format!(
                "{} {}",
                series.name,
                percent_label(series.values.last().copied().flatten())
            ));
            ui.add_space(8.);
        }
    });
}

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

fn chart(
    ui: &mut egui::Ui,
    times: &[Instant],
    series: &[Series<'_>],
    interval: Duration,
    palette: theme::Palette,
) {
    let height = if ui.available_width() < 450. {
        130.
    } else {
        155.
    };
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height + 30.),
        Sense::hover(),
    );
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
            Stroke::new(1., palette.border),
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
                    Stroke::new(if index == 0 { 2. } else { 1.3 }, series.color),
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
            Stroke::new(1., palette.muted),
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
