//! Native, theme-aware price/volume plot. The viewport is normalized to trading
//! observations (closed markets leave no artificial gaps); labels use provider UTC times.
use super::protocol;
use crate::app::*;
use rode_extension_api::panel::Bar;
#[cfg(test)]
use rode_extension_api::panel::Series;

/// Keep endpoints and both extrema in a dense line bucket. Sampling just its
/// final close can erase an entire short price spike from the plotted history.
fn line_indices(bars: &[Bar]) -> [usize; 4] {
    debug_assert!(!bars.is_empty());
    let mut indices = [0, 0, 0, bars.len().saturating_sub(1)];
    for (index, bar) in bars.iter().enumerate() {
        if bar.close < bars[indices[1]].close {
            indices[1] = index;
        }
        if bar.close > bars[indices[2]].close {
            indices[2] = index;
        }
    }
    indices.sort_unstable();
    indices
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct View {
    start: f64,
    width: f64,
}
impl Default for View {
    fn default() -> Self {
        Self {
            start: 0.,
            width: 1.,
        }
    }
}
impl View {
    /// Zoom around the pointer/center, retaining at least eight observations when possible.
    pub fn zoom(&mut self, factor: f64, anchor: f64, count: usize) {
        if !factor.is_finite() || !anchor.is_finite() || factor <= 0. {
            return;
        }
        let anchor = anchor.clamp(0., 1.);
        let width = (self.width * factor).clamp((8. / count.max(1) as f64).min(1.), 1.);
        self.start = (self.start + (self.width - width) * anchor).clamp(0., 1. - width);
        self.width = width;
    }
    pub fn pan(&mut self, fraction: f64) {
        if fraction.is_finite() {
            self.start = (self.start + fraction * self.width).clamp(0., 1. - self.width);
        }
    }
    fn indices(self, count: usize) -> (usize, usize) {
        let first = (self.start * count as f64).floor() as usize;
        let end = ((self.start + self.width) * count as f64).ceil() as usize;
        (first.min(count.saturating_sub(1)), end.clamp(1, count))
    }
}

use rode_extension_api::panel::format_price as price;
pub(super) fn date(at: i64, intraday: bool) -> String {
    let Ok(at) = time::OffsetDateTime::from_unix_timestamp(at) else {
        return String::new();
    };
    let date = format!("{:04}-{:02}-{:02}", at.year(), at.month() as u8, at.day());
    if intraday {
        format!("{date} {:02}:{:02}", at.hour(), at.minute())
    } else {
        date
    }
}

/// Axes and data share one measured rectangle; painter clipping prevents lines,
/// candles or crosshairs from escaping into controls at fractional UI scales.
pub(super) fn show(
    ui: &mut egui::Ui,
    plot: &protocol::Chart,
    view: &mut View,
    palette: theme::Palette,
    height: f32,
) {
    let protocol::Chart {
        key,
        title: label,
        series,
        candles,
        volume,
        ..
    } = plot;
    let (candles, volume) = (*candles, *volume);
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width().max(1.), height));
    // Search results and watchlist entries change widget counts above this plot.
    // Its focus identity must not depend on those auto-ID allocations.
    let response = ui.interact(
        rect,
        ui.id().with(("extension-chart", key)),
        Sense::click_and_drag(),
    );
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, ui.is_enabled(), label));
    keyboard::target(ui, label, &response);
    ui_probe::button(ui, &format!("extension-chart:{key}"), &response);
    if response.clicked() {
        response.request_focus();
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8., palette.background);
    painter.rect_stroke(
        rect.shrink(0.5),
        8.,
        Stroke::new(
            1.0_f32,
            if response.has_focus() {
                palette.accent
            } else {
                palette.border
            },
        ),
        egui::StrokeKind::Inside,
    );
    let plot = egui::Rect::from_min_max(
        rect.min + egui::vec2(12., 18.),
        rect.max - egui::vec2(84., 38.),
    );
    if plot.width() < 30. || plot.height() < 30. || series.bars.is_empty() {
        return;
    }
    let count = series.bars.len();
    if response.hovered() {
        let scroll = ui.input_mut(|i| {
            let delta = i.smooth_scroll_delta.y;
            if delta != 0. {
                i.smooth_scroll_delta = egui::Vec2::ZERO;
                i.raw_scroll_delta = egui::Vec2::ZERO;
            }
            delta
        });
        if scroll != 0. {
            let anchor = response
                .hover_pos()
                .map(|p| (p.x - plot.left()) / plot.width())
                .unwrap_or(0.5);
            view.zoom(
                (-scroll as f64 * 0.003).clamp(-2., 2.).exp(),
                anchor as f64,
                count,
            );
        }
    }
    if response.dragged() {
        view.pan(-response.drag_delta().x as f64 / plot.width() as f64);
    }
    if response.double_clicked() {
        *view = View::default();
    }
    if response.has_focus() {
        let mut moved = false;
        ui.input_mut(|i| {
            for key in [Key::Plus, Key::Equals] {
                if i.consume_key(egui::Modifiers::NONE, key) {
                    view.zoom(0.75, 0.5, count);
                }
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Minus) {
                view.zoom(1. / 0.75, 0.5, count);
            }
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowLeft) {
                view.pan(-0.15);
                moved = true;
            }
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowRight) {
                view.pan(0.15);
                moved = true;
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Home) {
                *view = View::default();
            }
        });
        if moved {
            ui.memory_mut(|memory| {
                memory.move_focus(egui::FocusDirection::None);
                memory.request_focus(response.id);
            });
        }
    }
    ui_probe::value(
        ui,
        &format!("extension-chart-window:{key}"),
        &response,
        &format!("{:.5}:{:.5}", view.start, view.width),
    );
    ui_probe::value(
        ui,
        &format!("extension-chart-style:{key}"),
        &response,
        &format!("{candles}:{volume}"),
    );
    let (first, end) = view.indices(count);
    let bars = &series.bars[first..end];
    let prices = egui::Rect::from_min_max(
        plot.min,
        egui::pos2(
            plot.right(),
            if volume {
                plot.bottom() - plot.height() * 0.2 - 12.
            } else {
                plot.bottom()
            },
        ),
    );
    let low = bars
        .iter()
        .map(|b| if candles { b.low } else { b.close })
        .fold(f64::INFINITY, f64::min);
    let high = bars
        .iter()
        .map(|b| if candles { b.high } else { b.close })
        .fold(f64::NEG_INFINITY, f64::max);
    let padding = ((high - low) * 0.06).max(high.abs().max(low.abs()).max(1e-8) * 0.001);
    let (low, high) = (low - padding, high + padding);
    let x = |index: f64| {
        prices.left()
            + ((index - view.start * count as f64 + 0.5) / (view.width * count as f64)) as f32
                * prices.width()
    };
    let y = |price: f64| prices.bottom() - ((price - low) / (high - low)) as f32 * prices.height();
    let grid = Stroke::new(1.0_f32, palette.border.gamma_multiply(0.5));
    for tick in 0..=4 {
        let value = low + (high - low) * tick as f64 / 4.;
        let y = y(value);
        painter.line_segment(
            [egui::pos2(prices.left(), y), egui::pos2(prices.right(), y)],
            grid,
        );
        painter.text(
            egui::pos2(prices.right() + 10., y),
            egui::Align2::LEFT_CENTER,
            price(value),
            FontId::monospace(11.),
            palette.muted,
        );
    }
    let ticks = [0, 1, 2, 3].map(|tick| first + bars.len().saturating_sub(1) * tick / 3);
    for (tick, &index) in ticks.iter().enumerate() {
        if tick > 0 && ticks[tick - 1] == index {
            continue;
        }
        let at = x(index as f64).clamp(plot.left(), plot.right());
        painter.line_segment(
            [egui::pos2(at, plot.top()), egui::pos2(at, plot.bottom())],
            grid,
        );
        let at_time = series.bars[index].at;
        let label = if date(series.bars[0].at, false) == date(series.bars[count - 1].at, false) {
            date(at_time, true)
                .split_once(' ')
                .map(|(_, time)| time.to_owned())
                .unwrap_or_default()
        } else {
            date(at_time, false)
        };
        painter.text(
            egui::pos2(at, plot.bottom() + 12.),
            if bars.len() == 1 {
                egui::Align2::CENTER_TOP
            } else if index == first {
                egui::Align2::LEFT_TOP
            } else if index == end - 1 {
                egui::Align2::RIGHT_TOP
            } else {
                egui::Align2::CENTER_TOP
            },
            label,
            FontId::proportional(11.),
            palette.muted,
        );
    }
    let ink = painter.with_clip_rect(plot.intersect(ui.clip_rect()));
    // At high density aggregate complete OHLC/volume groups to at most one
    // candle per logical pixel. Hover still reads the original observation.
    let stride = bars.len().div_ceil(prices.width().max(1.) as usize).max(1);
    let max_volume = bars
        .chunks(stride)
        .map(|g| g.iter().map(|b| b.volume).sum::<f64>())
        .fold(1., f64::max);
    let mut line = Vec::new();
    for (chunk, group) in bars.chunks(stride).enumerate() {
        let first_bar = group.first().unwrap();
        let last_bar = group.last().unwrap();
        let center = x((first + chunk * stride) as f64 + (group.len() - 1) as f64 / 2.);
        let width = ((stride as f64 / (view.width * count as f64)) as f32 * prices.width() * 0.7)
            .clamp(1., 20.);
        let color = if last_bar.close >= first_bar.open {
            palette.success
        } else {
            palette.error
        };
        if candles {
            let top = group
                .iter()
                .map(|b| b.high)
                .fold(f64::NEG_INFINITY, f64::max);
            let bottom = group.iter().map(|b| b.low).fold(f64::INFINITY, f64::min);
            ink.line_segment(
                [egui::pos2(center, y(top)), egui::pos2(center, y(bottom))],
                Stroke::new(1.0_f32, color),
            );
            let a = y(first_bar.open);
            let b = y(last_bar.close);
            ink.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(center - width / 2., a.min(b)),
                    egui::pos2(center + width / 2., a.max(b).max(a.min(b) + 1.)),
                ),
                1.,
                color,
            );
        } else {
            let mut previous = None;
            for offset in line_indices(group) {
                if previous == Some(offset) {
                    continue;
                }
                previous = Some(offset);
                line.push(egui::pos2(
                    x((first + chunk * stride + offset) as f64),
                    y(group[offset].close),
                ));
            }
        }
        if volume {
            let height = (group.iter().map(|b| b.volume).sum::<f64>() / max_volume) as f32
                * plot.height()
                * 0.2;
            ink.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(center - width / 2., plot.bottom() - height),
                    egui::pos2(center + width / 2., plot.bottom()),
                ),
                0.,
                color.gamma_multiply(0.4),
            );
        }
    }
    if line.len() == 1 {
        ink.circle_filled(line[0], 3., palette.accent);
    } else if !line.is_empty() {
        ink.add(egui::Shape::line(
            line,
            Stroke::new(2.0_f32, palette.accent),
        ));
    }
    if let Some(pointer) = response.hover_pos().filter(|p| plot.contains(*p)) {
        let index = ((view.start
            + (pointer.x - plot.left()) as f64 / plot.width() as f64 * view.width)
            * count as f64)
            .floor() as usize;
        let bar = &series.bars[index.min(count - 1)];
        ink.line_segment(
            [
                egui::pos2(pointer.x, plot.top()),
                egui::pos2(pointer.x, plot.bottom()),
            ],
            Stroke::new(1.0_f32, palette.muted),
        );
        response.on_hover_ui(|ui| {
            ui.label(format!("{} UTC", date(bar.at, true)));
            ui.monospace(format!(
                "O {}   H {}\nL {}   C {}\nVolume {:.0}",
                price(bar.open),
                price(bar.high),
                price(bar.low),
                price(bar.close),
                bar.volume
            ));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zoom_and_pan_never_leave_data_and_small_series_remain_visible() {
        for count in [1, 2, 8, 250, 25_000] {
            let mut view = View::default();
            for _ in 0..100 {
                view.zoom(0.1, 0.9, count);
                view.pan(10.);
                let (a, b) = view.indices(count);
                assert!(a < b && b <= count);
                assert!(view.start >= 0. && view.start + view.width <= 1.000001);
            }
            view.zoom(f64::NAN, 0.5, count);
            view.pan(f64::INFINITY);
            view.zoom(1e30, 0.5, count);
            assert_eq!(view.indices(count), (0, count));
        }
        assert_eq!(date(0, true), "1970-01-01 00:00");
        assert_eq!(date(-1, true), "1969-12-31 23:59");
    }

    #[test]
    fn dense_line_buckets_retain_spikes_and_price_order() {
        let bars: Vec<_> = [10., 30., 2., 10.]
            .into_iter()
            .map(|close| Bar {
                at: 0,
                open: close,
                high: close,
                low: close,
                close,
                volume: 0.,
            })
            .collect();
        assert_eq!(line_indices(&bars), [0, 1, 2, 3]);
        assert_eq!(line_indices(&bars[..1]), [0, 0, 0, 0]);
    }

    #[test]
    fn single_bar_has_one_time_label_and_focus_survives_controls_added_above_it() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let app = EditorApp::new(&cc, Startup::default());
        let palette = app.themes.active().palette;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("chart.json");
        ui_probe::install_at(&ctx, path.clone());
        let series = Series {
            name: String::new(),
            exchange: String::new(),
            currency: String::new(),
            bars: vec![Bar {
                at: 0,
                open: 10.,
                high: 10.,
                low: 10.,
                close: 10.,
                volume: 0.,
            }],
        };
        let mut view = View::default();
        for frame in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(700., 600.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if frame == 0 {
                            ui.memory_mut(|m| {
                                m.request_focus(ui.id().with(("extension-chart", "test")))
                            });
                        }
                        for _ in 0..frame {
                            let _ = keyboard::button(ui, "Extra watchlist entry");
                        }
                        show(
                            ui,
                            &protocol::Chart {
                                key: "test".into(),
                                title: "Price chart".into(),
                                subtitle: String::new(),
                                series: series.clone(),
                                candles: false,
                                volume: true,
                            },
                            &mut view,
                            palette,
                            300.,
                        );
                    });
                },
            );
            assert_eq!(output.shapes.iter().filter(|shape|matches!(&shape.shape,egui::Shape::Text(text) if text.galley.job.text=="00:00")).count(),1);
            let snapshot: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let chart = snapshot["buttons"]
                .as_array()
                .unwrap()
                .iter()
                .find(|button| button["name"] == "extension-chart:test")
                .unwrap();
            assert_eq!(chart["focused"], true);
        }
    }
}
