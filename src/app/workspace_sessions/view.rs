//! Welcome owns session discovery and explicit intent only. Rendering a row
//! never authenticates SSH, unlocks storage, replays work or changes persistence.
use super::*;
impl EditorApp {
    pub(in crate::app) fn workspace_sessions_ui(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let ctx = ui.ctx().clone();
        let palette = self.themes.active().palette;
        let ready = self.workspace_session_location().is_some();
        let mut selected = None;
        let mut enable = None;
        let mut cancel_restore = false;
        let mut unlock = None;
        let rect = ui.allocate_ui_with_layout(egui::vec2(ui.available_width().min(640.), 0.), egui::Layout::top_down(egui::Align::Min), |ui| {
            let Some(state) = &mut self.workspace_sessions else { return };
            ui.add_space(20.);
            ui.horizontal(|ui| {
                ui.strong("Workspace sessions");
                let refresh = ui.add_enabled(!state.listing, crate::keyboard::Button::new("Refresh"));
                ui_probe::button(ui, "workspace-sessions-refresh", &refresh);
                if refresh.clicked() { state.refresh(); }
            });
            let mut keep = state.own.is_some() || state.enabling;
            let checkbox = ui.add_enabled_ui(ready && !state.enabling && state.restore.is_none(), |ui| crate::keyboard::checkbox(ui, &mut keep, "Keep workspace" )).inner;
            ui_probe::button(ui, "workspace-session-keep", &checkbox);
            if state.own.is_some() && state.submitted.is_none() { ui_probe::button(ui, "workspace-session-kept", &checkbox); }
            if checkbox.changed() { enable = Some(keep); }
            ui.add(egui::Label::new(egui::RichText::new("Remember files, drafts and layout. Running tools keep their own Persist settings.").small().weak()).wrap());
            if state.enabling || state.submitted.is_some() { ui.small("Saving workspace…"); }
            if state.restore.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner(); ui.label("Restoring workspace…");
                    let cancel = crate::keyboard::button(ui, "Cancel restore");
                    ui_probe::button(ui, "workspace-session-cancel-restore", &cancel);
                    if cancel.clicked() { cancel_restore = true; }
                });
            }
            if let Some(restoring) = &state.restore
                && let Location::Encrypted { cipherdir, sandbox, .. } = &restoring.location
                && self.dialog != Dialog::Folder {
                let button = crate::keyboard::button(ui, "Unlock project…");
                ui_probe::button(ui, "workspace-session-unlock", &button);
                if button.clicked() { unlock = Some((cipherdir.clone(), sandbox.clone())); }
            }
            if !state.catalogue_warning.is_empty() { ui.add(egui::Label::new(&state.catalogue_warning).wrap()); }
            if state.failed.is_some() && crate::keyboard::button(ui, "Retry saving workspace").clicked() { state.failed = None; state.error.clear(); }
            if !state.error.is_empty() { ui.add(egui::Label::new(egui::RichText::new(&state.error).color(palette.error)).wrap()); }
            if let Some(confirmation) = &state.confirm {
                let (text, label) = match confirmation {
                    Confirmation::TakeOver(row) => (format!("Take over '{}' from {}? The original window will stop updating this saved workspace. Matching agent control may also move to this window; running work keeps its existing policy.", row.summary.name, row.summary.owner.desktop.label()), "Confirm takeover"),
                    Confirmation::Remove(row) => (format!("Remove '{}' and its saved drafts? Files and running host processes are kept.", row.summary.name), "Remove saved workspace"),
                };
                ui.separator(); ui.add(egui::Label::new(text).wrap());
                ui.horizontal(|ui| {
                    let yes = crate::keyboard::button(ui, label);
                    ui_probe::button(ui, "workspace-session-confirm", &yes);
                    if yes.clicked() { selected = Some(ViewAction::Confirm); }
                    if crate::keyboard::button(ui, "Cancel").clicked() { selected = Some(ViewAction::Cancel); }
                });
            }
            if let Some((key, name)) = &mut state.rename {
                ui.horizontal(|ui| {
                    let edit = ui.add(crate::keyboard::named("Workspace name", egui::TextEdit::singleline(name).char_limit(120).desired_width(230.)));
                    ui_probe::button(ui, "workspace-session-name", &edit);
                    ui_probe::value(ui, "workspace-session-name-value", &edit, name);
                    let save = crate::keyboard::button(ui, "Save name");
                    ui_probe::button(ui, "workspace-session-save-name", &save);
                    if save.clicked() || (edit.has_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))) { selected = Some(ViewAction::Rename(key.clone(), name.clone())); }
                    if crate::keyboard::button(ui, "Cancel").clicked() { selected = Some(ViewAction::Cancel); }
                });
            }
            let desktop = crate::desktop::owner();
            ui.vertical(|ui| {
                if state.rows.is_empty() { ui.small(if state.listing { "Loading saved workspaces…" } else { "No saved workspaces. Enable Keep workspace for this window." }); }
                for row in &state.rows {
                    let own = state.own.as_ref().is_some_and(|o| o.key == row.summary.key());
                    let same_desktop = row.summary.owner.desktop.host == desktop.host && row.summary.owner.desktop.display == desktop.display;
                    ui.push_id(&row.summary.id, |ui| {
                    layout::card_frame(palette).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(egui::RichText::new(&row.summary.name).strong()).truncate());
                            ui.label(palette.tag(if own { "This window" } else if row.active { "Open" } else if row.summary.snapshot.location.encrypted() { "Encrypted" } else if row.summary.snapshot.location.remote() { "Disconnected" } else { "Saved" }));
                        });
                        let location = &row.summary.snapshot.location;
                        ui.add(egui::Label::new(format!("{} · {}", location.host(), location.path())).truncate()).on_hover_text(location.path());
                        let elapsed = model::now().saturating_sub(row.summary.updated);
                        let age = if elapsed < 60 { "Saved just now".into() } else if elapsed < 3600 { format!("Saved {} min ago", elapsed / 60) } else if elapsed < 86400 { format!("Saved {} h ago", elapsed / 3600) } else { format!("Saved {} d ago", elapsed / 86400) };
                        ui.small(age);
                        ui.horizontal_wrapped(|ui| {
                            let label = if own { "Show workspace" } else if row.active && same_desktop { "Switch to window" } else if row.active { "Take over" } else { "Resume" };
                            let response = ui.add_enabled(state.restore.is_none() && !state.enabling, crate::keyboard::Button::new(label));
                            ui_probe::button(ui, &format!("workspace-session-resume:{}", row.summary.id), &response);
                            if response.clicked() { selected = Some(if own { ViewAction::Show } else if row.active && same_desktop { ViewAction::Focus(row.summary.key()) } else if row.active { ViewAction::TakeOver(row.clone()) } else { ViewAction::Resume(row.clone(), false) }); }
                            if row.active && !own && same_desktop {
                                let take = crate::keyboard::button(ui, "Take over"); ui_probe::button(ui, &format!("workspace-session-takeover:{}", row.summary.id), &take);
                                if take.clicked() { selected = Some(ViewAction::TakeOver(row.clone())); }
                            }
                            let rename = crate::keyboard::button(ui, "Rename"); ui_probe::button(ui, &format!("workspace-session-rename:{}", row.summary.id), &rename);
                            if rename.clicked() { selected = Some(ViewAction::BeginRename(row.summary.key(), row.summary.name.clone())); }
                            let remove = ui.add_enabled(!row.active, crate::keyboard::Button::new("Remove")); ui_probe::button(ui, &format!("workspace-session-remove:{}", row.summary.id), &remove);
                            if remove.clicked() { selected = Some(ViewAction::Remove(row.clone())); }
                        });
                    });
                    }); ui.add_space(6.);
                }
            });
        }).response.rect;
        if let Some((path, options)) = unlock {
            self.show_workspace_unlock(&path, options);
        }
        if cancel_restore && let Some(state) = &mut self.workspace_sessions {
            state.restore = None;
            state.release();
        }
        if let Some(keep) = enable {
            if keep {
                self.enable_workspace_session(&ctx);
            } else if let Some(state) = &mut self.workspace_sessions {
                state.release();
                state.refresh();
            }
        }
        if let Some(action) = selected {
            self.workspace_session_action(action, &ctx);
        }
        rect
    }
    fn workspace_session_action(&mut self, action: ViewAction, ctx: &egui::Context) {
        let Some(state) = &mut self.workspace_sessions else {
            return;
        };
        match action {
            ViewAction::Show => self.open_area(areas::Area::Projects, ctx),
            ViewAction::Focus(key) => state.request(Request::Focus(key)),
            ViewAction::TakeOver(row) => state.confirm = Some(Confirmation::TakeOver(row)),
            ViewAction::Remove(row) => state.confirm = Some(Confirmation::Remove(row)),
            ViewAction::BeginRename(key, name) => state.rename = Some((key, name)),
            ViewAction::Rename(key, name) => {
                state.request(Request::Rename(key, name));
                state.rename = None;
            }
            ViewAction::Cancel => {
                state.confirm = None;
                state.rename = None;
            }
            ViewAction::Resume(row, takeover) => self.guarded(
                Action::WorkspaceSession(Box::new(Resume {
                    key: row.summary.key(),
                    location: row.summary.snapshot.location,
                    takeover,
                })),
                ctx,
            ),
            ViewAction::Confirm => match state.confirm.take() {
                Some(Confirmation::TakeOver(row)) => {
                    self.workspace_session_action(ViewAction::Resume(row, true), ctx)
                }
                Some(Confirmation::Remove(row)) => {
                    state.request(Request::Remove(row.summary.key(), row.summary.revision))
                }
                None => {}
            },
        }
    }
}
enum ViewAction {
    Show,
    Focus(Key),
    Resume(Row, bool),
    TakeOver(Row),
    Remove(Row),
    BeginRename(Key, String),
    Rename(Key, String),
    Confirm,
    Cancel,
}
