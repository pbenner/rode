//! A bundled extension for explicit project bookmarks in Connections.
//!
//! The extension preference map is the only persistence owner. UI edits touch
//! individual records so the existing writer can merge concurrent changes.
//! Reopening delegates to the common project/connection guards and preparation.
use super::*;
use crate::extensions::Feature;
mod model;
pub(in crate::app) use model::Bookmark;
pub(super) use model::State;
use model::{Project, Restoration, RestoreStage};

const FEATURE: Feature = Feature::ProjectBookmarks;
const SETTING: &str = "projects";

enum Edit {
    Insert(Bookmark),
    Remove(String),
    Rename(String, String),
    Shares(String, Option<Vec<crate::shared_folders::Config>>),
}

impl EditorApp {
    fn bookmark_settings(&self) -> Option<&serde_json::Value> {
        self.extensions
            .registry
            .preferences
            .get(FEATURE.id())?
            .settings
            .get(SETTING)
    }
    fn sync_bookmarks(&mut self) {
        let raw = self
            .extensions
            .registry
            .preferences
            .get(FEATURE.id())
            .and_then(|p| p.settings.get(SETTING));
        self.project_bookmarks.sync(raw);
    }

    /// Recipes belong to the current SSH project, independently of the transfer-browser host.
    fn bookmark_share_configs(&self) -> Option<Vec<crate::shared_folders::Config>> {
        self.project_shared_folder_endpoint()
            .map(|_| self.shared_folders.configs())
    }

    pub(in crate::app) fn bookmark_candidate(&self) -> Option<Bookmark> {
        let workspace = self.workspace.as_ref().filter(|w| w.ready)?;
        if self.workspace.preparing() || self.vaults.has_active() {
            return None;
        }
        let project = if let Some(remote) = &workspace.remote {
            let mut profile = remote.profile.clone();
            profile.root.clone_from(&workspace.root);
            profile.sandbox = workspace.sandbox_options.clone();
            Project::Ssh {
                profile: Box::new(profile),
            }
        } else {
            Project::Local {
                root: workspace.root.clone(),
                sandbox: workspace.sandbox_options.clone(),
            }
        };
        let mut name: String = ssh::display_name(&workspace.root)
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        while name.len() > 120 {
            name.pop();
        }
        if name.trim().is_empty() {
            name = "Project".into();
        }
        let shared_folders = (matches!(project, Project::Ssh { .. })
            && self.project_bookmarks.include_shares)
            .then(|| self.bookmark_share_configs())
            .flatten()
            .filter(|configs| !configs.is_empty());
        Some(Bookmark {
            name,
            project,
            shared_folders,
        })
    }

    /// Patch raw records, retaining fields introduced by newer versions of the extension.
    fn edit_bookmarks(&mut self, edit: Edit) -> anyhow::Result<()> {
        model::decode(self.bookmark_settings())?;
        let mut raw = self
            .bookmark_settings()
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let map = raw
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("Invalid project bookmarks"))?;
        match edit {
            Edit::Insert(bookmark) => {
                map.insert(bookmark.project.id(), serde_json::to_value(bookmark)?);
            }
            Edit::Remove(id) => {
                map.remove(&id);
            }
            Edit::Shares(id, shares) => {
                let record = map
                    .get_mut(&id)
                    .and_then(serde_json::Value::as_object_mut)
                    .ok_or_else(|| anyhow::anyhow!("Project bookmark no longer exists"))?;
                if let Some(shares) = shares {
                    record.insert("shared_folders".into(), serde_json::to_value(shares)?);
                } else {
                    record.remove("shared_folders");
                }
            }
            Edit::Rename(id, name) => {
                model::validate_name(&name)?;
                let record = map
                    .get_mut(&id)
                    .and_then(serde_json::Value::as_object_mut)
                    .ok_or_else(|| anyhow::anyhow!("Project bookmark no longer exists"))?;
                record.insert("name".into(), serde_json::Value::String(name));
            }
        }
        model::decode(Some(&raw))?;
        let mut preferences = self.extensions.registry.preferences.clone();
        preferences
            .entry(FEATURE.id().into())
            .or_default()
            .settings
            .insert(SETTING.into(), raw);
        crate::extensions::preferences(&serde_json::to_string(&preferences)?)?;
        self.extensions.registry.preferences = preferences;
        self.extensions.registry.pending = true;
        self.sync_bookmarks();
        Ok(())
    }

    /// Toggle the current validated project location in saved preferences; untitled/disconnected
    /// state cannot become a live-session bookmark.
    pub(super) fn toggle_project_bookmark(&mut self) {
        if !self.feature(FEATURE) {
            self.notify("Enable Project bookmarks in View → Extensions.", false);
            return;
        }
        self.sync_bookmarks();
        let Some(bookmark) = self.bookmark_candidate() else {
            self.notify("Open a local or SSH project before bookmarking. Encrypted projects use Open encrypted project.", false);
            return;
        };
        let id = bookmark.project.id();
        let saved = self.project_bookmarks.projects.contains_key(&id);
        let edit = if saved {
            Edit::Remove(id)
        } else {
            Edit::Insert(bookmark)
        };
        match self.edit_bookmarks(edit) {
            Ok(()) => self.notify(
                if saved {
                    "Project bookmark removed"
                } else {
                    "Project bookmarked"
                },
                false,
            ),
            Err(error) => self.notify(format!("Cannot save project bookmark: {error:#}"), true),
        }
    }

    /// Persisted locations are intents, never proof of a live transport or an active sandbox.
    fn reopen_bookmark(&mut self, bookmark: Bookmark, ctx: &egui::Context) {
        self.guarded(Action::Bookmark(Box::new(bookmark)), ctx);
    }

    pub(in crate::app) fn bookmark_is_current(&self, bookmark: &Bookmark) -> bool {
        self.project_ready()
            && self
                .bookmark_candidate()
                .is_some_and(|current| current.project.id() == bookmark.project.id())
    }

    /// Match ordinary SSH recovery: reopening the same disconnected project retains its edits.
    pub(in crate::app) fn bookmark_preserves_buffers(&self, bookmark: &Bookmark) -> bool {
        self.bookmark_is_current(bookmark)
            || matches!(&bookmark.project, Project::Ssh { profile } if self.can_resume_ssh(profile))
    }

    /// A saved location obeys the same encrypted-project barriers as its ordinary open action.
    pub(in crate::app) fn bookmark_transition<'a>(
        &'a self,
        bookmark: &'a Bookmark,
    ) -> vaults::Transition<'a> {
        match &bookmark.project {
            Project::Local { root, .. } => vaults::Transition::Project(root, None),
            Project::Ssh { profile } => {
                let remote = self
                    .workspace
                    .as_ref()
                    .filter(|w| w.connected)
                    .and_then(|w| w.remote.as_ref())
                    .filter(|remote| Project::same_connection(&remote.profile, profile));
                match remote {
                    Some(remote) => vaults::Transition::Project(&profile.root, Some(remote)),
                    None => vaults::Transition::Connection,
                }
            }
        }
    }

    /// Execute an admitted bookmark after dirty-buffer decisions and shared-folder cleanup.
    pub(in crate::app) fn open_bookmark(&mut self, bookmark: Bookmark, ctx: &egui::Context) {
        let current = self.bookmark_is_current(&bookmark);
        if current
            && bookmark
                .shared_folders
                .as_ref()
                .is_some_and(|configs| self.shared_folders.matches_mounted(configs))
        {
            self.project_bookmarks.restore_shares = None;
            return;
        }
        self.project_bookmarks.restore_shares =
            bookmark.shared_folders.clone().map(|configs| Restoration {
                project: bookmark.project.clone(),
                configs,
                stage: if current {
                    RestoreStage::Ready(self.workers.generation())
                } else {
                    RestoreStage::Armed
                },
            });
        if current {
            return;
        }
        match bookmark.project {
            Project::Local { root, sandbox } => {
                let options = self.saved_project_sandbox(&root, None).unwrap_or(sandbox);
                self.prepare_project(
                    root,
                    options,
                    false,
                    false,
                    workspace::PreparationPolicy::Given,
                    ctx,
                );
            }
            Project::Ssh { mut profile } => {
                if let Some(options) = self.saved_project_sandbox(&profile.root, Some(&profile)) {
                    profile.sandbox = options;
                }
                let same_host = self
                    .workspace
                    .as_ref()
                    .filter(|w| w.connected)
                    .and_then(|w| w.remote.as_ref())
                    .is_some_and(|remote| Project::same_connection(&remote.profile, &profile));
                if same_host {
                    self.prepare_project(
                        profile.root,
                        profile.sandbox,
                        true,
                        false,
                        workspace::PreparationPolicy::Given,
                        ctx,
                    );
                } else {
                    let resume = self.can_resume_ssh(&profile);
                    self.connect(*profile, ctx, resume);
                    if let Some(restore) = &mut self.project_bookmarks.restore_shares {
                        restore.stage = RestoreStage::Connecting(self.workers.generation());
                    }
                }
            }
        }
    }

    /// Bind the intent to this exact preparation. A later settings/picker request supersedes it.
    pub(in crate::app) fn bookmark_project_preparing(
        &mut self,
        id: workspace::PreparationId,
        path: &str,
        remote: bool,
    ) {
        let Some(restore) = &self.project_bookmarks.restore_shares else {
            return;
        };
        let armed = match restore.stage {
            RestoreStage::Armed => true,
            RestoreStage::Connecting(generation) => generation == self.workers.generation(),
            RestoreStage::Preparing(_) | RestoreStage::Ready(_) => false,
        };
        let matches = match &restore.project {
            Project::Local { root, .. } => !remote && root == path,
            Project::Ssh { profile } => {
                remote
                    && profile.root == path
                    && self
                        .workspace
                        .as_ref()
                        .and_then(|w| w.remote.as_ref())
                        .is_some_and(|host| Project::same_connection(&host.profile, profile))
            }
        };
        if armed && matches {
            self.project_bookmarks
                .restore_shares
                .as_mut()
                .unwrap()
                .stage = RestoreStage::Preparing(id);
        } else {
            self.project_bookmarks.restore_shares = None;
        }
    }

    /// Only an accepted matching candidate can authorize automatic mounts on the new workspace.
    pub(in crate::app) fn bookmark_project_committed(
        &mut self,
        id: workspace::PreparationId,
        previous: u64,
    ) {
        let Some(restore) = &self.project_bookmarks.restore_shares else {
            return;
        };
        if !matches!(restore.stage, RestoreStage::Preparing(candidate) if candidate == id) {
            return;
        }
        let accepted = previous != self.workers.generation()
            && self
                .bookmark_candidate()
                .is_some_and(|current| current.project.id() == restore.project.id());
        if accepted {
            self.project_bookmarks
                .restore_shares
                .as_mut()
                .unwrap()
                .stage = RestoreStage::Ready(self.workers.generation());
        } else {
            self.project_bookmarks.restore_shares = None;
        }
    }

    /// Explicit reopening restores mounts after the accepted project/host is ready. Loading a
    /// bookmark catalogue alone cannot start workers, and stale requests cannot use another policy.
    pub(in crate::app) fn restore_bookmark_shares(&mut self, ctx: &egui::Context) {
        let Some(restore) = &self.project_bookmarks.restore_shares else {
            return;
        };
        let ready = match restore.stage {
            RestoreStage::Armed => false,
            RestoreStage::Connecting(generation) => {
                let connecting = self.workspace.as_ref().is_some_and(|workspace| {
                    !workspace.ready
                        && workspace.remote.as_ref().is_some_and(|host| {
                            matches!(&restore.project, Project::Ssh { profile }
                                if Project::same_connection(&host.profile, profile))
                        })
                });
                if generation == self.workers.generation() && connecting {
                    return;
                }
                false
            }
            RestoreStage::Preparing(id) => {
                if self.workspace.preparation_id() == Some(id) {
                    return;
                }
                false
            }
            RestoreStage::Ready(generation) => {
                generation == self.workers.generation()
                    && self.project_ready()
                    && self
                        .bookmark_candidate()
                        .is_some_and(|current| current.project.id() == restore.project.id())
            }
        };
        if !ready {
            self.project_bookmarks.restore_shares = None;
            return;
        }
        if !self.feature(Feature::Transfers) {
            self.project_bookmarks.restore_shares = None;
            self.notify(
                "Enable File transfers to restore this bookmark's shared folders.",
                true,
            );
            return;
        }
        if self.pending.as_ref().is_some()
            || self.vaults.locking()
            || self.exit_pending()
            || self.dialog != Dialog::None
            || self.workspace.preparing()
            || self.shared_folders.pending()
            || self.foreign_modal(ctx)
        {
            return;
        }
        let restore = self.project_bookmarks.restore_shares.take().unwrap();
        self.mount_bookmark_shares(restore.configs, ctx);
    }

    /// A native disclosure and independently clickable bookmark action share the header row.
    /// Reserve the action first so long labels truncate without shrinking the full-width list.
    pub(super) fn project_bookmarks_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.vertical(|ui| {
            let id = ui.make_persistent_id(egui::Id::new("project-bookmarks"));
            let mut label = None;
            let mut header = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                true,
            )
            .show_header(ui, |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.project_bookmark_button(ui);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        // Pointer clicks on the title share the disclosure's single keyboard stop.
                        label = Some(
                            ui.add(
                                egui::Label::new("Saved projects")
                                    .selectable(false)
                                    .truncate()
                                    .sense(Sense::CLICK),
                            ),
                        );
                    });
                });
            });
            let label = label.unwrap();
            if label.clicked() {
                header.toggle();
            }
            let (disclosure, _, _) =
                header.body_unindented(|ui| self.project_bookmarks_ui(ui, ctx));
            let response = disclosure.union(label);
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::CollapsingHeader,
                    ui.is_enabled(),
                    "Saved projects",
                )
            });
            keyboard::target(ui, "Saved projects", &response);
            ui_probe::button(ui, "connections-section:Saved projects", &response);
        });
    }

    /// Compact bookmark toggle with the catalogue's existing selection, tooltip and context menu.
    fn project_bookmark_button(&mut self, ui: &mut egui::Ui) {
        if !self.feature(FEATURE) {
            return;
        }
        self.sync_bookmarks();
        let candidate = self.bookmark_candidate();
        let saved = candidate.as_ref().is_some_and(|b| {
            self.project_bookmarks
                .projects
                .contains_key(&b.project.id())
        });
        let enabled = candidate.is_some()
            && self.project_bookmarks.error.is_none()
            && self.dialog == Dialog::None
            && !self.pending.modal();
        let label = if saved {
            "Remove project bookmark"
        } else {
            "Bookmark current project"
        };
        let button = IconButton::toolbar(Icon::Bookmark, label);
        let button = ui.add_enabled(enabled, button.selected(saved));
        let button = button.on_disabled_hover_text(if self.vaults.has_active() {
            "Reopen encrypted projects through Open encrypted project."
        } else {
            "Open a project and wait for it to be ready."
        });
        ui_probe::button(ui, "bookmark-project", &button);
        ui_probe::button(
            ui,
            if saved {
                "project-bookmarked"
            } else {
                "project-not-bookmarked"
            },
            &button,
        );
        if button.clicked() {
            self.toggle_project_bookmark();
        }
        crate::keyboard::context_menu(&button, |ui| {
            ui.add_enabled_ui(
                candidate
                    .as_ref()
                    .is_some_and(|b| matches!(b.project, Project::Ssh { .. })),
                |ui| {
                    crate::keyboard::checkbox(
                        ui,
                        &mut self.project_bookmarks.include_shares,
                        "Include shared-folder settings in new bookmarks",
                    );
                },
            );
            ui.weak("Saved shared folders mount automatically when you reopen the bookmark.");
        });
    }

    /// Render saved local/SSH locations and their management actions in Connections. Reopening uses
    /// normal guarded project/connection transitions.
    pub(super) fn project_bookmarks_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if !self.feature(FEATURE) {
            return;
        }
        self.sync_bookmarks();
        let palette = self.themes.active().palette;
        let mut open = None;
        let mut edit = None;
        let mut rename = None;
        if self.foreign_modal(ctx)
            || self.dialog != Dialog::None
            || self.pending.modal()
            || self.exit_pending()
        {
            ui.disable();
        }
        if let Some(error) = &self.project_bookmarks.error {
            ui.colored_label(palette.error, error);
            ui.small("Existing bookmarks were preserved. Correct the project-bookmarks extension settings to edit this list.");
            return;
        }
        if self.project_bookmarks.projects.is_empty() {
            ui.label(palette.tag("No saved projects yet."));
        }
        let current = self.bookmark_candidate().map(|b| b.project.id());
        let mut projects: Vec<_> = self.project_bookmarks.projects.iter().collect();
        projects.sort_by(|(a_id, a), (b_id, b)| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a_id.cmp(b_id))
        });
        for (id, bookmark) in projects {
            let label = format!("{} · {}", bookmark.name, bookmark.project.host());
            let response = ui
                .add_sized(
                    [ui.available_width(), 30.],
                    IconButton::new(
                        if matches!(bookmark.project, Project::Local { .. }) {
                            Icon::Folder
                        } else {
                            Icon::Remote
                        },
                        &label,
                    )
                    .selected(current.as_ref() == Some(id))
                    .truncate(),
                )
                .on_hover_text(format!(
                    "{}\n{}\nRight-click to rename or remove",
                    bookmark.project.host(),
                    bookmark.project.root()
                ));
            ui_probe::button(ui, &format!("saved-project:{id}"), &response);
            if response.clicked() {
                open = Some(bookmark.clone());
            }
            crate::keyboard::context_menu(&response, |ui| {
                let change_name = crate::keyboard::button(ui, "Rename bookmark…");
                ui_probe::button(ui, &format!("bookmark-rename:{id}"), &change_name);
                if change_name.clicked() {
                    rename = Some((id.clone(), bookmark.name.clone()));
                    ui.close();
                }
                if matches!(bookmark.project, Project::Ssh { .. }) {
                    let mut include = bookmark.shared_folders.is_some();
                    let configs = self.bookmark_share_configs();
                    let can_capture = current.as_ref() == Some(id) && configs.is_some();
                    let include_control = ui
                        .add_enabled_ui(can_capture, |ui| {
                            crate::keyboard::checkbox(
                                ui,
                                &mut include,
                                "Include shared-folder settings",
                            )
                        })
                        .inner;
                    ui_probe::button(ui, &format!("bookmark-shares:{id}"), &include_control);
                    if include_control.changed() {
                        edit = Some(Edit::Shares(
                            id.clone(),
                            if include {
                                self.bookmark_share_configs()
                            } else {
                                None
                            },
                        ));
                    }
                    if include && current.as_ref() == Some(id) {
                        let update = ui.add_enabled(
                            can_capture,
                            crate::keyboard::Button::new("Update saved shared folders"),
                        );
                        ui_probe::button(ui, &format!("bookmark-shares-update:{id}"), &update);
                        if update.clicked() {
                            edit = Some(Edit::Shares(id.clone(), configs));
                            ui.close();
                        }
                    }
                }
                let remove = crate::keyboard::button(
                    ui,
                    RichText::new("Remove bookmark").color(palette.error),
                );
                ui_probe::button(ui, &format!("bookmark-remove:{id}"), &remove);
                if remove.clicked() {
                    edit = Some(Edit::Remove(id.clone()));
                    ui.close();
                }
            });
        }
        if let Some((id, name)) = &mut self.project_bookmarks.rename {
            let field = ui.add(crate::keyboard::named(
                "Project name",
                egui::TextEdit::singleline(name)
                    .hint_text("Project name")
                    .desired_width(ui.available_width()),
            ));
            ui_probe::button(ui, "bookmark-name", &field);
            ui_probe::value(ui, "bookmark-name-value", &field, name);
            if std::mem::take(&mut self.project_bookmarks.rename_focus) {
                field.request_focus();
            }
            let enter = (field.has_focus() || field.lost_focus())
                && ui.input(|i| i.key_pressed(Key::Enter));
            let mut cancel = false;
            ui.horizontal(|ui| {
                let save = ui.add_enabled(
                    model::validate_name(name).is_ok(),
                    crate::keyboard::Button::new("Save name"),
                );
                ui_probe::button(ui, "bookmark-name-save", &save);
                if save.clicked() || (enter && model::validate_name(name).is_ok()) {
                    edit = Some(Edit::Rename(id.clone(), name.trim().into()));
                }
                cancel = crate::keyboard::button(ui, "Cancel").clicked();
            });
            if cancel {
                self.project_bookmarks.rename = None;
            }
        }
        if let Some(form) = rename {
            self.project_bookmarks.rename = Some(form);
            self.project_bookmarks.rename_focus = true;
        }
        if let Some(edit) = edit {
            match self.edit_bookmarks(edit) {
                Ok(()) => self.project_bookmarks.rename = None,
                Err(error) => self.notify(format!("Cannot save project bookmark: {error:#}"), true),
            }
        }
        if let Some(bookmark) = open {
            self.reopen_bookmark(bookmark, ctx);
        }
    }
}

#[cfg(test)]
mod tests;
