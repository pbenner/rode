//! Bounded, virtualized commit rows with stable action columns and readable metadata.
//! Rendering returns an immutable commit identity; Git mutations remain with the owner.
use crate::{
    app::{layout, ui_probe},
    theme::Palette,
};
use eframe::egui::{self, Align, Button, Layout, RichText, Sense, Ui, Vec2};

/// Retained commit filter and selected commit identity for the history view.
#[derive(Default)]
pub(super) struct State {
    filter: String,
    selected: Option<String>,
}

/// User intent for one selected commit; the owning workbench validates and dispatches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Details,
    CherryPick,
    Revert,
}

#[derive(Clone, Copy)]
struct Commit<'a> {
    oid: &'a str,
    short: &'a str,
    author: &'a str,
    date: &'a str,
    subject: &'a str,
}
impl<'a> Commit<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        let mut fields = line.splitn(5, '\0');
        let commit = Self {
            oid: fields.next()?,
            short: fields.next()?,
            author: fields.next()?,
            date: fields.next()?,
            subject: fields.next()?,
        };
        ((commit.oid.len() == 40 || commit.oid.len() == 64)
            && commit.oid.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some(commit)
    }
    fn matches(self, query: &str) -> bool {
        query.is_empty()
            || [self.oid, self.author, self.date, self.subject]
                .into_iter()
                .any(|value| value.to_lowercase().contains(query))
    }
}

/// Git log exits unsuccessfully on an unborn branch; present that known case
/// as an empty history, while retaining other Git failures.
pub(super) fn empty_error(output: &crate::project::Output) -> bool {
    output.code == 128 && output.stderr.contains("does not have any commits yet")
}

/// Render parsed commit rows with filtering and return an optional commit/action intent.
/// This view neither invokes Git nor changes repository history.
pub(super) fn show(
    ui: &mut Ui,
    state: &mut State,
    data: &str,
    palette: Palette,
) -> Option<(String, Action)> {
    let commits: Vec<_> = data.lines().filter_map(Commit::parse).collect();
    let filter = ui.add(
        egui::TextEdit::singleline(&mut state.filter)
            .id_salt("git-history-filter")
            .desired_width(f32::INFINITY)
            .hint_text("Filter recent commits by message, author or hash")
            .char_limit(256),
    );
    ui_probe::button(ui, "git-history-filter", &filter);
    let query = state.filter.trim().to_lowercase();
    let visible: Vec<_> = commits
        .iter()
        .filter(|commit| commit.matches(&query))
        .collect();
    ui.label(
        RichText::new(if query.is_empty() {
            format!(
                "{} recent commit{} · newest first",
                commits.len(),
                if commits.len() == 1 { "" } else { "s" }
            )
        } else {
            format!("{} of {} recent commits", visible.len(), commits.len())
        })
        .small()
        .color(palette.muted),
    );
    if commits.is_empty() {
        ui.label("No commits yet. Your first commit will appear here.");
        return None;
    }
    if visible.is_empty() {
        ui.label("No recent commits match this filter.");
        return None;
    }
    let height = layout::control_height(ui);
    let row_height = height + ui.text_style_height(&egui::TextStyle::Small) + 14.;
    let widths = ["Details", "More"].map(|label| {
        ui.painter()
            .layout_no_wrap(
                label.into(),
                egui::TextStyle::Button.resolve(ui.style()),
                palette.text,
            )
            .size()
            .x
            .ceil()
            + 2. * ui.spacing().button_padding.x
    });
    let mut selected = None;
    let max_height = (ui.available_height() - 32.).clamp(130., 440.);
    ui.scope(|ui| {
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("git-history-list")
            .auto_shrink([false, true])
            .max_height(max_height)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible);
        if filter.changed() {
            scroll = scroll.vertical_scroll_offset(0.);
        }
        scroll.show_rows(ui, row_height, visible.len(), |ui, range| {
            for index in range {
                let commit = visible[index];
                let action = ui
                    .push_id(commit.oid, |ui| {
                        row(
                            ui,
                            commit,
                            palette,
                            row_height,
                            height,
                            widths,
                            state.selected.as_deref() == Some(commit.oid),
                        )
                    })
                    .inner;
                if let Some(action) = action {
                    state.selected = Some(commit.oid.into());
                    selected = Some((commit.oid.into(), action));
                }
            }
        });
    });
    selected
}

fn row(
    ui: &mut Ui,
    commit: &Commit<'_>,
    palette: Palette,
    row_height: f32,
    height: f32,
    widths: [f32; 2],
    selected: bool,
) -> Option<Action> {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), row_height), Sense::hover());
    let fill = if selected {
        palette.active
    } else if response.hovered() {
        palette.raised
    } else {
        palette.panel
    };
    ui.painter().rect_filled(
        rect,
        ui.visuals().widgets.noninteractive.corner_radius,
        fill,
    );
    let mut action = None;
    let inner = rect.shrink2(egui::vec2(8., 6.));
    let tooltip = format!(
        "{}\n{} · {}\n{}",
        commit.subject, commit.author, commit.date, commit.oid
    );
    let gap = ui.spacing().item_spacing.x;
    let actions_width = widths.iter().sum::<f32>() + gap;
    let text_rect = egui::Rect::from_min_max(
        inner.min,
        egui::pos2(
            (inner.right() - actions_width - gap).max(inner.left() + 1.),
            inner.bottom(),
        ),
    );
    let actions_rect = egui::Rect::from_min_max(
        egui::pos2(inner.right() - actions_width, inner.top()),
        inner.max,
    );
    // Anchor actions independently of text's minimum layout size, so a long
    // subject or author cannot move this row's buttons out of their column.
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            let text_width = text_rect.width();
            ui.set_width(text_width);
            ui.spacing_mut().item_spacing.y = 2.;
            let subject = if commit.subject.trim().is_empty() {
                "(No commit subject)"
            } else {
                commit.subject
            };
            if ui
                .add_sized(
                    [text_width, height],
                    Button::new((RichText::new(subject).strong(), egui::Atom::grow()))
                        .frame(false)
                        .truncate(),
                )
                .on_hover_text(&tooltip)
                .clicked()
            {
                action = Some(Action::Details);
            }
            let metadata = format!("{} · {} · {}", commit.short, commit.author, commit.date);
            ui.add(
                egui::Label::new(RichText::new(metadata).small().color(palette.muted))
                    .truncate()
                    .show_tooltip_when_elided(false),
            )
            .on_hover_text(&tooltip);
        },
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(actions_rect)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            let details = ui
                .add_sized([widths[0], height], Button::new("Details"))
                .on_hover_text("View this commit's message, changed files and patch");
            ui_probe::button(ui, &format!("git-history-details-{}", commit.oid), &details);
            if details.clicked() {
                action = Some(Action::Details);
            }
            let more = egui::containers::menu::MenuButton::from_button(
                Button::new("More").min_size(Vec2::new(widths[1], height)),
            )
            .ui(ui, |ui| {
                let copy = ui.button("Copy commit hash");
                ui_probe::button(ui, "git-history-copy-hash", &copy);
                if copy.clicked() {
                    ui.ctx().copy_text(commit.oid.into());
                    ui.close();
                }
                ui.separator();
                if ui
                    .button("Cherry-pick")
                    .on_hover_text("Apply this commit to the current branch")
                    .clicked()
                {
                    action = Some(Action::CherryPick);
                    ui.close();
                }
                if ui
                    .button("Revert…")
                    .on_hover_text("Create a new commit reverting this change, after confirmation")
                    .clicked()
                {
                    action = Some(Action::Revert);
                    ui.close();
                }
            });
            ui_probe::button(ui, &format!("git-history-more-{}", commit.oid), &more.0);
        },
    );
    action
}

#[cfg(test)]
mod tests;
