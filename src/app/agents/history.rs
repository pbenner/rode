//! Native project-scoped history list. The browser owns all asynchronous work;
//! rendering filters only bounded cached metadata and never performs host I/O.
use crate::agent::history::Browser;
use eframe::egui;

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
) -> Action {
    let mut action = Action::default();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Chat history").strong());
        if ui
            .small_button("Back")
            .on_hover_text("Return to the current chat")
            .clicked()
        {
            action.close = true;
        }
        if ui
            .add_enabled(
                browser.is_none_or(|b| !b.busy),
                egui::Button::new("Refresh").small(),
            )
            .clicked()
        {
            action.refresh = true;
        }
        if ui.add_enabled(
            can_resume && browser.is_some_and(|b| b.can_delete && !b.busy),
            egui::Button::new("Delete all…").small(),
        ).on_hover_text("Delete saved chats for this host, project and backend, including unloaded pages. Open and active chats are kept.").clicked() {
            *confirmation = Some(Delete::All);
        }
    });
    ui.add(
        egui::TextEdit::singleline(filter)
            .desired_width(f32::INFINITY)
            .hint_text("Filter loaded conversations…")
            .char_limit(256),
    );
    let Some(browser) = browser else {
        ui.label("Open or refresh history to read saved conversations.");
        return action;
    };
    ui.add(egui::Label::new(&browser.status).wrap().selectable(true));
    if let Some(selected) = confirmation.clone() {
        ui.group(|ui| {
            ui.label(match &selected {
                Delete::One { title, .. } => format!("Delete “{title}” from backend history?"),
                Delete::All => "Delete all saved chats for this project and backend? This includes unloaded pages and archived Codex chats; open and active chats are kept. The text filter does not limit this action.".into(),
            });
            ui.small("Codex also deletes spawned child chats. This cannot be undone here.");
            ui.horizontal(|ui| {
                if ui.small_button("Cancel deletion").clicked() {
                    *confirmation = None;
                }
                if ui.add_enabled(
                    can_resume && browser.can_delete && !browser.busy,
                    egui::Button::new(match selected { Delete::All => "Confirm delete all", _ => "Confirm delete chat" }),
                ).clicked() {
                    action.delete = confirmation.take();
                }
            });
        });
    }
    if browser.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            if ui.small_button("Cancel").clicked() {
                action.close = true;
            }
        });
    }
    if !can_resume {
        ui.small("Stop the current turn before resuming or deleting conversations.");
    }
    if !browser.can_delete && !browser.busy {
        ui.small("History deletion is unavailable with this backend. ACP agents must advertise session deletion; older Codex versions may need updating.");
    }
    let query = filter.to_lowercase();
    egui::ScrollArea::vertical()
        .id_salt("agent-history-list")
        .auto_shrink([false, false])
        .max_height(ui.available_height().max(1.))
        .show(ui, |ui| {
            for entry in &browser.entries {
                if !query.is_empty()
                    && !entry.title.to_lowercase().contains(&query)
                    && !entry.preview.to_lowercase().contains(&query)
                {
                    continue;
                }
                ui.push_id(&entry.id, |ui| {
                    let label = if entry.updated.is_empty() {
                        entry.title.clone()
                    } else {
                        format!("{} · {}", entry.title, entry.updated)
                    };
                    let enabled =
                        can_resume && browser.can_resume && !browser.busy && !entry.active;
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - 65.).max(1.);
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new(label)
                                    .wrap()
                                    .min_size(egui::vec2(width, 28.)),
                            )
                            .on_hover_text(format!("Resume {}", entry.id))
                            .clicked()
                        {
                            action.resume = Some(entry.id.clone());
                        }
                        if ui
                            .add_enabled(
                                can_resume
                                    && browser.can_delete
                                    && !browser.busy
                                    && !entry.active
                                    && current != Some(entry.id.as_str()),
                                egui::Button::new("Delete…").small(),
                            )
                            .on_disabled_hover_text(
                                "Open and active chats are kept; the backend must support deletion",
                            )
                            .clicked()
                        {
                            *confirmation = Some(Delete::One {
                                id: entry.id.clone(),
                                title: entry.title.clone(),
                            });
                        }
                    });
                    if !entry.preview.is_empty() && entry.preview != entry.title {
                        ui.add(
                            egui::Label::new(egui::RichText::new(&entry.preview).small()).wrap(),
                        );
                    }
                    if entry.active {
                        ui.small("Active turn");
                    } else if current == Some(entry.id.as_str()) {
                        ui.small("Open chat");
                    }
                    ui.add_space(5.);
                });
            }
            if browser.more
                && ui
                    .add_enabled(
                        !browser.busy,
                        egui::Button::new("Load more conversations")
                            .min_size(egui::vec2(ui.available_width(), 24.)),
                    )
                    .clicked()
            {
                action.more = true;
            }
        });
    action
}
