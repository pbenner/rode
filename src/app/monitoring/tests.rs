//! Monitor sampling, history and responsive chart regressions.
use super::*;

#[test]
fn fractional_sampling_intervals_stay_finite_and_within_bounds() {
    for (seconds, millis) in [
        (0.1, 100),
        (0.2, 200),
        (0.9, 900),
        (2.5, 2500),
        (5.0, 5000),
        (0.0, 100),
        (-1.0, 100),
        (30.0, 5000),
        (f64::NAN, 1000),
        (f64::INFINITY, 1000),
        (f64::NEG_INFINITY, 1000),
    ] {
        assert_eq!(sampling_interval(seconds), Duration::from_millis(millis));
    }
}

#[test]
fn sampling_controls_fit_narrow_panels_at_both_ends_of_the_range() {
    for width in [180., 240., 320., 800.] {
        for mut seconds in [0.1, 5.0] {
            let ctx = egui::Context::default();
            for _ in 0..3 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 200.),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let bounds = ui.max_rect();
                            sampling_control(ui, &mut seconds);
                            assert!(
                                ui.min_rect().right() <= bounds.right() + 1.,
                                "Sampling control overflow at {width} points"
                            );
                        });
                    },
                );
            }
        }
    }
}

#[test]
fn animation_only_runs_for_visible_started_scrolling_charts() {
    for (active, visible, mode, animated) in [
        (true, true, timeline::Mode::Scrolling, true),
        (false, true, timeline::Mode::Scrolling, false),
        (true, false, timeline::Mode::Scrolling, false),
        (true, true, timeline::Mode::History, false),
    ] {
        let ctx = egui::Context::default();
        let mut state = State {
            active,
            ..Default::default()
        };
        let interval = Duration::from_secs(2);
        let now = Instant::now();
        state.record(sample(100), now - interval, interval);
        state.record(sample(200), now, interval);
        let mut delay = Duration::ZERO;
        // Drain egui's initial font/layout repaint requests before inspecting
        // steady-state scheduling from the real dashboard render path.
        for frame in 0..5 {
            let output = ctx.run(
                egui::RawInput {
                    time: Some(frame as f64 / 60.),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800., 600.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if !visible {
                            ui.set_clip_rect(egui::Rect::NOTHING);
                        }
                        dashboard::show(
                            ui,
                            &state,
                            interval,
                            ChartView {
                                mode,
                                ..Default::default()
                            },
                            theme::Palette::default(),
                        );
                    });
                },
            );
            delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        }
        if animated {
            assert!(delay <= Duration::from_secs_f64(1. / 60.), "{delay:?}");
        } else {
            assert_eq!(
                delay,
                Duration::MAX,
                "Unexpected animation: {mode:?}, active={active}, visible={visible}"
            );
        }
        assert_eq!(state.history.len(), 2, "Rendering collected a sample");
    }
}

#[test]
fn dashboard_reflows_without_stretching_plots_or_misaligning_cards() {
    for palette in [
        theme::Palette::default(),
        theme::Theme::builtin("studio").unwrap().palette,
    ] {
        assert_dashboard_layout(palette);
    }
}

fn assert_dashboard_layout(palette: theme::Palette) {
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
                            dashboard::show(ui, &state, interval, ChartView::default(), palette);
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
                    egui::Shape::Rect(rect) if rect.fill == palette.raised => Some(rect.rect),
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
    assert!(state.history.len() <= HISTORY_LIMIT);
    assert_eq!(state.history.back().unwrap().at, now + interval * 149);
    // Host counter resets and zero deltas do not create artificial idle readings.
    state.record(sample(1), now + interval * 150, interval);
    assert!(state.history.back().unwrap().cpu.is_none());
    assert!(state.network_rate.is_none());
    state.record(sample(1), now + interval * 151, interval);
    assert!(state.history.back().unwrap().cpu.is_none());
    let mut bounded = State::default();
    let fastest = sampling_interval(0.1);
    assert_eq!(fastest, Duration::from_millis(100));
    assert_eq!(HISTORY_LIMIT, 6003);
    for index in 0..HISTORY_LIMIT + 20 {
        bounded.record(
            sample(index as u64 * 100),
            now + fastest * index as u32,
            fastest,
        );
    }
    assert_eq!(bounded.history.len(), HISTORY_LIMIT);
    assert!(
        bounded
            .history
            .back()
            .unwrap()
            .at
            .duration_since(bounded.history[2].at)
            <= Duration::from_secs(timeline::MAX_SECONDS)
    );
    let boundary = bounded.history.back().unwrap().at - Duration::from_secs(timeline::MAX_SECONDS);
    assert_eq!(
        bounded
            .history
            .iter()
            .take_while(|r| r.at < boundary)
            .count(),
        2,
        "Left-edge curve tangents need both preceding samples"
    );
    // At slower sample rates, retention still follows elapsed time rather
    // than retaining hours of data in the same number of sample slots.
    bounded.record(sample(100_000), now + Duration::from_secs(3600), interval);
    assert_eq!(bounded.history.len(), 3);
    assert!(bounded.history.back().unwrap().cpu.is_none());
}

#[test]
fn resetting_history_clears_rate_baselines_and_makes_the_next_poll_due() {
    let mut state = State::default();
    let interval = Duration::from_secs(2);
    let now = Instant::now();
    state.record(sample(100), now, interval);
    state.record(sample(200), now + interval, interval);
    state.monitor_last = Some(now + interval);
    assert!(state.network_rate.is_some());
    assert_eq!(state.cpu_usage[0], Some(50.));
    state.reset_history();
    assert!(state.monitor.is_none());
    assert!(state.history.is_empty());
    assert!(state.cpu_usage.is_empty());
    assert!(state.network_rate.is_none());
    assert!(state.monitor_last.is_none());
    assert!(!state.monitor_busy);
    assert!(!state.discard_reply());
    let fresh = now + Duration::from_secs(3 * 3600);
    state.record(sample(300), fresh, interval);
    assert_eq!(state.history.len(), 1);
    assert!(state.history[0].cpu.is_none());
    assert!(state.network_rate.is_none());
    state.record(sample(400), fresh + interval, interval);
    assert_eq!(state.history.len(), 2);
    assert_eq!(state.history[1].cpu, Some(50.));
    assert!(state.network_rate.is_some());
}

#[test]
fn stopping_freezes_the_chart_and_resuming_requires_two_new_counter_samples() {
    let mut state = State::default();
    let interval = Duration::from_secs(2);
    let now = Instant::now();
    state.set_active(true);
    state.record(sample(100), now, interval);
    state.record(sample(200), now + interval, interval);
    assert!(state.network_rate.is_some());
    state.set_active(false);
    let stopped = state.stopped_at;
    state.set_active(false);
    assert_eq!(state.stopped_at, stopped);
    assert_eq!(state.history.len(), 2);
    state.reset_history();
    assert!(!state.active);
    assert_eq!(state.stopped_at, stopped);
    state.set_active(true);
    assert!(state.stopped_at.is_none());
    state.record(sample(300), now + interval * 2, interval);
    assert!(state.network_rate.is_none());
    assert!(state.history.back().unwrap().cpu.is_none());
    state.record(sample(400), now + interval * 3, interval);
    assert!(state.network_rate.is_some());
    state.set_active(false);
    state.set_active(true);
    state.record(sample(450), now + interval * 4, interval);
    assert!(state.network_rate.is_none());
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
                                ChartView {
                                    mode: timeline::Mode::History,
                                    ..Default::default()
                                }
                                .axis(
                                    times[0],
                                    times[2],
                                    times[2],
                                    Duration::from_secs(2),
                                ),
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
            assert!(
                output.shapes.iter().any(
                    |s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == palette.background)
                )
            );
            assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Path(p) if p.stroke.color == egui::epaint::ColorMode::Solid(palette.accent))));
            assert!(output.shapes.iter().any(
                |s| matches!(&s.shape, egui::Shape::Text(t) if t.fallback_color == palette.text)
            ));
        }
    }
}
