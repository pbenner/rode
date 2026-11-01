//! Virtualized file rows with stable widths and aligned, responsive action columns.
use crate::{project::GitFile, theme};
use eframe::egui::{self, Align, Button, Layout, RichText, Ui, Vec2};
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Open,
    Diff,
    StagedDiff,
    Stage,
    Unstage,
    Discard,
}

impl Action {
    fn label(self, file: &GitFile) -> &'static str {
        match self {
            Self::Open => "Open file",
            Self::Diff => "Diff",
            Self::StagedDiff => "Staged diff",
            Self::Stage if file.conflict() => "Mark resolved",
            Self::Stage => "Stage",
            Self::Unstage => "Unstage",
            Self::Discard => "Discard",
        }
    }

    fn enabled(self, file: &GitFile) -> bool {
        match self {
            Self::StagedDiff | Self::Unstage => file.staged(),
            Self::Discard => file.index != '?',
            _ => true,
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Open => "Open file / resolve conflicts",
            Self::Diff => "View changes in the working tree",
            Self::StagedDiff => "View staged changes; requires changes in the index",
            Self::Stage => "Stage this file, or mark its conflicts resolved",
            Self::Unstage => "Remove this file's changes from the index",
            Self::Discard => "Discard unstaged changes to a tracked file after confirmation",
        }
    }
}

const ACTIONS: [Action; 5] = [
    Action::Diff,
    Action::StagedDiff,
    Action::Stage,
    Action::Unstage,
    Action::Discard,
];

pub(super) fn show(
    ui: &mut Ui,
    files: &[GitFile],
    palette: theme::Palette,
) -> Option<(usize, Action)> {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let padding = ui.spacing().button_padding;
    let height = (ui.text_style_height(&egui::TextStyle::Button) + 2. * padding.y)
        .max(ui.spacing().interact_size.y);
    // Measure every action, including conflict resolution, independently of the
    // visible files so scrolling never changes the column widths or breakpoint.
    let widths = [
        "Diff",
        "Staged diff",
        "Mark resolved",
        "Unstage",
        "Discard",
        "More",
    ]
    .map(|label| {
        ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(label.into(), font.clone(), palette.text)
                .size()
                .x
                .ceil()
        }) + 2. * padding.x
    });
    let status_width = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                "MM".into(),
                egui::TextStyle::Monospace.resolve(ui.style()),
                palette.text,
            )
            .size()
            .x
            .ceil()
    });
    let mut selected = None;
    ui.scope(|ui| {
        // Reserve scrollbar space even for short lists. Neither the scrollbar
        // nor the rows' contents may resize the list as files enter the viewport.
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        egui::ScrollArea::vertical()
            .id_salt("git-files")
            .auto_shrink([false, true])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .max_height(310.)
            .show_rows(ui, height, files.len().min(10000), |ui, range| {
                let gap = ui.spacing().item_spacing.x;
                let full_width = widths[..5].iter().sum::<f32>();
                let compact = ui.available_width() < status_width + full_width + 6. * gap + 180.;
                let actions_width = if compact {
                    widths[0] + widths[2] + widths[5] + 3. * gap
                } else {
                    full_width + 5. * gap
                };
                let path_width =
                    (ui.available_width() - status_width - gap - actions_width).max(1.);
                for index in range {
                    let file = &files[index];
                    let display_path = file.path.replace(['\n', '\r', '\t'], " ");
                    ui.push_id(&file.path, |ui| {
                        ui.allocate_ui_with_layout(
                            Vec2::new(ui.available_width(), height),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                let color = if file.conflict() {
                                    palette.error
                                } else {
                                    palette.accent
                                };
                                ui.add_sized(
                                    [status_width, height],
                                    egui::Label::new(
                                        RichText::new(format!("{}{}", file.index, file.worktree))
                                            .monospace()
                                            .color(color),
                                    ),
                                );
                                if ui
                                    .add_sized(
                                        [path_width, height],
                                        Button::new((display_path.as_str(), egui::Atom::grow()))
                                            .frame(false)
                                            .truncate(),
                                    )
                                    .on_hover_text(format!(
                                        "{}\n{}",
                                        file.path,
                                        Action::Open.hint()
                                    ))
                                    .clicked()
                                {
                                    selected = Some((index, Action::Open));
                                }
                                for (column, action) in ACTIONS.into_iter().enumerate() {
                                    if compact && !matches!(action, Action::Diff | Action::Stage) {
                                        continue;
                                    }
                                    if ui
                                        .add_enabled(
                                            action.enabled(file),
                                            Button::new(action.label(file))
                                                .min_size(Vec2::new(widths[column], height)),
                                        )
                                        .on_hover_text(action.hint())
                                        .on_disabled_hover_text(action.hint())
                                        .clicked()
                                    {
                                        selected = Some((index, action));
                                    }
                                }
                                if compact {
                                    egui::containers::menu::MenuButton::from_button(
                                        Button::new("More").min_size(Vec2::new(widths[5], height)),
                                    )
                                    .ui(ui, |ui| {
                                        for action in
                                            [Action::StagedDiff, Action::Unstage, Action::Discard]
                                        {
                                            if ui
                                                .add_enabled(
                                                    action.enabled(file),
                                                    Button::new(action.label(file)),
                                                )
                                                .on_hover_text(action.hint())
                                                .on_disabled_hover_text(action.hint())
                                                .clicked()
                                            {
                                                selected = Some((index, action));
                                                ui.close();
                                            }
                                        }
                                    });
                                }
                            },
                        );
                    });
                }
            });
    });
    selected
}
