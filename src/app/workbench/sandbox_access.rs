//! Shared sandbox forms with stable path, access and action columns.
use crate::{sandbox, theme::Palette};
use eframe::egui::{self, Align, Layout, RichText};

use super::super::{
    icons::{Icon, IconButton},
    layout,
};

#[cfg(test)]
mod tests;

/// Full settings form; folder and SSH dialogs already render the backend selector.
pub(in crate::app) fn options_ui(
    ui: &mut egui::Ui,
    options: &mut sandbox::Options,
    palette: Palette,
) {
    super::sandbox_backend_ui(ui, options);
    ui.add_space(8.);
    rules_ui(ui, options, palette);
}

/// Edit grants without duplicating the enclosing dialog's sandbox/backend controls.
/// Forms sharing a modal must supply distinct parent UI scopes for edit history.
pub(in crate::app) fn rules_ui(
    ui: &mut egui::Ui,
    options: &mut sandbox::Options,
    palette: Palette,
) {
    ui.add_enabled_ui(options.enabled, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut options.share_network, "Allow network access");
            ui.checkbox(&mut options.project_read_only, "Project is read only");
        });
        ui.add_space(8.);
        let columns = Columns::new(ui);
        list(ui, "Built-in shared paths", palette, columns, |ui| {
            for (index, (path, default)) in sandbox::default_mounts().into_iter().enumerate() {
                let mut access = options.overrides.get(&path).copied().unwrap_or(default);
                ui.push_id(("access", &path), |ui| {
                    row(ui, index, columns.height, palette, |ui| {
                        cell_label(ui, columns.path_width(ui), columns.height, RichText::new(&path).monospace())
                            .on_hover_text(format!("{path}\nDefault: {}", default.label()));
                        access_picker(ui, &mut access, columns, true);
                        ui.allocate_space(egui::vec2(columns.action, columns.height));
                    });
                });
                if access == default {
                    options.overrides.remove(&path);
                } else {
                    options.overrides.insert(path, access);
                }
            }
        });
        ui.add_space(12.);
        // Storage groups paths by access. Keep display order and widget identities
        // independent of those groups, including while adding or removing rows.
        let draft_id = ui.id().with("sandbox-extra-paths");
        let mut draft = ui.data_mut(|data| data.get_temp::<ExtraPaths>(draft_id)).unwrap_or_default();
        draft.sync(options);
        let mut remove = None;
        list(ui, "Additional shared paths", palette, columns, |ui| {
            if draft.rows.is_empty() {
                layout::subtitle(ui, "No additional paths. Add a folder or file to share it with the project.", palette);
            }
            for (index, grant) in draft.rows.iter_mut().enumerate() {
                // Stable child scope also preserves text undo/cursor and button
                // focus when a preceding row is removed.
                ui.scope_builder(egui::UiBuilder::new().id(draft_id.with(grant.serial)), |ui| {
                    row(ui, index, columns.height, palette, |ui| {
                        ui.add_sized(
                            [columns.path_width(ui), columns.height],
                            egui::TextEdit::singleline(&mut grant.path).font(egui::TextStyle::Monospace)
                                .desired_width(0.).vertical_align(Align::Center).hint_text("/path or ~/folder"),
                        ).on_hover_text(grant.path.as_str());
                        access_picker(ui, &mut grant.access, columns, false);
                        if ui.add_sized(
                            [columns.action, columns.height],
                            IconButton::icon(Icon::Close, "Remove path grant").icon_size(14.),
                        ).clicked() {
                            remove = Some(index);
                        }
                    });
                });
            }
        });
        if let Some(index) = remove {
            draft.rows.remove(index);
        }
        ui.add_space(6.);
        if ui.button("+ Add folder or file").clicked() {
            draft.push(String::new(), sandbox::Access::ReadOnly);
        }
        ui.small("Add any custom shell setup, tool installation or cache needed on this host. Paths are literal (absolute or ~/); use Read only for configuration and Read + write for tool state.");
        draft.publish(options);
        ui.data_mut(|data| data.insert_temp(draft_id, draft));
        ui.add_space(8.);
        layout::subtitle(ui, match options.backend {
            sandbox::Backend::Bwrap => "Private /tmp and hidden home. Unavailable bwrap is visibly normal mode. Shared writable caches retain their contents and access.",
            sandbox::Backend::Landlock => "Requires Linux Landlock ABI 4+. ABI 5 adds device-ioctl restrictions; ABI 6 adds signal and abstract Unix socket isolation. Restricts file access, not host process visibility or file metadata. Uses private project scratch via TMPDIR; cannot hide names or subtract child grants. Unavailable enforcement is visibly normal mode.",
        }, palette);
    });
}

/// Private, per-form state only; persisted options retain their existing format.
#[derive(Clone, Default)]
struct ExtraPaths {
    rows: Vec<ExtraPath>,
    next_serial: u64,
    published: (Vec<String>, Vec<String>),
}

#[derive(Clone)]
struct ExtraPath {
    serial: u64,
    path: String,
    access: sandbox::Access,
}

impl ExtraPaths {
    fn sync(&mut self, options: &sandbox::Options) {
        if self.published.0 == options.read_only && self.published.1 == options.read_write {
            return;
        }
        // A profile switch or reset replaces the draft. Fresh IDs prevent undo
        // history from a previous policy being applied to the replacement paths.
        self.rows.clear();
        for path in &options.read_only {
            self.push(path.clone(), sandbox::Access::ReadOnly);
        }
        for path in &options.read_write {
            self.push(path.clone(), sandbox::Access::ReadWrite);
        }
    }

    fn push(&mut self, path: String, access: sandbox::Access) {
        self.rows.push(ExtraPath {
            serial: self.next_serial,
            path,
            access,
        });
        self.next_serial += 1;
    }

    fn publish(&mut self, options: &mut sandbox::Options) {
        options.read_only = self
            .rows
            .iter()
            .filter(|row| row.access == sandbox::Access::ReadOnly)
            .map(|row| row.path.clone())
            .collect();
        options.read_write = self
            .rows
            .iter()
            .filter(|row| row.access == sandbox::Access::ReadWrite)
            .map(|row| row.path.clone())
            .collect();
        self.published = (options.read_only.clone(), options.read_write.clone());
    }
}

#[derive(Clone, Copy)]
struct Columns {
    access: f32,
    action: f32,
    height: f32,
}

impl Columns {
    fn new(ui: &egui::Ui) -> Self {
        let font = egui::TextStyle::Button.resolve(ui.style());
        let spacing = ui.spacing();
        let text = ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap("Read + write".into(), font, ui.visuals().text_color())
                .size()
                .x
                .ceil()
        });
        let height = ui
            .text_style_height(&egui::TextStyle::Button)
            .max(spacing.icon_width)
            + 2. * spacing.button_padding.y;
        Self {
            access: text
                + spacing.icon_width
                + spacing.icon_spacing
                + 2. * spacing.button_padding.x,
            action: height.max(26.),
            height: height.max(spacing.interact_size.y),
        }
    }

    fn path_width(self, ui: &egui::Ui) -> f32 {
        (ui.available_width() - self.access - self.action - 2. * ui.spacing().item_spacing.x)
            .max(1.)
    }
}

fn list(
    ui: &mut egui::Ui,
    title: &str,
    palette: Palette,
    columns: Columns,
    contents: impl FnOnce(&mut egui::Ui),
) {
    ui.label(RichText::new(title).strong());
    ui.add_space(4.);
    layout::card_frame(palette).inner_margin(6).show(ui, |ui| {
        ui.set_width(ui.available_width().max(1.));
        ui.spacing_mut().item_spacing.y = 2.;
        row(ui, 0, 18., palette, |ui| {
            cell_label(ui, columns.path_width(ui), 18., palette.tag("PATH"));
            cell_label(ui, columns.access, 18., palette.tag("ACCESS"));
            ui.allocate_space(egui::vec2(columns.action, 18.));
        });
        contents(ui);
    });
}

fn row(
    ui: &mut egui::Ui,
    index: usize,
    height: f32,
    palette: Palette,
    contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(if index % 2 == 1 {
            palette.background
        } else {
            egui::Color32::TRANSPARENT
        })
        .inner_margin(egui::Margin::symmetric(6, 4))
        .corner_radius(3)
        .show(ui, |ui| {
            ui.set_width(ui.available_width().max(1.));
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), height),
                Layout::left_to_right(Align::Center),
                contents,
            );
        });
}

fn cell_label(ui: &mut egui::Ui, width: f32, height: f32, text: RichText) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(width, height));
            ui.add(egui::Label::new(text).truncate())
        },
    )
    .inner
}

fn access_picker(
    ui: &mut egui::Ui,
    access: &mut sandbox::Access,
    columns: Columns,
    allow_hidden: bool,
) {
    ui.allocate_ui_with_layout(
        egui::vec2(columns.access, columns.height),
        Layout::top_down(Align::Min),
        |ui| {
            ui.set_min_size(egui::vec2(columns.access, columns.height));
            egui::ComboBox::from_id_salt("mode")
                .width(columns.access)
                .truncate()
                .selected_text(access.label())
                .show_ui(ui, |ui| {
                    for value in [
                        sandbox::Access::Hidden,
                        sandbox::Access::ReadOnly,
                        sandbox::Access::ReadWrite,
                    ] {
                        if allow_hidden || value != sandbox::Access::Hidden {
                            ui.selectable_value(access, value, value.label());
                        }
                    }
                });
        },
    );
}
