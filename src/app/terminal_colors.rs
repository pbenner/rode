//! Per-interface-theme terminal overrides and their native settings window.
//!
//! Each explicit swatch/contrast edit is a scalar preference delta. Keeping
//! pending keys separate from the loaded values prevents an idle/stale window
//! from overwriting another window's edits. Theme files remain reusable defaults;
//! no shell configuration, project file or remote preference is written here.
use super::*;
use eframe::Storage;
use eframe::egui::Color32;
use std::collections::BTreeMap;

const NAMES: [&str; 16] = [
    "Black",
    "Red",
    "Green",
    "Yellow",
    "Blue",
    "Magenta",
    "Cyan",
    "White",
    "Bright black",
    "Bright red",
    "Bright green",
    "Bright yellow",
    "Bright blue",
    "Bright magenta",
    "Bright cyan",
    "Bright white",
];

fn key(id: &str, setting: &str) -> String {
    format!("terminal_color.{id}.{setting}")
}

fn hex(color: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

#[derive(Default)]
struct Overrides {
    foreground: Option<Color32>,
    background: Option<Color32>,
    ansi: [Option<Color32>; 16],
    minimum_contrast: Option<f32>,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) open: bool,
    themes: BTreeMap<String, Overrides>,
    pending: BTreeMap<String, String>,
}

impl State {
    pub(super) fn load(storage: Option<&dyn Storage>, themes: &theme::Themes) -> Self {
        let mut state = Self::default();
        for theme in &themes.entries {
            let read = |field: &str| storage.and_then(|s| s.get_string(&key(&theme.id, field)));
            let color = |field: &str| read(field).and_then(|v| theme::parse_color(&v).ok());
            state.themes.insert(
                theme.id.clone(),
                Overrides {
                    foreground: color("foreground"),
                    background: color("background"),
                    ansi: std::array::from_fn(|i| color(&format!("ansi.{i}"))),
                    minimum_contrast: read("minimum_contrast")
                        .and_then(|s| s.parse::<f32>().ok())
                        .filter(|v| v.is_finite() && (1.0..=21.0).contains(v)),
                },
            );
        }
        state
    }

    /// Cheap palette composition shared by project/login/error and debug views.
    pub(super) fn palette(&self, theme: &theme::Theme) -> theme::Palette {
        let mut palette = theme.palette;
        if let Some(settings) = self.themes.get(&theme.id) {
            palette.terminal_background =
                settings.background.unwrap_or(palette.terminal_background);
            palette.terminal_foreground =
                settings.foreground.unwrap_or(palette.terminal_foreground);
            palette.terminal_minimum_contrast = settings
                .minimum_contrast
                .unwrap_or(palette.terminal_minimum_contrast);
            for (color, saved) in palette.ansi.iter_mut().zip(settings.ansi) {
                *color = saved.unwrap_or(*color);
            }
        }
        palette
    }

    /// Drain only user edits through the existing locked/asynchronous writer.
    pub(super) fn store(&mut self, storage: &mut dyn Storage) {
        for (key, value) in std::mem::take(&mut self.pending) {
            storage.set_string(&key, value);
        }
    }

    fn reset(&mut self, id: &str) {
        self.themes.insert(id.into(), Overrides::default());
        for field in ["foreground", "background", "minimum_contrast"]
            .into_iter()
            .map(str::to_owned)
            .chain((0..16).map(|i| format!("ansi.{i}")))
        {
            // A sentinel preserves future changes in the theme defaults. An
            // explicit reset is still an edit when this window looks default.
            self.pending.insert(key(id, &field), "theme".into());
        }
    }

    pub(super) fn show(&mut self, ctx: &egui::Context, theme: &theme::Theme) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Terminal colors")
            .id(egui::Id::new("terminal-colors"))
            .open(&mut open)
            .collapsible(false)
            .default_width(410.)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 110.).max(180.))
                    .show(ui, |ui| {
                        ui.label(RichText::new(format!("Theme: {}", theme.name)).strong());
                        ui.label("Saved separately for each color theme. Applies to all terminal views.");
                        let mut palette = self.palette(theme);
                        let settings = self.themes.entry(theme.id.clone()).or_default();
                        ui.add_space(8.);
                        ui.label("Minimum text contrast");
                        if ui.add(egui::Slider::new(&mut palette.terminal_minimum_contrast, 1.0..=21.0)
                            .step_by(0.5).suffix(":1")).changed() {
                            settings.minimum_contrast = Some(palette.terminal_minimum_contrast);
                            self.pending.insert(key(&theme.id, "minimum_contrast"), palette.terminal_minimum_contrast.to_string());
                        }
                        ui.small("1 = off (default) · 4.5 = optional contrast boost");
                        ui.small("Adjusts text against its background, including RGB prompts. Keeps backgrounds and Powerline/block graphics unchanged.");
                        ui.separator();
                        egui::Grid::new("terminal-base-colors").show(ui, |ui| {
                            for (label, field, color, saved) in [
                                ("Foreground", "foreground", palette.terminal_foreground, &mut settings.foreground),
                                ("Background", "background", palette.terminal_background, &mut settings.background),
                            ] {
                                ui.label(label);
                                let mut rgb = [color.r(), color.g(), color.b()];
                                if ui.color_edit_button_srgb(&mut rgb).changed() {
                                    let color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                                    *saved = Some(color);
                                    self.pending.insert(key(&theme.id, field), hex(color));
                                }
                                ui.end_row();
                            }
                        });
                        ui.collapsing("ANSI colors", |ui| {
                            ui.small("Bold ANSI / PS1 colors use bright swatches (8–15).");
                            egui::Grid::new("terminal-ansi-colors").num_columns(4).show(ui, |ui| {
                                for i in 0..8 {
                                    for index in [i, i + 8] {
                                        ui.label(NAMES[index]);
                                        let c = palette.ansi[index];
                                        let mut rgb = [c.r(), c.g(), c.b()];
                                        if ui.color_edit_button_srgb(&mut rgb).changed() {
                                            let color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                                            settings.ansi[index] = Some(color);
                                            self.pending.insert(key(&theme.id, &format!("ansi.{index}")), hex(color));
                                        }
                                    }
                                    ui.end_row();
                                }
                            });
                        });
                        ui.separator();
                        if ui.button("Reset to theme defaults").clicked() { self.reset(&theme.id); }
                    });
            });
        self.open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::Preferences;

    #[test]
    fn theme_scoped_overrides_reset_and_idle_saves_preserve_other_edits() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("app.ron");
        let (themes, _) = theme::Themes::load(root.path(), "carbon");
        let mut storage = Preferences::open(path.clone(), Arc::new(|| {}));
        storage.set_string("terminal_color.carbon.foreground", "#d0e0ff".into());
        storage.set_string("terminal_color.carbon.ansi.4", "#8888ff".into());
        storage.set_string("terminal_color.carbon.minimum_contrast", "7".into());
        storage.set_string("terminal_color.ice.minimum_contrast", "NaN".into());
        let mut state = State::load(Some(&storage), &themes);
        let carbon = theme::Theme::carbon();
        let ice = theme::Theme::builtin("ice").unwrap();
        assert_eq!(state.palette(&carbon).terminal_minimum_contrast, 7.);
        assert_eq!(
            state.palette(&carbon).terminal_foreground,
            theme::parse_color("#d0e0ff").unwrap()
        );
        assert_eq!(
            state.palette(&carbon).ansi[4],
            theme::parse_color("#8888ff").unwrap()
        );
        assert_eq!(state.palette(&ice), ice.palette);
        assert!(state.pending.is_empty());
        state.reset("carbon");
        assert_eq!(state.palette(&carbon), carbon.palette);
        state.store(&mut storage);
        assert!(state.pending.is_empty());
        assert_eq!(
            State::load(Some(&storage), &themes).palette(&carbon),
            carbon.palette
        );
        // Idle saves must not submit the loaded snapshot a second time.
        storage.set_string("terminal_color.carbon.foreground", "#abcdef".into());
        state.store(&mut storage);
        assert_eq!(
            storage
                .get_string("terminal_color.carbon.foreground")
                .as_deref(),
            Some("#abcdef")
        );
        drop(storage);
    }
}
