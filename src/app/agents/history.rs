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
}

pub(super) fn ui(
    ui: &mut egui::Ui,
    browser: Option<&Browser>,
    filter: &mut String,
    can_resume: bool,
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
    if browser.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            if ui.small_button("Cancel").clicked() {
                action.close = true;
            }
        });
    }
    if !can_resume {
        ui.small("Stop the current turn before resuming another conversation.");
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
                    if ui
                        .add_enabled(
                            enabled,
                            egui::Button::new(label)
                                .wrap()
                                .min_size(egui::vec2(ui.available_width(), 28.)),
                        )
                        .on_hover_text(format!("Resume {}", entry.id))
                        .clicked()
                    {
                        action.resume = Some(entry.id.clone());
                    }
                    if !entry.preview.is_empty() && entry.preview != entry.title {
                        ui.add(
                            egui::Label::new(egui::RichText::new(&entry.preview).small()).wrap(),
                        );
                    }
                    if entry.active {
                        ui.small("Active turn");
                    }
                    ui.add_space(5.);
                });
            }
            if browser.more
                && ui
                    .add_enabled(!browser.busy, egui::Button::new("Load more conversations"))
                    .clicked()
            {
                action.more = true;
            }
        });
    action
}
