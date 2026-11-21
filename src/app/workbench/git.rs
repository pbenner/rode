//! Bundled git feature implementation.
//!
//! Git commands run on the project host with the user's selected Git policy. UI actions guard dirty
//! buffers before worktree/index/history mutations, and destructive actions require explicit
//! choices. Render from cached results rather than running Git during layout.
use super::*;
mod auth;
mod controller;
mod history;
pub(super) mod identity;
mod jobs;
mod rows;
#[cfg(test)]
mod tests;

#[derive(Clone)]
/// Confirmed Git mutation payload. Stash actions capture both selector and immutable
/// object ID so a refreshed/reordered stash list cannot retarget removal.
pub(super) enum Mutation {
    Command(Vec<String>),
    Stash {
        action: project::StashAction,
        reference: String,
        oid: String,
    },
}
impl From<Vec<String>> for Mutation {
    fn from(args: Vec<String>) -> Self {
        Self::Command(args)
    }
}

/// Cached host Git results and unapplied forms; a single busy gate serializes requests
/// and confirmations retain their original mutation payload.
pub(super) struct State {
    pub(super) host_plan: execution::HostPlan,
    pub(super) git_root: String,
    pub(super) git_busy: bool,
    /// Only an in-flight commit locks the draft that its success will clear.
    pub(super) committing: bool,
    pub(super) git_error: String,
    pub(super) git_output: String,
    pub(super) git_last: Option<Instant>,
    pub(super) branch: String,
    pub(super) files: Vec<project::GitFile>,
    pub(super) git_tab: GitTab,
    pub(super) git_tab_data: String,
    pub(super) commit_message: String,
    pub(super) amend: bool,
    pub(super) diff: Option<Diff>,
    pub(super) show_output: bool,
    pub(super) branch_input: String,
    pub(super) branch_target: String,
    pub(super) stash_message: String,
    pub(super) remote_name: String,
    pub(super) remote_url: String,
    pub(super) tag_input: String,
    pub(super) worktree_path: String,
    pub(super) worktree_branch: String,
    pub(super) worktree_new_branch: bool,
    pub(super) clone_url: String,
    identity: identity::State,
    history: history::State,
    auth: auth::State,
    pub(super) confirm: Option<(String, Mutation)>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            host_plan: Default::default(),
            git_root: String::new(),
            git_busy: false,
            committing: false,
            git_error: String::new(),
            git_output: String::new(),
            git_last: None,
            branch: String::new(),
            files: vec![],
            git_tab: GitTab::Changes,
            git_tab_data: String::new(),
            commit_message: String::new(),
            amend: false,
            diff: None,
            show_output: false,
            branch_input: String::new(),
            branch_target: String::new(),
            stash_message: String::new(),
            remote_name: "origin".into(),
            remote_url: String::new(),
            tag_input: String::new(),
            worktree_path: String::new(),
            worktree_branch: String::new(),
            worktree_new_branch: true,
            clone_url: String::new(),
            identity: Default::default(),
            history: Default::default(),
            auth: Default::default(),
            confirm: None,
        }
    }
}
impl State {
    /// Discard pending secrets when Git is disabled, including late completions.
    pub(super) fn clear_auth(&mut self) {
        self.auth = Default::default();
    }
}

impl EditorApp {
    /// Render cached source-control views and explicit host actions without running Git during layout.
    pub(in crate::app) fn git_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        layout::heading(
            ui,
            "Source control",
            "Review changes and manage this repository",
            palette,
        );
        ui.horizontal_wrapped(|ui| {
            self.git_execution_control(ui, ctx);
            if ui
                .add_enabled(
                    !self.workbench.git.git_busy,
                    crate::keyboard::Button::new("Refresh"),
                )
                .clicked()
            {
                self.refresh_git(ctx);
            }
            for (label, args) in [
                ("Fetch", strings(&["fetch", "--all", "--prune"])),
                ("Pull", strings(&["pull", "--ff-only"])),
                ("Push", strings(&["push"])),
            ] {
                let button = ui.add_enabled(
                    !self.workbench.git.git_busy && !self.workbench.git.git_root.is_empty(),
                    crate::keyboard::Button::new(label),
                );
                ui_probe::button(ui, &format!("git-network-{label}"), &button);
                if button.clicked() {
                    self.git_mutate(args, label, ctx);
                }
            }
            if self.workbench.git.git_busy {
                ui.spinner();
            }
        });
        if !self.workbench.git.git_error.is_empty() {
            ui.colored_label(palette.error, &self.workbench.git.git_error);
            if self.workbench.git.git_tab != GitTab::Settings
                && identity::missing(&self.workbench.git.git_error)
            {
                let configure = ui
                    .add_enabled(
                        !self.workbench.git.git_busy,
                        crate::keyboard::Button::new("Configure Git identity"),
                    )
                    .on_disabled_hover_text("Wait for the current Git request to finish.");
                ui_probe::button(ui, "git-configure-identity", &configure);
                if configure.clicked() {
                    self.workbench.git.git_tab = GitTab::Settings;
                    self.refresh_git_tab(ctx);
                }
            }
            if self.workbench.git.git_root.is_empty() {
                if crate::keyboard::button(ui, "Initialize Git repository").clicked() {
                    self.git_mutate(strings(&["init"]), "Initialize repository", ctx);
                }
                ui.label("Clone into this folder (must be empty)");
                ui.add(crate::keyboard::named(
                    "Clone repository URL",
                    layout::text_input(ui, &mut self.workbench.git.clone_url),
                ));
                if crate::keyboard::button(ui, "Clone repository").clicked()
                    && !self.workbench.git.clone_url.is_empty()
                {
                    self.git_mutate(
                        vec![
                            "clone".into(),
                            "--".into(),
                            self.workbench.git.clone_url.clone(),
                            ".".into(),
                        ],
                        "Clone repository",
                        ctx,
                    );
                }
                return;
            }
        }
        if self.workbench.git.git_root.is_empty() {
            ui.label("Finding repository…");
            return;
        }
        ui.label(
            RichText::new(&self.workbench.git.branch)
                .monospace()
                .color(palette.accent),
        );
        ui.add(
            egui::Label::new(
                RichText::new(&self.workbench.git.git_root)
                    .small()
                    .color(palette.muted),
            )
            .wrap(),
        )
        .on_hover_text(&self.workbench.git.git_root);
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in [
                (GitTab::Changes, "Changes"),
                (GitTab::History, "History"),
                (GitTab::Branches, "Branches"),
                (GitTab::Stash, "Stash"),
                (GitTab::Remotes, "Remotes"),
                (GitTab::Tags, "Tags"),
                (GitTab::Worktrees, "Worktrees"),
                (GitTab::Submodules, "Submodules"),
                (GitTab::Settings, "Settings"),
            ] {
                let button = ui.add_enabled(
                    !self.workbench.git.git_busy,
                    crate::keyboard::Button::selectable(self.workbench.git.git_tab == tab, label),
                );
                if tab == GitTab::Settings {
                    ui_probe::button(ui, "git-tab-settings", &button);
                } else if tab == GitTab::Changes {
                    ui_probe::button(ui, "git-tab-changes", &button);
                } else if tab == GitTab::History {
                    ui_probe::button(ui, "git-tab-history", &button);
                }
                if button.clicked() {
                    self.workbench.git.git_tab = tab;
                    self.workbench.git.git_tab_data.clear();
                    self.refresh_git_tab(ctx);
                }
            }
        });
        ui.add_space(8.);
        // The commit draft is local state. Keep it editable during host refreshes;
        // Changes guards each repository action against overlapping Git requests.
        let changes = self.workbench.git.git_tab == GitTab::Changes;
        ui.add_enabled_ui(changes || !self.workbench.git.git_busy,|ui| {
            let tab = self.workbench.git.git_tab;
            if tab == GitTab::Changes {
                self.git_changes_ui(ui, ctx);
                return;
            }
            if tab == GitTab::Settings {
                self.git_identity_ui(ui, ctx);
                return;
            }
            let title = match tab {
                GitTab::History => "Commit history",
                GitTab::Branches => "Branches",
                GitTab::Stash => "Stashes",
                GitTab::Remotes => "Remotes",
                GitTab::Tags => "Tags",
                GitTab::Worktrees => "Worktrees",
                GitTab::Submodules => "Submodules",
                GitTab::Changes | GitTab::Settings => unreachable!(),
            };
            layout::section(ui, title, palette, |ui| {
            match tab {
                GitTab::Changes | GitTab::Settings => unreachable!(),
                GitTab::History => self.git_history_ui(ui,ctx),
                GitTab::Branches => self.git_branches_ui(ui,ctx),
                GitTab::Stash => self.git_stash_ui(ui,ctx),
                GitTab::Remotes => {
                    ui.monospace(&self.workbench.git.git_tab_data);
                    ui.label("Remote name"); ui.add(crate::keyboard::named("Remote name", layout::text_input(ui, &mut self.workbench.git.remote_name)));
                    ui.label("URL"); ui.add(crate::keyboard::named("Remote URL", layout::text_input(ui, &mut self.workbench.git.remote_url)));
                    ui.horizontal_wrapped(|ui| {
                        if crate::keyboard::button(ui, "Add remote").clicked() { self.git_mutate(vec!["remote".into(),"add".into(),self.workbench.git.remote_name.clone(),self.workbench.git.remote_url.clone()],"Add remote",ctx); }
                        if crate::keyboard::button(ui, "Set URL").clicked() { self.git_mutate(vec!["remote".into(),"set-url".into(),self.workbench.git.remote_name.clone(),self.workbench.git.remote_url.clone()],"Set remote URL",ctx); }
                        if crate::keyboard::button(ui, "Remove").clicked() { self.workbench.git.confirm = Some(("Remove remote configuration?".into(),vec!["remote".into(),"remove".into(),self.workbench.git.remote_name.clone()].into())); }
                        if crate::keyboard::button(ui, "Push + set upstream").clicked() { self.git_mutate(vec!["push".into(),"--set-upstream".into(),self.workbench.git.remote_name.clone(),"HEAD".into()],"Set upstream",ctx); }
                    });
                }
                GitTab::Tags => {
                    for tag in self.workbench.git.git_tab_data.clone().lines() { ui.horizontal_wrapped(|ui| {
                        ui.monospace(tag);
                        if crate::keyboard::small_button(ui, "Checkout").clicked() { self.git_mutate(vec!["switch".into(),"--detach".into(),tag.into()],"Checkout tag",ctx); }
                        if crate::keyboard::small_button(ui, "Delete").clicked() { self.workbench.git.confirm = Some((format!("Delete local tag {tag}?"),vec!["tag".into(),"-d".into(),tag.into()].into())); }
                    }); }
                    ui.add(crate::keyboard::named("Tag name", layout::text_input(ui, &mut self.workbench.git.tag_input)));
                    ui.horizontal_wrapped(|ui| {
                        if crate::keyboard::button(ui, "Create tag at HEAD").clicked() { self.git_mutate(vec!["tag".into(),self.workbench.git.tag_input.clone()],"Create tag",ctx); }
                        if crate::keyboard::button(ui, "Push tags").clicked() { self.git_mutate(strings(&["push","--tags"]),"Push tags",ctx); }
                    });
                }
                GitTab::Worktrees => {
                    let metadata = self.workbench.git.git_tab_data.lines().find_map(|line|line.strip_prefix("worktree ")).map(|root|format!("{root}/.git"));
                    for worktree in self.workbench.git.git_tab_data.clone().split("\n\n") {
                        let Some(path) = worktree.lines().find_map(|line|line.strip_prefix("worktree ")) else { continue; };
                        layout::card(ui, palette, |ui| {
                            ui.monospace(path);
                            if let Some(branch) = worktree.lines().find_map(|line|line.strip_prefix("branch refs/heads/")) { ui.label(branch); }
                            if crate::keyboard::small_button(ui, "Open project").clicked() {
                                let remote = self.workspace.as_ref().is_some_and(|w|w.remote.is_some());
                                let options = self.saved_sandbox_for(path,remote).unwrap_or_else(||self.workspace.as_ref().map(|w|w.sandbox_options.clone()).unwrap_or_default());
                                self.guarded(Action::Project { path:path.into(),remote,options },ctx);
                            }
                            if self.workspace.as_ref().is_some_and(|w|w.sandbox_options.enabled)
                                && let Some(metadata) = &metadata
                                && crate::keyboard::small_button(ui, "Open + share main .git").on_hover_text(format!("Grant read/write access to {metadata} so Git works in the linked worktree sandbox.")).clicked()
                            {
                                let remote = self.workspace.as_ref().is_some_and(|w|w.remote.is_some());
                                let mut options = self.saved_sandbox_for(path,remote).unwrap_or_else(||self.workspace.as_ref().map(|w|w.sandbox_options.clone()).unwrap_or_default());
                                options.enabled = true;
                                options.read_only.retain(|p|p != metadata);
                                if !options.read_write.contains(metadata) { options.read_write.push(metadata.clone()); }
                                self.guarded(Action::Project { path:path.into(),remote,options },ctx);
                            }
                        });
                    }
                    ui.label(RichText::new("Linked worktrees need access to the main repository's .git directory. Grant it in Sandbox access when opening a separate sandbox.").small());
                    ui.label("Worktree path (must be accessible in the sandbox)"); ui.add(crate::keyboard::named("Worktree path", layout::text_input(ui, &mut self.workbench.git.worktree_path)));
                    ui.label("Branch"); ui.add(crate::keyboard::named("Worktree branch", layout::text_input(ui, &mut self.workbench.git.worktree_branch)));
                    crate::keyboard::checkbox(ui, &mut self.workbench.git.worktree_new_branch,"Create new branch");
                    ui.horizontal_wrapped(|ui| {
                        if crate::keyboard::button(ui, "Add worktree").clicked() {
                            let mut args = strings(&["worktree","add"]);
                            if self.workbench.git.worktree_new_branch { args.extend(["-b".into(),self.workbench.git.worktree_branch.clone()]); }
                            args.extend(["--".into(),self.workbench.git.worktree_path.clone()]);
                            if !self.workbench.git.worktree_new_branch && !self.workbench.git.worktree_branch.is_empty() { args.push(self.workbench.git.worktree_branch.clone()); }
                            self.git_mutate(args,"Add worktree",ctx);
                        }
                        if crate::keyboard::button(ui, "Remove worktree").clicked() { self.workbench.git.confirm = Some(("Remove this worktree directory? Git will refuse if it has changes.".into(),vec!["worktree".into(),"remove".into(),"--".into(),self.workbench.git.worktree_path.clone()].into())); }
                        if crate::keyboard::button(ui, "Prune metadata").clicked() { self.git_mutate(strings(&["worktree","prune"]),"Prune worktrees",ctx); }
                    });
                }
                GitTab::Submodules => {
                    ui.monospace(&self.workbench.git.git_tab_data);
                    if crate::keyboard::button(ui, "Initialize / update recursively").clicked() { self.git_mutate(strings(&["submodule","update","--init","--recursive"]),"Update submodules",ctx); }
                    if crate::keyboard::button(ui, "Sync URLs").clicked() { self.git_mutate(strings(&["submodule","sync","--recursive"]),"Sync submodules",ctx); }
                }
            }
            });
        });
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if crate::keyboard::button(ui, "Git output").clicked() {
                self.workbench.git.show_output = true;
            }
            if crate::keyboard::button(ui, "Git terminal")
                .on_hover_text("Interactive authentication and all Git commands")
                .clicked()
            {
                self.send_terminal_command(
                    format!(
                        "cd -- {} && git status",
                        ssh::quote(&self.workbench.git.git_root)
                    ),
                    self.terminal_plan(self.workbench.git_sandbox()),
                    ctx,
                );
            }
        });
        ui.label(RichText::new("HTTPS authentication opens a one-operation credential prompt. Use Git terminal for other interactive authentication.").small().color(palette.muted));
    }

    fn git_execution_control(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let sandboxed = self.workbench.prefs.git_sandbox;
        let active = sandboxed
            && self
                .workspace
                .as_ref()
                .is_some_and(|workspace| workspace.plan.active);
        let label = if active {
            "Git sandbox: on"
        } else {
            "Git sandbox: off"
        };
        let ready = self.project_ready()
            && !self.workbench.git.git_busy
            && !self.workbench.git.auth.open()
            && self.workbench.git.confirm.is_none();
        ui.add_enabled_ui(ready, |ui| {
            let menu = crate::keyboard::menu_button(ui, label, |ui| {
                ui.label("Runs on the project host.");
                for (choice, title) in [(false, "Outside sandbox"), (true, "Use project sandbox")] {
                    let response = crate::keyboard::selectable_label(ui, sandboxed == choice, title);
                    ui_probe::button(ui, if choice { "git-use-sandbox" } else { "git-use-host" }, &response);
                    if response.clicked() {
                        self.set_git_sandbox(choice, ctx);
                        ui.close();
                    }
                }
                if sandboxed && !active {
                    ui.label("The project sandbox is not active. Git has normal host access.");
                }
            });
            ui_probe::button(ui, "git-execution-mode", &menu.response);
            menu.response.on_hover_text("Choose normal host permissions or this project's sandbox for Git. Agent tools keep the project policy.");
        });
    }

    fn set_git_sandbox(&mut self, sandboxed: bool, ctx: &egui::Context) {
        if self.workbench.git.git_busy
            || self.workbench.git.auth.open()
            || self.workbench.git.confirm.is_some()
            || self.workbench.prefs.git_sandbox == sandboxed
        {
            return;
        }
        self.workbench.prefs.git_sandbox = sandboxed;
        self.workbench.draft.git_sandbox = sandboxed;
        self.workbench.save_preferences();
        self.workbench.git.git_root.clear();
        self.workbench.git.files.clear();
        self.workbench.git.diff = None;
        self.workbench.git.git_tab_data.clear();
        self.workbench.git.identity.invalidate();
        self.workbench.git.branch.clear();
        self.workbench.git.git_error.clear();
        self.workbench.git.git_output.clear();
        self.workbench.git.git_last = None;
        self.refresh_git(ctx);
    }
    /// Render bounded working-tree/index rows and defer file-specific actions until selection completes.
    pub(in crate::app) fn git_changes_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.label(
            RichText::new("On-disk changes · save editor buffers before staging")
                .small()
                .color(palette.muted),
        );
        let ready = !self.workbench.git.git_busy;
        let stageable = self
            .workbench
            .git
            .files
            .iter()
            .any(|file| rows::Action::Stage.enabled(file));
        let staged = self
            .workbench
            .git
            .files
            .iter()
            .any(project::GitFile::staged);
        ui.horizontal_wrapped(|ui| {
            let stage = ui
                .add_enabled(
                    ready && stageable,
                    crate::keyboard::Button::new("Stage all"),
                )
                .on_disabled_hover_text(if ready {
                    "No unstaged or untracked changes to stage."
                } else {
                    "Wait for the current Git operation to finish."
                });
            ui_probe::button(ui, "git-stage-all", &stage);
            if stage.clicked() {
                self.git_mutate(strings(&["add", "--all"]), "Stage all", ctx);
            }
            if ui
                .add_enabled(ready && staged, crate::keyboard::Button::new("Unstage all"))
                .on_disabled_hover_text(if ready {
                    "No staged changes to unstage."
                } else {
                    "Wait for the current Git operation to finish."
                })
                .clicked()
            {
                self.git_mutate(self.unstage_args(None), "Unstage all", ctx);
            }
            ui.label(format!("{} changed", self.workbench.git.files.len()));
        });
        let selected = ui
            .add_enabled_ui(ready, |ui| {
                rows::show(ui, &self.workbench.git.files, palette)
            })
            .inner;
        if let Some((index, action)) = selected {
            let file = self.workbench.git.files[index].clone();
            match action {
                rows::Action::Open => self.open_file(
                    format!("{}/{}", self.workbench.git.git_root, file.path),
                    ctx,
                ),
                rows::Action::Diff => self.git_file_diff(&file, false, ctx),
                rows::Action::StagedDiff => self.git_file_diff(&file, true, ctx),
                rows::Action::Stage => self.git_mutate(
                    vec!["add".into(), "--".into(), file.path],
                    "Stage file",
                    ctx,
                ),
                rows::Action::Unstage => {
                    self.git_mutate(self.unstage_args(Some(&file.path)), "Unstage file", ctx)
                }
                rows::Action::Discard => {
                    self.workbench.git.confirm = Some((
                        format!(
                            "Discard unstaged changes to {}? This replaces its on-disk content with the index version.",
                            file.path
                        ),
                        vec![
                            "restore".into(),
                            "--worktree".into(),
                            "--".into(),
                            file.path,
                        ]
                        .into(),
                    ))
                }
            }
        }
        if self.workbench.git.files.is_empty() {
            ui.label("Working tree clean.");
        }
        ui.add_space(8.);
        ui.label("Commit message");
        let message = ui.add_enabled(
            !self.workbench.git.committing,
            crate::keyboard::named(
                "Commit message",
                egui::TextEdit::multiline(&mut self.workbench.git.commit_message)
                    .id(egui::Id::new(("git-commit-message", &self.workbench.key)))
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            ),
        );
        ui_probe::button(ui, "git-commit-message", &message);
        let conflicts = self
            .workbench
            .git
            .files
            .iter()
            .any(project::GitFile::conflict);
        ui.horizontal_wrapped(|ui| {
            // Reserve the button's full height before laying out the checkbox.
            let height = layout::control_height(ui);
            ui.spacing_mut().interact_size.y = height;
            ui.add_enabled(
                !self.workbench.git.committing,
                egui::Checkbox::new(&mut self.workbench.git.amend, "Amend last commit"),
            );
            let hint = if !ready {
                "Wait for the current Git operation to finish."
            } else if conflicts {
                "Resolve and stage all conflicts before committing."
            } else {
                "Enter a commit message."
            };
            let commit = ui
                .add_enabled(
                    ready && !conflicts && !self.workbench.git.commit_message.trim().is_empty(),
                    crate::keyboard::Button::new("Commit staged"),
                )
                .on_disabled_hover_text(hint);
            ui_probe::button(ui, "git-commit", &commit);
            if commit.clicked() {
                let mut args = strings(&["commit", "-m"]);
                args.push(self.workbench.git.commit_message.clone());
                if self.workbench.git.amend {
                    args.push("--amend".into());
                }
                self.git_mutate(args, "Commit", ctx);
            }
        });
        if self
            .workbench
            .git
            .files
            .iter()
            .any(project::GitFile::conflict)
        {
            ui.separator();
            ui.colored_label(
                palette.error,
                "Merge conflicts: open files, edit markers, save, then Mark resolved.",
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(ready, crate::keyboard::Button::new("Continue merge"))
                    .clicked()
                {
                    self.git_mutate(strings(&["merge", "--continue"]), "Continue merge", ctx);
                }
                if ui
                    .add_enabled(ready, crate::keyboard::Button::new("Continue rebase"))
                    .clicked()
                {
                    self.git_mutate(strings(&["rebase", "--continue"]), "Continue rebase", ctx);
                }
                if ui
                    .add_enabled(ready, crate::keyboard::Button::new("Abort merge"))
                    .clicked()
                {
                    self.workbench.git.confirm = Some((
                        "Abort the in-progress merge?".into(),
                        strings(&["merge", "--abort"]).into(),
                    ));
                }
                if ui
                    .add_enabled(ready, crate::keyboard::Button::new("Abort rebase"))
                    .clicked()
                {
                    self.workbench.git.confirm = Some((
                        "Abort the in-progress rebase?".into(),
                        strings(&["rebase", "--abort"]).into(),
                    ));
                }
            });
        }
    }
    /// Request the selected path's staged or working-tree diff under the captured project policy.
    pub(in crate::app) fn git_file_diff(
        &mut self,
        file: &project::GitFile,
        staged: bool,
        ctx: &egui::Context,
    ) {
        let args = if file.index == '?' {
            vec![
                "diff".into(),
                "--no-index".into(),
                "--".into(),
                "/dev/null".into(),
                file.path.clone(),
            ]
        } else {
            let mut args = strings(&["diff", "--no-ext-diff", "--no-textconv"]);
            if staged {
                args.push("--cached".into());
            }
            args.extend(["--".into(), file.path.clone()]);
            if let Some(original) = &file.original {
                args.push(original.clone());
            }
            args
        };
        self.git_command(
            args,
            GitJob::Diff {
                path: file.path.clone(),
                staged,
            },
            None,
            ctx,
        );
    }
    /// Render cached commit metadata and explicit show/history actions.
    pub(in crate::app) fn git_history_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.workbench.git.git_busy && self.workbench.git.git_tab_data.is_empty() {
            ui.label("Loading commit history…");
            return;
        }
        let palette = self.themes.active().palette;
        if let Some((oid, action)) = history::show(
            ui,
            &mut self.workbench.git.history,
            &self.workbench.git.git_tab_data,
            palette,
        ) {
            match action {
                history::Action::Details => {
                    self.git_command(
                        vec![
                            "show".into(),
                            "--no-ext-diff".into(),
                            "--no-textconv".into(),
                            "--format=fuller".into(),
                            "--stat".into(),
                            "--patch".into(),
                            oid,
                        ],
                        GitJob::Show,
                        None,
                        ctx,
                    );
                }
                history::Action::CherryPick => {
                    self.git_mutate(vec!["cherry-pick".into(), oid], "Cherry-pick commit", ctx);
                }
                history::Action::Revert => {
                    self.workbench.git.confirm = Some((
                        format!("Create a new commit reverting {oid}?"),
                        vec!["revert".into(), "--no-edit".into(), oid].into(),
                    ));
                }
            }
        }
        if let Some(path) = self.documents.active().and_then(|d| d.path.clone())
            && crate::keyboard::button(ui, "Blame current file").clicked()
        {
            self.git_command(
                vec!["blame".into(), "--".into(), path],
                GitJob::Show,
                None,
                ctx,
            );
        }
    }
    /// Render cached local/remote branch identities and guarded branch mutations.
    pub(in crate::app) fn git_branches_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        for line in self.workbench.git.git_tab_data.clone().lines() {
            let fields: Vec<_> = line.split('\0').collect();
            if fields.len() < 2 {
                continue;
            }
            let remote = fields
                .get(3)
                .is_some_and(|r| r.starts_with("refs/remotes/"));
            if remote && fields[0].ends_with("/HEAD") {
                continue;
            }
            ui.horizontal_wrapped(|ui| {
                ui.monospace(fields[0]);
                if crate::keyboard::small_button(ui, "Switch").clicked() {
                    let args = if remote {
                        vec!["switch".into(), "--track".into(), fields[0].into()]
                    } else {
                        vec!["switch".into(), fields[0].into()]
                    };
                    self.git_mutate(args, "Switch branch", ctx);
                }
                if crate::keyboard::small_button(ui, "Merge").clicked() {
                    self.git_mutate(
                        vec!["merge".into(), "--no-edit".into(), fields[0].into()],
                        "Merge branch",
                        ctx,
                    );
                }
                if crate::keyboard::small_button(ui, "Rebase onto").clicked() {
                    self.workbench.git.confirm = Some((
                        format!("Rebase the current branch onto {}?", fields[0]),
                        vec!["rebase".into(), fields[0].into()].into(),
                    ));
                }
                if !remote && crate::keyboard::small_button(ui, "Delete").clicked() {
                    self.workbench.git.confirm = Some((
                        format!(
                            "Delete branch {}? Git will refuse if it is not merged.",
                            fields[0]
                        ),
                        vec!["branch".into(), "-d".into(), fields[0].into()].into(),
                    ));
                }
            });
        }
        ui.separator();
        ui.label("New branch name");
        ui.add(crate::keyboard::named(
            "New branch name",
            layout::text_input(ui, &mut self.workbench.git.branch_input),
        ));
        ui.label("Start point (blank: HEAD)");
        ui.add(crate::keyboard::named(
            "Branch start point",
            layout::text_input(ui, &mut self.workbench.git.branch_target),
        ));
        if crate::keyboard::button(ui, "Create and switch").clicked() {
            let mut args = vec![
                "switch".into(),
                "-c".into(),
                self.workbench.git.branch_input.clone(),
            ];
            if !self.workbench.git.branch_target.is_empty() {
                args.push(self.workbench.git.branch_target.clone());
            }
            self.git_mutate(args, "Create branch", ctx);
        }
    }
    /// Render cached stash selector/object identities; destructive operations retain both for confirmation.
    pub(in crate::app) fn git_stash_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add(crate::keyboard::named(
            "Stash message",
            layout::text_input(ui, &mut self.workbench.git.stash_message),
        ));
        if crate::keyboard::button(ui, "Stash changes + untracked files").clicked() {
            self.git_mutate(
                vec![
                    "stash".into(),
                    "push".into(),
                    "-u".into(),
                    "-m".into(),
                    self.workbench.git.stash_message.clone(),
                ],
                "Stash changes",
                ctx,
            );
        }
        for line in self.workbench.git.git_tab_data.clone().lines() {
            let mut fields = line.splitn(3, '\0');
            let (Some(reference), Some(oid), Some(message)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            ui.label(message);
            ui.horizontal(|ui| {
                if crate::keyboard::small_button(ui, "Apply").clicked() {
                    self.git_stash(
                        project::StashAction::Apply,
                        reference.into(),
                        oid.into(),
                        ctx,
                    );
                }
                if crate::keyboard::small_button(ui, "Pop").clicked() {
                    self.git_stash(project::StashAction::Pop, reference.into(), oid.into(), ctx);
                }
                if crate::keyboard::small_button(ui, "Drop").clicked() {
                    self.workbench.git.confirm = Some((
                        format!("Permanently drop {reference}: {message}?"),
                        Mutation::Stash {
                            action: project::StashAction::Drop,
                            reference: reference.into(),
                            oid: oid.into(),
                        },
                    ));
                }
            });
        }
    }
    /// Render cached Git diff/output and confirmation windows belonging to the current tool viewport.
    pub(in crate::app) fn workbench_windows(&mut self, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Git) {
            self.workbench.git.clear_auth();
        }
        if ctx.viewport_id() != self.tool_viewport(ToolTab::Panel(Panel::Git)) {
            return;
        }
        let palette = self.themes.active().palette;
        if let Some(mut diff) = self.workbench.git.diff.take() {
            if diff.open {
                let action = if diff.staged {
                    rows::Action::Unstage
                } else {
                    rows::Action::Stage
                };
                let file = self
                    .workbench
                    .git
                    .files
                    .iter()
                    .find(|f| f.path == diff.path);
                let available = file.is_some_and(|file| action.enabled(file));
                let conflict = file.is_some_and(project::GitFile::conflict);
                egui::Window::new(format!(
                    "{} diff · {}",
                    if diff.staged {
                        "Staged"
                    } else {
                        "Working tree"
                    },
                    diff.path
                ))
                .id(egui::Id::new("git-diff"))
                .open(&mut diff.open)
                .default_size([900., 600.])
                .frame(layout::dialog_frame(&ctx.style(), palette))
                .show(ctx, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                !self.workbench.git.git_busy && available,
                                crate::keyboard::Button::new(if diff.staged {
                                    "Unstage file"
                                } else if conflict {
                                    "Mark resolved"
                                } else {
                                    "Stage file"
                                }),
                            )
                            .clicked()
                        {
                            let args = if diff.staged {
                                self.unstage_args(Some(&diff.path))
                            } else {
                                vec!["add".into(), "--".into(), diff.path.clone()]
                            };
                            self.git_mutate(args, "Stage/unstage file", ctx);
                        }
                        if crate::keyboard::button(ui, "Open file").clicked() {
                            self.open_file(
                                format!("{}/{}", self.workbench.git.git_root, diff.path),
                                ctx,
                            );
                        }
                    });
                    let simple = ![
                        "rename from ",
                        "new file mode ",
                        "deleted file mode ",
                        "Binary files ",
                    ]
                    .iter()
                    .any(|s| diff.text.contains(s));
                    if simple {
                        crate::keyboard::collapsing(ui, "Stage / unstage individual hunks", |ui| {
                            for (label, patch) in project::hunk_patches(&diff.text) {
                                if ui
                                    .add_enabled(
                                        !self.workbench.git.git_busy && available && !conflict,
                                        crate::keyboard::Button::new(format!(
                                            "{} {label}",
                                            if diff.staged { "Unstage" } else { "Stage" }
                                        )),
                                    )
                                    .clicked()
                                {
                                    let mut args = strings(&["apply", "--cached"]);
                                    if diff.staged {
                                        args.push("--reverse".into());
                                    }
                                    args.push("-".into());
                                    self.git_command(
                                        args,
                                        GitJob::Mutate {
                                            label: "Stage hunk".into(),
                                            clear_message: false,
                                        },
                                        Some(patch),
                                        ctx,
                                    );
                                }
                            }
                        });
                    }
                    diff_view(ui, &diff.text, palette);
                });
            }
            self.workbench.git.diff = Some(diff);
        }
        if self.workbench.git.show_output {
            let mut close = false;
            egui::Window::new("Git output / history")
                .open(&mut self.workbench.git.show_output)
                .default_size([850., 580.])
                .frame(layout::dialog_frame(&ctx.style(), palette))
                .show(ctx, |ui| {
                    let output =
                        ui.scope(|ui| diff_view(ui, &self.workbench.git.git_output, palette));
                    ui_probe::button(ui, "git-output-view", &output.response);
                    let dismiss = crate::keyboard::button(ui, "Close");
                    ui_probe::button(ui, "git-output-close", &dismiss);
                    close = dismiss.clicked();
                });
            if close {
                self.workbench.git.show_output = false;
            }
        }
        if let Some((message, mutation)) = self.workbench.git.confirm.clone() {
            egui::Modal::new(egui::Id::new("git-confirm"))
                .frame(layout::dialog_frame(&ctx.style(), palette))
                .show(ctx, |ui| {
                    ui.set_max_width(480.);
                    ui.heading("Confirm Git operation");
                    ui.label(message);
                    ui.horizontal(|ui| {
                        if crate::keyboard::button(ui, "Continue").clicked() {
                            self.workbench.git.confirm = None;
                            match mutation {
                                Mutation::Command(args) => {
                                    self.git_mutate(args, "Requested operation", ctx)
                                }
                                Mutation::Stash {
                                    action,
                                    reference,
                                    oid,
                                } => self.git_stash(action, reference, oid, ctx),
                            }
                        }
                        if crate::keyboard::button(ui, "Cancel").clicked() {
                            self.workbench.git.confirm = None;
                        }
                    });
                });
        }
        self.git_auth_dialog(ctx);
    }
}
