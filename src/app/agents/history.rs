//! Native project-scoped history list. The browser owns all asynchronous work;
//! rendering filters only bounded cached metadata and never performs host I/O.
use super::super::{
    icons::{Icon, IconButton},
    layout, ui_probe,
};
use crate::{
    agent::history::{Browser, Entry, Owner},
    theme::Palette,
};
use eframe::egui::{self, Align, Layout, RichText, Sense, Stroke};

#[derive(Default)]
/// History interaction intent returned to the owner after rendering cached entries.
pub(super) struct Action {
    pub refresh: bool,
    pub more: bool,
    pub resume: Option<String>,
    pub running: bool,
    pub close: bool,
    pub delete: Option<Delete>,
}

#[derive(Clone)]
/// Deletion confirmation target: one captured conversation identity or the project history set.
pub(super) enum Delete {
    One { id: String, title: String },
    All,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Availability {
    Current,
    Saved,
    Active,
    Owned(Owner),
}
impl Availability {
    fn of(entry: &Entry, current: Option<&str>, owner: Option<Owner>) -> Self {
        if current == Some(entry.id.as_str()) {
            Self::Current
        } else if let Some(owner) = owner {
            Self::Owned(owner)
        } else if entry.active {
            Self::Active
        } else {
            Self::Saved
        }
    }
    fn label(self, can_resume: bool) -> &'static str {
        match self {
            Self::Current => "Open here · Return to chat",
            Self::Saved if can_resume => "Saved · Restore chat",
            Self::Saved => "Saved · Restore unavailable",
            Self::Active => "Active turn · Use Running",
            Self::Owned(Owner::Attached) => "Open in another window · Use Running",
            Self::Owned(Owner::Detached) => "Running in background · Use Running",
            Self::Owned(Owner::OtherProcess) => "Open in another process · Cannot restore here",
        }
    }
    fn hint(self) -> &'static str {
        match self {
            Self::Current => "Return to the chat already open in this window.",
            Self::Saved => {
                "Restore this saved chat. The backend checks ownership when restoring; another client may still have it open."
            }
            Self::Active => "This chat has an active turn. Use Running to find its session.",
            Self::Owned(Owner::Attached) => {
                "Continue in the RODE window that owns this chat, or detach it there before attaching through Running."
            }
            Self::Owned(Owner::Detached) => {
                "This chat is still open in a background RODE session. Use Running → Attach to continue it."
            }
            Self::Owned(Owner::OtherProcess) => {
                "Another process owns this chat. Find its RODE session in Running, or return to the other client. End that session and refresh History to restore it here."
            }
        }
    }
}

/// Render bounded cached metadata and return explicit refresh/resume/deletion actions.
/// Filtering never queries the host, and busy/current sessions cannot be deleted here.
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
    layout::page(ui, layout::PAGE_WIDTH, |ui| {
        layout::header(
            ui,
            "Chat history",
            "Restore saved chats here. Open chats use Running. Ownership is checked on restore.",
            palette,
            |ui| {
                toolbar(ui, browser, can_resume, confirmation, &mut action, palette);
            },
        );
        let search = ui.add_sized(
            [ui.available_width(), 30.],
            egui::TextEdit::singleline(filter)
                .desired_width(f32::INFINITY)
                .hint_text("Filter loaded conversations…")
                .char_limit(256),
        );
        super::super::ui_probe::button(ui, "agent-history-filter", &search);
        ui.add_space(4.);
        let Some(browser) = browser else {
            ui.label(RichText::new("Loading saved conversations…").color(palette.muted));
            return;
        };
        if !browser.busy && (!browser.can_resume || !browser.diagnostics().is_empty()) {
            // Keep backend diagnostics readable without allowing a
            // long traceback to displace the conversation list.
            egui::Frame::new()
                .fill(palette.raised)
                .inner_margin(8)
                .corner_radius(4)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("history-status")
                        .max_height(96.)
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(&browser.status).wrap().selectable(true));
                        });
                });
        } else {
            ui.horizontal(|ui| {
                if browser.busy {
                    ui.spinner();
                }
                ui.add(
                    egui::Label::new(
                        RichText::new(&browser.status)
                            .size(11.)
                            .color(palette.muted),
                    )
                    .truncate(),
                )
                .on_hover_text(&browser.status);
            });
        }
        if !browser.diagnostics().is_empty() {
            let details = egui::CollapsingHeader::new("Error details")
                .id_salt("history-backend-details")
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("history-backend-diagnostics")
                        .max_height(120.)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(browser.diagnostics())
                                    .wrap()
                                    .selectable(true),
                            );
                        });
                });
            ui_probe::button(ui, "agent-history-error-details", &details.header_response);
            ui_probe::button(
                ui,
                if details.body_response.is_some() {
                    "agent-history-error-expanded"
                } else {
                    "agent-history-error-collapsed"
                },
                &details.header_response,
            );
        }
        if !browser.busy
            && !browser.backend_output().is_empty()
            && (!browser.diagnostics().is_empty() || !browser.can_resume)
        {
            let output = egui::CollapsingHeader::new("Startup warnings and backend output")
                .id_salt("history-backend-output")
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("history-backend-output-scroll")
                        .max_height(120.)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(browser.backend_output())
                                    .wrap()
                                    .selectable(true),
                            );
                        });
                });
            ui_probe::button(ui, "agent-history-backend-output", &output.header_response);
            ui_probe::button(
                ui,
                if output.body_response.is_some() {
                    "agent-history-output-expanded"
                } else {
                    "agent-history-output-collapsed"
                },
                &output.header_response,
            );
        }
        if !can_resume {
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Stop the current turn before resuming or deleting conversations.",
                    )
                    .small()
                    .color(palette.muted),
                )
                .wrap(),
            );
        }
        if !browser.can_delete && !browser.busy && browser.diagnostics().is_empty() {
            ui.add(egui::Label::new(RichText::new("History deletion is unavailable with this backend.").small().color(palette.muted)).wrap())
                            .on_hover_text("ACP agents must advertise session deletion; older Codex versions may need updating.");
        }
        ui.add_space(6.);
        if let Some(selected) = confirmation.clone() {
            confirm(
                ui,
                browser,
                can_resume,
                selected,
                confirmation,
                &mut action,
                palette,
            );
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
                    let availability =
                        Availability::of(entry, current, browser.owners.get(&entry.id).copied());
                    let resumable = can_resume
                        && !browser.busy
                        && (availability != Availability::Saved || browser.can_resume);
                    let deletable = can_resume
                        && browser.can_delete
                        && !browser.busy
                        && availability == Availability::Saved;
                    ui.push_id(&entry.id, |ui| {
                        let (resume, delete) = conversation(
                            ui,
                            entry,
                            resumable,
                            deletable,
                            availability,
                            browser.can_resume,
                            palette,
                        );
                        if resume {
                            match availability {
                                Availability::Current => action.close = true,
                                Availability::Saved => action.resume = Some(entry.id.clone()),
                                _ => action.running = true,
                            }
                        }
                        if delete {
                            *confirmation = Some(Delete::One {
                                id: entry.id.clone(),
                                title: entry.title.clone(),
                            });
                        }
                    });
                    ui.add_space(6.);
                }
                if visible == 0 && !browser.busy && (browser.can_resume || browser.can_delete) {
                    ui.add_space(20.);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new(if query.is_empty() {
                                "No saved conversations"
                            } else {
                                "No matching conversations"
                            })
                            .strong(),
                        );
                        ui.label(
                            RichText::new(if query.is_empty() {
                                "Conversations saved by this agent will appear here."
                            } else {
                                "Try another search or load more conversations."
                            })
                            .small()
                            .color(palette.muted),
                        );
                        if !query.is_empty() && ui.button("Clear filter").clicked() {
                            filter.clear();
                        }
                    });
                    ui.add_space(20.);
                }
                if browser.more {
                    let more = ui.add_enabled(
                        !browser.busy,
                        egui::Button::new("Load more conversations")
                            .min_size(egui::vec2(ui.available_width(), 30.)),
                    );
                    super::super::ui_probe::button(ui, "agent-history-more", &more);
                    if more.clicked() {
                        action.more = true;
                    }
                }
            });
        // A click dispatches host work after rendering. Do not publish the old
        // idle state in that frame as acknowledgement of the new operation.
        if !browser.busy
            && !action.refresh
            && !action.more
            && action.delete.is_none()
            && action.resume.is_none()
            && !action.close
        {
            ui_probe::button(ui, "agent-history-ready", &search);
        }
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
    let delete = ui.add_enabled(
        can_resume && browser.is_some_and(|b| b.can_delete && !b.busy),
        egui::Button::new(RichText::new("Delete all…").color(palette.error)),
    ).on_hover_text("Delete saved chats for this host, project and backend, including unloaded pages. Open and active chats are kept.");
    ui_probe::button(ui, "agent-history-delete-all", &delete);
    if delete.clicked() {
        *confirmation = Some(Delete::All);
    }
    let refresh = ui.add_enabled(
        browser.is_none_or(|b| !b.busy),
        IconButton::new(Icon::Refresh, "Refresh"),
    );
    ui_probe::button(ui, "agent-history-refresh", &refresh);
    if refresh.clicked() {
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
    if browser.is_some_and(|browser| !browser.diagnostics().is_empty()) {
        let running = ui.button("Running agents…");
        ui_probe::button(ui, "agent-history-running", &running);
        if running.clicked() {
            action.running = true;
        }
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
                let cancel = ui.button("Cancel deletion");
                ui_probe::button(ui, "agent-history-cancel-delete", &cancel);
                if cancel.clicked() {
                    *confirmation = None;
                }
                let confirm = ui.add_enabled(can_resume && browser.can_delete && !browser.busy,
                    egui::Button::new(RichText::new(match selected { Delete::All => "Confirm delete all", _ => "Confirm delete chat" }).color(palette.error)),
                );
                ui_probe::button(ui, "agent-history-confirm-delete", &confirm);
                if confirm.clicked() {
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
    availability: Availability,
    can_resume: bool,
    palette: Palette,
) -> (bool, bool) {
    let open = availability == Availability::Current;
    let mut frame = layout::card_frame(palette)
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
                    ui.add(
                        egui::Label::new(
                            RichText::new(availability.label(can_resume))
                                .size(11.)
                                .color(palette.accent),
                        )
                        .truncate(),
                    )
                    .on_hover_text(availability.hint());
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
            .on_hover_text(format!("{}\n{}", entry.title, availability.hint()))
            .on_disabled_hover_text(availability.hint())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        body.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                interaction.is_enabled(),
                format!(
                    "{} {}",
                    match availability {
                        Availability::Current => "Show",
                        Availability::Saved => "Restore",
                        _ => "Find in Running",
                    },
                    single_line(&entry.title)
                ),
            )
        });
        if availability == Availability::Saved {
            super::super::ui_probe::button(&interaction, "Restore conversation", &body);
            super::super::ui_probe::button(
                &interaction,
                &format!("agent-history-restore:{}", entry.id),
                &body,
            );
        } else if availability == Availability::Current {
            super::super::ui_probe::button(&interaction, "Show current conversation", &body);
        }
        resume = body.clicked();
        hovered |= body.hovered() || body.has_focus();
        if body.has_focus() {
            frame.frame.stroke = interaction.visuals().selection.stroke;
        }
        let button = ui
            .add_enabled_ui(deletable, |ui| {
                ui.add_sized([78., 28.], egui::Button::new("Delete…"))
            })
            .inner
            .on_disabled_hover_text(
                "Open and active chats are kept; the backend must support deletion",
            );
        ui_probe::button(ui, &format!("agent-history-delete:{}", entry.id), &button);
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

#[cfg(test)]
mod tests;
