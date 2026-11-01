//! Native project-scoped history list. The browser owns all asynchronous work;
//! rendering filters only bounded cached metadata and never performs host I/O.
use super::super::icons::{Icon, IconButton};
use crate::{
    agent::history::{Browser, Entry},
    theme::Palette,
};
use eframe::egui::{self, Align, Layout, RichText, Sense, Stroke};

#[derive(Default)]
pub(super) struct Action {
    pub refresh: bool,
    pub more: bool,
    pub resume: Option<String>,
    pub close: bool,
    pub delete: Option<Delete>,
}

#[derive(Clone)]
pub(super) enum Delete {
    One { id: String, title: String },
    All,
}

pub(super) fn ui(
    ui: &mut egui::Ui,
    browser: Option<&Browser>,
    filter: &mut String,
    can_resume: bool,
    current: Option<&str>,
    confirmation: &mut Option<Delete>,
    palette: Palette,
) -> Action {
    let mut action = Action::default();
    // A bounded reading width avoids stretching a short title across an entire
    // detached tool window. The inner layout stays left-aligned at every width.
    ui.with_layout(Layout::top_down(Align::Center), |ui| {
        let width = ui.available_width().min(1000.);
        ui.allocate_ui_with_layout(
            egui::vec2(width, ui.available_height()),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(width);
                egui::Frame::new().inner_margin(12).show(ui, |ui| {
                    if ui.available_width() >= 500. {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Chat history").size(18.).strong());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                toolbar(ui, browser, can_resume, confirmation, &mut action, palette);
                            });
                        });
                    } else {
                        ui.label(RichText::new("Chat history").size(18.).strong());
                        ui.horizontal(|ui| {
                            ui.with_layout(Layout::right_to_left(Align::Center).with_main_wrap(true), |ui| {
                                toolbar(ui, browser, can_resume, confirmation, &mut action, palette);
                            });
                        });
                    }
                    ui.label(RichText::new("Saved conversations in this project").size(12.).color(palette.muted));
                    ui.add_space(8.);
                    ui.add_sized(
                        [ui.available_width(), 30.],
                        egui::TextEdit::singleline(filter)
                            .desired_width(f32::INFINITY)
                            .hint_text("Filter loaded conversations…")
                            .char_limit(256),
                    );
                    ui.add_space(4.);
                    let Some(browser) = browser else {
                        ui.label(RichText::new("Loading saved conversations…").color(palette.muted));
                        return;
                    };
                    if !browser.busy && !browser.can_resume {
                        // Keep backend diagnostics readable without allowing a
                        // long traceback to displace the conversation list.
                        egui::Frame::new().fill(palette.raised).inner_margin(8).corner_radius(4).show(ui, |ui| {
                            egui::ScrollArea::vertical().id_salt("history-status").max_height(96.).show(ui, |ui| {
                                ui.add(egui::Label::new(&browser.status).wrap().selectable(true));
                            });
                        });
                    } else {
                        ui.horizontal(|ui| {
                            if browser.busy { ui.spinner(); }
                            ui.add(egui::Label::new(RichText::new(&browser.status).size(11.).color(palette.muted)).truncate())
                                .on_hover_text(&browser.status);
                        });
                    }
                    if !can_resume {
                        ui.add(egui::Label::new(RichText::new("Stop the current turn before resuming or deleting conversations.").small().color(palette.muted)).wrap());
                    }
                    if !browser.can_delete && !browser.busy {
                        ui.add(egui::Label::new(RichText::new("History deletion is unavailable with this backend.").small().color(palette.muted)).wrap())
                            .on_hover_text("ACP agents must advertise session deletion; older Codex versions may need updating.");
                    }
                    ui.add_space(6.);
                    if let Some(selected) = confirmation.clone() {
                        confirm(ui, browser, can_resume, selected, confirmation, &mut action, palette);
                        ui.add_space(8.);
                    }
                    let query = filter.to_lowercase();
                    egui::ScrollArea::vertical()
                        .id_salt("agent-history-list")
                        .auto_shrink([false, false])
                        .min_scrolled_height(0.)
                        .max_height(ui.available_height().max(1.))
                        .show(ui, |ui| {
                            let mut visible = 0;
                            for entry in &browser.entries {
                                if !query.is_empty()
                                    && !entry.title.to_lowercase().contains(&query)
                                    && !entry.preview.to_lowercase().contains(&query)
                                {
                                    continue;
                                }
                                visible += 1;
                                let resumable = can_resume && browser.can_resume && !browser.busy && !entry.active;
                                let deletable = can_resume && browser.can_delete && !browser.busy && !entry.active && current != Some(entry.id.as_str());
                                ui.push_id(&entry.id, |ui| {
                                    let (resume, delete) = conversation(ui, entry, resumable, deletable, current, palette);
                                    if resume {
                                        action.resume = Some(entry.id.clone());
                                    }
                                    if delete {
                                        *confirmation = Some(Delete::One { id: entry.id.clone(), title: entry.title.clone() });
                                    }
                                });
                                ui.add_space(6.);
                            }
                            if visible == 0 && !browser.busy && (browser.can_resume || browser.can_delete) {
                                ui.add_space(20.);
                                ui.vertical_centered(|ui| {
                                    ui.label(RichText::new(if query.is_empty() { "No saved conversations" } else { "No matching conversations" }).strong());
                                    ui.label(RichText::new(if query.is_empty() {
                                        "Conversations saved by this agent will appear here."
                                    } else {
                                        "Try another search or load more conversations."
                                    }).small().color(palette.muted));
                                    if !query.is_empty() && ui.button("Clear filter").clicked() {
                                        filter.clear();
                                    }
                                });
                                ui.add_space(20.);
                            }
                            if browser.more && ui.add_enabled(
                                !browser.busy,
                                egui::Button::new("Load more conversations").min_size(egui::vec2(ui.available_width(), 30.)),
                            ).clicked() {
                                action.more = true;
                            }
                        });
                });
            },
        );
    });
    action
}

fn toolbar(
    ui: &mut egui::Ui,
    browser: Option<&Browser>,
    can_resume: bool,
    confirmation: &mut Option<Delete>,
    action: &mut Action,
    palette: Palette,
) {
    if ui.add_enabled(
        can_resume && browser.is_some_and(|b| b.can_delete && !b.busy),
        egui::Button::new(RichText::new("Delete all…").color(palette.error)),
    ).on_hover_text("Delete saved chats for this host, project and backend, including unloaded pages. Open and active chats are kept.").clicked() {
        *confirmation = Some(Delete::All);
    }
    if ui
        .add_enabled(
            browser.is_none_or(|b| !b.busy),
            IconButton::new(Icon::Refresh, "Refresh"),
        )
        .clicked()
    {
        action.refresh = true;
    }
    if ui
        .add(IconButton::new(
            Icon::ChevronLeft,
            if browser.is_some_and(|b| b.busy) {
                "Cancel"
            } else {
                "Back to chat"
            },
        ))
        .clicked()
    {
        action.close = true;
    }
}

fn confirm(
    ui: &mut egui::Ui,
    browser: &Browser,
    can_resume: bool,
    selected: Delete,
    confirmation: &mut Option<Delete>,
    action: &mut Action,
    palette: Palette,
) {
    egui::Frame::new().fill(palette.raised).stroke(Stroke::new(1_f32, palette.error.gamma_multiply(0.5)))
        .corner_radius(6).inner_margin(12).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong(match &selected { Delete::One { .. } => "Delete this conversation?", Delete::All => "Delete all saved conversations?" });
            match &selected {
                Delete::One { title, .. } => {
                    ui.add(egui::Label::new(single_line(title)).truncate()).on_hover_text(title);
                }
                Delete::All => {
                    ui.add(egui::Label::new("Includes unloaded pages and archived Codex chats for this project and backend. Open and active chats are kept, regardless of the filter.").wrap());
                }
            }
            ui.add(egui::Label::new(RichText::new("Codex also deletes spawned child chats. This cannot be undone here.").small().color(palette.muted)).wrap());
            ui.add_space(4.);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Cancel deletion").clicked() {
                    *confirmation = None;
                }
                if ui.add_enabled(can_resume && browser.can_delete && !browser.busy,
                    egui::Button::new(RichText::new(match selected { Delete::All => "Confirm delete all", _ => "Confirm delete chat" }).color(palette.error)),
                ).clicked() {
                    action.delete = confirmation.take();
                }
            });
        });
}

fn conversation(
    ui: &mut egui::Ui,
    entry: &Entry,
    resumable: bool,
    deletable: bool,
    current: Option<&str>,
    palette: Palette,
) -> (bool, bool) {
    let open = current == Some(entry.id.as_str());
    let mut frame = egui::Frame::new()
        .fill(if open { palette.active } else { palette.panel })
        .stroke(Stroke::new(
            1_f32,
            if open {
                palette.accent.gamma_multiply(0.6)
            } else {
                palette.border
            },
        ))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .begin(ui);
    let mut resume = false;
    let mut delete = false;
    let mut hovered = false;
    frame.content_ui.horizontal(|ui| {
        // Reserve the action column before laying out text. Long titles and
        // tracebacks must elide instead of pushing Delete beyond the panel.
        let text_width = (ui.available_width() - 78. - ui.spacing().item_spacing.x).max(1.);
        let content = ui.allocate_ui_with_layout(
            egui::vec2(text_width, 0.),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(text_width);
                ui.spacing_mut().item_spacing.y = 3.;
                let title = single_line(
                    entry
                        .title
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("Untitled conversation"),
                );
                ui.add(
                    egui::Label::new(RichText::new(&title).size(14.).strong().color(palette.text))
                        .truncate()
                        .show_tooltip_when_elided(false),
                );
                let preview = single_line(&entry.preview);
                // Backends often use the opening prompt as both title and preview.
                let preview = preview
                    .strip_prefix(&title)
                    .filter(|rest| rest.chars().next().is_none_or(char::is_whitespace))
                    .unwrap_or(&preview)
                    .trim();
                if !preview.is_empty() {
                    ui.add(
                        egui::Label::new(RichText::new(preview).size(12.).color(palette.muted))
                            .truncate()
                            .show_tooltip_when_elided(false),
                    );
                }
                ui.horizontal(|ui| {
                    if entry.active {
                        ui.label(RichText::new("Active turn").size(11.).color(palette.accent));
                    } else if open {
                        ui.label(RichText::new("Open chat").size(11.).color(palette.accent));
                    }
                    if !entry.updated.is_empty() {
                        ui.add(
                            egui::Label::new(
                                RichText::new(single_line(&entry.updated))
                                    .size(11.)
                                    .color(palette.muted),
                            )
                            .truncate(),
                        );
                    }
                });
            },
        );
        // Keep protected conversations readable while disabling their resume target.
        let mut interaction = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("resume-conversation")
                .max_rect(content.response.rect),
        );
        if !resumable {
            interaction.disable();
        }
        let body = interaction
            .interact(content.response.rect, interaction.id(), Sense::click())
            .on_hover_text(&entry.title)
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        body.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                interaction.is_enabled(),
                format!("Resume {}", single_line(&entry.title)),
            )
        });
        resume = body.clicked();
        hovered |= body.hovered() || body.has_focus();
        if body.has_focus() {
            frame.frame.stroke = Stroke::new(1_f32, palette.accent);
        }
        let button = ui
            .add_enabled_ui(deletable, |ui| {
                ui.add_sized([78., 28.], egui::Button::new("Delete…"))
            })
            .inner
            .on_disabled_hover_text(
                "Open and active chats are kept; the backend must support deletion",
            );
        delete = button.clicked();
        hovered |= button.hovered();
    });
    if hovered && !open {
        frame.frame.fill = palette.hover;
    }
    frame.end(ui);
    (resume, delete)
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
