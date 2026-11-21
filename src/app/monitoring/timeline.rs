//! One shared time axis for every host chart. Scrolling follows monotonic wall
//! time, independently of sampling success; history fits only collected samples.
use super::*;

pub(super) const MAX_SECONDS: u64 = 600;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::app) enum Mode {
    #[default]
    Scrolling,
    History,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(in crate::app) struct View {
    pub mode: Mode,
    pub seconds: u64,
}
impl Default for View {
    fn default() -> Self {
        Self {
            mode: Mode::Scrolling,
            seconds: 120,
        }
    }
}
impl View {
    /// Shared, keyboard-accessible controls for the dashboard and settings form.
    pub fn controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Chart view");
            let mode = crate::keyboard::ComboBox::new("monitor-chart-mode", "Monitor chart view")
                .selected_text(match self.mode {
                    Mode::Scrolling => "Scrolling",
                    Mode::History => "Fit history",
                })
                .width(120.)
                .show_ui(ui, |ui| {
                    for (mode, label) in [
                        (Mode::Scrolling, "Scrolling"),
                        (Mode::History, "Fit history"),
                    ] {
                        let response =
                            crate::keyboard::selectable_value(ui, &mut self.mode, mode, label);
                        ui_probe::button(ui, &format!("monitor-view-{label}"), &response);
                    }
                });
            ui_probe::button(ui, "monitor-chart-mode", &mode.response);
            ui.add_enabled_ui(self.mode == Mode::Scrolling, |ui| {
                ui.label("Window");
                let window =
                    crate::keyboard::ComboBox::new("monitor-chart-window", "Monitor time window")
                        .selected_text(duration_label(self.seconds.clamp(30, MAX_SECONDS) as f32))
                        .width(90.)
                        .show_ui(ui, |ui| {
                            for seconds in [30, 60, 120, 300, 600] {
                                let response = crate::keyboard::selectable_value(
                                    ui,
                                    &mut self.seconds,
                                    seconds,
                                    duration_label(seconds as f32),
                                );
                                ui_probe::button(
                                    ui,
                                    &format!("monitor-window-{seconds}"),
                                    &response,
                                );
                            }
                        });
                ui_probe::button(ui, "monitor-chart-window", &window.response);
            });
        });
    }

    pub(super) fn axis(
        self,
        first: Instant,
        latest: Instant,
        now: Instant,
        interval: Duration,
    ) -> Axis {
        Axis {
            end: if self.mode == Mode::Scrolling {
                now.max(latest)
            } else {
                latest
            },
            span: if self.mode == Mode::Scrolling {
                Duration::from_secs(self.seconds.clamp(30, MAX_SECONDS))
            } else {
                latest.duration_since(first).max(interval)
            },
            live: self.mode == Mode::Scrolling,
        }
    }
}

/// Shared per-frame bounds keep axes aligned across cards and GPU rows.
#[derive(Clone, Copy)]
pub(super) struct Axis {
    pub end: Instant,
    pub span: Duration,
    pub live: bool,
}
impl Axis {
    /// Values outside 0–1 are clipped by the plot, never compressed into view.
    pub fn fraction(self, time: Instant) -> f32 {
        1. - self.end.duration_since(time).as_secs_f32() / self.span.as_secs_f32()
    }
    pub fn contains(self, time: Instant) -> bool {
        (0. ..=1.).contains(&self.fraction(time))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolling_keeps_a_fixed_scale_from_startup_and_moves_old_samples_left() {
        let first = Instant::now();
        let view = View::default();
        let axis = view.axis(first, first, first, Duration::from_secs(2));
        assert_eq!(axis.span, Duration::from_secs(120));
        assert_eq!(axis.fraction(first), 1.);
        let later = view.axis(
            first,
            first,
            first + Duration::from_secs(30),
            Duration::from_secs(2),
        );
        assert_eq!(later.span, axis.span);
        assert_eq!(later.fraction(first), 0.75);
        assert_eq!(later.fraction(first + Duration::from_secs(30)), 1.);
        let expired = view.axis(
            first,
            first,
            first + Duration::from_secs(121),
            Duration::from_secs(2),
        );
        assert!(!expired.contains(first));
    }

    #[test]
    fn history_stays_anchored_to_samples_and_saved_windows_are_bounded() {
        let first = Instant::now();
        let latest = first + Duration::from_secs(20);
        let history = View {
            mode: Mode::History,
            ..Default::default()
        }
        .axis(
            first,
            latest,
            latest + Duration::from_secs(60),
            Duration::from_secs(2),
        );
        assert_eq!(history.end, latest);
        assert_eq!(history.span, Duration::from_secs(20));
        assert_eq!(history.fraction(first), 0.);
        assert_eq!(history.fraction(latest), 1.);
        for (seconds, expected) in [(0, 30), (u64::MAX, MAX_SECONDS)] {
            let axis = View {
                seconds,
                ..Default::default()
            }
            .axis(first, latest, latest, Duration::from_secs(30));
            assert_eq!(axis.span, Duration::from_secs(expected));
        }
        let restored: View = serde_json::from_str("{}").unwrap();
        assert_eq!(restored, View::default());
    }
}
