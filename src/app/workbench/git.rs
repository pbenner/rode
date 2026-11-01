//! Bundled git feature implementation.
//!
//! Git commands run on the project host under its actual sandbox. UI actions guard dirty
//! buffers before worktree/index/history mutations, and destructive actions require explicit
//! choices. Render from cached results rather than running Git during layout.
use super::*;

#[derive(Clone)]
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

pub(super) struct State {
    pub(super) git_root: String,
    pub(super) git_busy: bool,
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
    pub(super) confirm: Option<(String, Mutation)>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            git_root: String::new(),
            git_busy: false,
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
            confirm: None,
        }
    }
}

impl EditorApp {
    pub(in crate::app) fn git_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.workbench.git.git_busy, egui::Button::new("Refresh"))
                .clicked()
            {
                self.refresh_git(ctx);
            }
            for (label, args) in [
                ("Fetch", strings(&["fetch", "--all", "--prune"])),
                ("Pull", strings(&["pull", "--ff-only"])),
                ("Push", strings(&["push"])),
            ] {
                if ui
                    .add_enabled(
                        !self.workbench.git.git_busy && !self.workbench.git.git_root.is_empty(),
                        egui::Button::new(label),
                    )
                    .clicked()
                {
                    self.git_mutate(args, label, ctx);
                }
            }
            if self.workbench.git.git_busy {
                ui.spinner();
            }
        });
        if !self.workbench.git.git_error.is_empty() {
            ui.colored_label(palette.error, &self.workbench.git.git_error);
            if self.workbench.git.git_root.is_empty() {
                if ui.button("Initialize Git repository").clicked() {
                    self.git_mutate(strings(&["init"]), "Initialize repository", ctx);
                }
                ui.label("Clone into this folder (must be empty)");
                ui.text_edit_singleline(&mut self.workbench.git.clone_url);
                if ui.button("Clone repository").clicked()
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
            .truncate(),
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
            ] {
                if ui
                    .add_enabled(
                        !self.workbench.git.git_busy,
                        egui::Button::selectable(self.workbench.git.git_tab == tab, label),
                    )
                    .clicked()
                {
                    self.workbench.git.git_tab = tab;
                    self.workbench.git.git_tab_data.clear();
                    self.refresh_git_tab(ctx);
                }
            }
        });
        ui.separator();
        ui.add_enabled_ui(!self.workbench.git.git_busy,|ui| {
            match self.workbench.git.git_tab {
                GitTab::Changes => self.git_changes_ui(ui,ctx),
                GitTab::History => self.git_history_ui(ui,ctx),
                GitTab::Branches => self.git_branches_ui(ui,ctx),
                GitTab::Stash => self.git_stash_ui(ui,ctx),
                GitTab::Remotes => {
                    ui.monospace(&self.workbench.git.git_tab_data);
                    ui.label("Remote name"); ui.text_edit_singleline(&mut self.workbench.git.remote_name);
                    ui.label("URL"); ui.text_edit_singleline(&mut self.workbench.git.remote_url);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Add remote").clicked() { self.git_mutate(vec!["remote".into(),"add".into(),self.workbench.git.remote_name.clone(),self.workbench.git.remote_url.clone()],"Add remote",ctx); }
                        if ui.button("Set URL").clicked() { self.git_mutate(vec!["remote".into(),"set-url".into(),self.workbench.git.remote_name.clone(),self.workbench.git.remote_url.clone()],"Set remote URL",ctx); }
                        if ui.button("Remove").clicked() { self.workbench.git.confirm = Some(("Remove remote configuration?".into(),vec!["remote".into(),"remove".into(),self.workbench.git.remote_name.clone()].into())); }
                        if ui.button("Push + set upstream").clicked() { self.git_mutate(vec!["push".into(),"--set-upstream".into(),self.workbench.git.remote_name.clone(),"HEAD".into()],"Set upstream",ctx); }
                    });
                }
                GitTab::Tags => {
                    for tag in self.workbench.git.git_tab_data.clone().lines() { ui.horizontal_wrapped(|ui| {
                        ui.monospace(tag);
                        if ui.small_button("Checkout").clicked() { self.git_mutate(vec!["switch".into(),"--detach".into(),tag.into()],"Checkout tag",ctx); }
                        if ui.small_button("Delete").clicked() { self.workbench.git.confirm = Some((format!("Delete local tag {tag}?"),vec!["tag".into(),"-d".into(),tag.into()].into())); }
                    }); }
                    ui.text_edit_singleline(&mut self.workbench.git.tag_input);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Create tag at HEAD").clicked() { self.git_mutate(vec!["tag".into(),self.workbench.git.tag_input.clone()],"Create tag",ctx); }
                        if ui.button("Push tags").clicked() { self.git_mutate(strings(&["push","--tags"]),"Push tags",ctx); }
                    });
                }
                GitTab::Worktrees => {
                    let metadata = self.workbench.git.git_tab_data.lines().find_map(|line|line.strip_prefix("worktree ")).map(|root|format!("{root}/.git"));
                    for worktree in self.workbench.git.git_tab_data.clone().split("\n\n") {
                        let Some(path) = worktree.lines().find_map(|line|line.strip_prefix("worktree ")) else { continue; };
                        ui.group(|ui| {
                            ui.monospace(path);
                            if let Some(branch) = worktree.lines().find_map(|line|line.strip_prefix("branch refs/heads/")) { ui.label(branch); }
                            if ui.small_button("Open project").clicked() {
                                let remote = self.workspace.as_ref().is_some_and(|w|w.remote.is_some());
                                let options = self.saved_sandbox_for(path,remote).unwrap_or_else(||self.workspace.as_ref().map(|w|w.sandbox_options.clone()).unwrap_or_default());
                                self.guarded(Action::Project { path:path.into(),remote,options },ctx);
                            }
                            if self.workspace.as_ref().is_some_and(|w|w.sandbox_options.enabled)
                                && let Some(metadata) = &metadata
                                && ui.small_button("Open + share main .git").on_hover_text(format!("Grant read/write access to {metadata} so Git works in the linked worktree sandbox.")).clicked()
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
                    ui.label("Worktree path (must be accessible in the sandbox)"); ui.text_edit_singleline(&mut self.workbench.git.worktree_path);
                    ui.label("Branch"); ui.text_edit_singleline(&mut self.workbench.git.worktree_branch);
                    ui.checkbox(&mut self.workbench.git.worktree_new_branch,"Create new branch");
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Add worktree").clicked() {
                            let mut args = strings(&["worktree","add"]);
                            if self.workbench.git.worktree_new_branch { args.extend(["-b".into(),self.workbench.git.worktree_branch.clone()]); }
                            args.extend(["--".into(),self.workbench.git.worktree_path.clone()]);
                            if !self.workbench.git.worktree_new_branch && !self.workbench.git.worktree_branch.is_empty() { args.push(self.workbench.git.worktree_branch.clone()); }
                            self.git_mutate(args,"Add worktree",ctx);
                        }
                        if ui.button("Remove worktree").clicked() { self.workbench.git.confirm = Some(("Remove this worktree directory? Git will refuse if it has changes.".into(),vec!["worktree".into(),"remove".into(),"--".into(),self.workbench.git.worktree_path.clone()].into())); }
                        if ui.button("Prune metadata").clicked() { self.git_mutate(strings(&["worktree","prune"]),"Prune worktrees",ctx); }
                    });
                }
                GitTab::Submodules => {
                    ui.monospace(&self.workbench.git.git_tab_data);
                    if ui.button("Initialize / update recursively").clicked() { self.git_mutate(strings(&["submodule","update","--init","--recursive"]),"Update submodules",ctx); }
                    if ui.button("Sync URLs").clicked() { self.git_mutate(strings(&["submodule","sync","--recursive"]),"Sync submodules",ctx); }
                }
            }
        });
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if ui.button("Git output").clicked() {
                self.workbench.git.show_output = true;
            }
            if ui
                .button("Git terminal")
                .on_hover_text("Interactive authentication and all Git commands")
                .clicked()
            {
                self.send_project_command(
                    format!(
                        "cd -- {} && git status",
                        ssh::quote(&self.workbench.git.git_root)
                    ),
                    ctx,
                );
            }
        });
        ui.label(RichText::new("Network commands use existing credentials and stop if input is required. Use Git terminal for interactive prompts.").small().color(palette.muted));
    }
    pub(in crate::app) fn git_changes_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.label(
            RichText::new("On-disk changes · save editor buffers before staging")
                .small()
                .color(palette.muted),
        );
        ui.horizontal_wrapped(|ui| {
            if ui.button("Stage all").clicked() {
                self.git_mutate(strings(&["add", "--all"]), "Stage all", ctx);
            }
            if ui.button("Unstage all").clicked() {
                self.git_mutate(self.unstage_args(None), "Unstage all", ctx);
            }
            ui.label(format!("{} changed", self.workbench.git.files.len()));
        });
        let files = self.workbench.git.files.clone();
        let row_height = if ui.available_width() < 360. {
            76.
        } else {
            56.
        };
        egui::ScrollArea::vertical().id_salt("git-files").max_height(310.).show_rows(ui,row_height,files.len().min(10000),|ui,range| {
            for index in range { let file = &files[index]; ui.push_id(index,|ui| {
                ui.set_min_height(row_height);
                let color = if file.conflict() { palette.error } else { palette.accent };
                ui.horizontal(|ui| {
                    ui.monospace(RichText::new(format!("{}{}",file.index,file.worktree)).color(color));
                    if ui.add(egui::Button::new(&file.path).frame(false).truncate()).on_hover_text(format!("{}\nOpen file / resolve conflicts",file.path)).clicked() { self.open_file(format!("{}/{}",self.workbench.git.git_root,file.path),ctx); }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Diff").clicked() { self.git_file_diff(file,false,ctx); }
                    if file.staged() && ui.small_button("Staged diff").clicked() { self.git_file_diff(file,true,ctx); }
                    if ui.small_button(if file.conflict() { "Mark resolved" } else { "Stage" }).clicked() { self.git_mutate(vec!["add".into(),"--".into(),file.path.clone()],"Stage file",ctx); }
                    if file.staged() && ui.small_button("Unstage").clicked() {
                        self.git_mutate(self.unstage_args(Some(&file.path)),"Unstage file",ctx);
                    }
                    if file.index != '?' && ui.small_button("Discard").clicked() { self.workbench.git.confirm = Some((format!("Discard unstaged changes to {}? This replaces its on-disk content with the index version.",file.path),vec!["restore".into(),"--worktree".into(),"--".into(),file.path.clone()].into())); }
                });
            }); }
        });
        if files.is_empty() {
            ui.label("Working tree clean.");
        }
        ui.add_space(8.);
        ui.label("Commit message");
        ui.add(
            egui::TextEdit::multiline(&mut self.workbench.git.commit_message)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.workbench.git.amend, "Amend last commit");
            if ui
                .add_enabled(
                    !self.workbench.git.commit_message.trim().is_empty(),
                    egui::Button::new("Commit staged"),
                )
                .clicked()
            {
                let mut args = strings(&["commit", "-m"]);
                args.push(self.workbench.git.commit_message.clone());
                if self.workbench.git.amend {
                    args.push("--amend".into());
                }
                self.git_mutate(args, "Commit", ctx);
            }
        });
        if files.iter().any(project::GitFile::conflict) {
            ui.separator();
            ui.colored_label(
                palette.error,
                "Merge conflicts: open files, edit markers, save, then Mark resolved.",
            );
            ui.horizontal_wrapped(|ui| {
                if ui.button("Continue merge").clicked() {
                    self.git_mutate(strings(&["merge", "--continue"]), "Continue merge", ctx);
                }
                if ui.button("Continue rebase").clicked() {
                    self.git_mutate(strings(&["rebase", "--continue"]), "Continue rebase", ctx);
                }
                if ui.button("Abort merge").clicked() {
                    self.workbench.git.confirm = Some((
                        "Abort the in-progress merge?".into(),
                        strings(&["merge", "--abort"]).into(),
                    ));
                }
                if ui.button("Abort rebase").clicked() {
                    self.workbench.git.confirm = Some((
                        "Abort the in-progress rebase?".into(),
                        strings(&["rebase", "--abort"]).into(),
                    ));
                }
            });
        }
    }
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
    pub(in crate::app) fn git_history_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let data = self.workbench.git.git_tab_data.clone();
        for line in data.lines() {
            let fields: Vec<_> = line.splitn(5, '\0').collect();
            if fields.len() != 5 {
                continue;
            }
            ui.push_id(fields[0], |ui| {
                if ui
                    .button(format!("{}  {}", fields[1], fields[4]))
                    .on_hover_text(format!("{} · {}", fields[2], fields[3]))
                    .clicked()
                {
                    self.git_command(
                        vec![
                            "show".into(),
                            "--no-ext-diff".into(),
                            "--no-textconv".into(),
                            "--format=fuller".into(),
                            "--stat".into(),
                            "--patch".into(),
                            fields[0].into(),
                        ],
                        GitJob::Show,
                        None,
                        ctx,
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Cherry-pick").clicked() {
                        self.git_mutate(
                            vec!["cherry-pick".into(), fields[0].into()],
                            "Cherry-pick commit",
                            ctx,
                        );
                    }
                    if ui.small_button("Revert").clicked() {
                        self.workbench.git.confirm = Some((
                            format!("Create a new commit reverting {}?", fields[1]),
                            vec!["revert".into(), "--no-edit".into(), fields[0].into()].into(),
                        ));
                    }
                });
            });
        }
        if let Some(path) = self.documents.get(self.active).and_then(|d| d.path.clone())
            && ui.button("Blame current file").clicked()
        {
            self.git_command(
                vec!["blame".into(), "--".into(), path],
                GitJob::Show,
                None,
                ctx,
            );
        }
    }
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
                if ui.small_button("Switch").clicked() {
                    let args = if remote {
                        vec!["switch".into(), "--track".into(), fields[0].into()]
                    } else {
                        vec!["switch".into(), fields[0].into()]
                    };
                    self.git_mutate(args, "Switch branch", ctx);
                }
                if ui.small_button("Merge").clicked() {
                    self.git_mutate(
                        vec!["merge".into(), "--no-edit".into(), fields[0].into()],
                        "Merge branch",
                        ctx,
                    );
                }
                if ui.small_button("Rebase onto").clicked() {
                    self.workbench.git.confirm = Some((
                        format!("Rebase the current branch onto {}?", fields[0]),
                        vec!["rebase".into(), fields[0].into()].into(),
                    ));
                }
                if !remote && ui.small_button("Delete").clicked() {
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
        ui.text_edit_singleline(&mut self.workbench.git.branch_input);
        ui.label("Start point (blank: HEAD)");
        ui.text_edit_singleline(&mut self.workbench.git.branch_target);
        if ui.button("Create and switch").clicked() {
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
    pub(in crate::app) fn git_stash_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.text_edit_singleline(&mut self.workbench.git.stash_message);
        if ui.button("Stash changes + untracked files").clicked() {
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
                if ui.small_button("Apply").clicked() {
                    self.git_stash(
                        project::StashAction::Apply,
                        reference.into(),
                        oid.into(),
                        ctx,
                    );
                }
                if ui.small_button("Pop").clicked() {
                    self.git_stash(project::StashAction::Pop, reference.into(), oid.into(), ctx);
                }
                if ui.small_button("Drop").clicked() {
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
    pub(in crate::app) fn workbench_windows(&mut self, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        if let Some(mut diff) = self.workbench.git.diff.take() {
            if diff.open {
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
                .show(ctx, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                !self.workbench.git.git_busy,
                                egui::Button::new(if diff.staged {
                                    "Unstage file"
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
                        if ui.button("Open file").clicked() {
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
                        ui.collapsing("Stage / unstage individual hunks", |ui| {
                            for (label, patch) in project::hunk_patches(&diff.text) {
                                if ui
                                    .add_enabled(
                                        !self.workbench.git.git_busy,
                                        egui::Button::new(format!(
                                            "{} {label}",
                                            if diff.staged { "Unstage" } else { "Stage" }
                                        )),
                                    )
                                    .clicked()
                                {
                                    if self.documents.iter().any(Document::dirty) {
                                        self.notify(
                                            "Save modified editor buffers before staging hunks.",
                                            true,
                                        );
                                    } else {
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
                            }
                        });
                    }
                    diff_view(ui, &diff.text, palette);
                });
            }
            self.workbench.git.diff = Some(diff);
        }
        if self.workbench.git.show_output {
            egui::Window::new("Git output / history")
                .open(&mut self.workbench.git.show_output)
                .default_size([850., 580.])
                .show(ctx, |ui| {
                    diff_view(ui, &self.workbench.git.git_output, palette);
                });
        }
        if let Some((message, mutation)) = self.workbench.git.confirm.clone() {
            egui::Modal::new(egui::Id::new("git-confirm")).show(ctx, |ui| {
                ui.set_max_width(480.);
                ui.heading("Confirm Git operation");
                ui.label(message);
                ui.horizontal(|ui| {
                    if ui.button("Continue").clicked() {
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
                    if ui.button("Cancel").clicked() {
                        self.workbench.git.confirm = None;
                    }
                });
            });
        }
    }
    pub(in crate::app) fn git_command(
        &mut self,
        args: Vec<String>,
        job: GitJob,
        input: Option<String>,
        ctx: &egui::Context,
    ) {
        if self.workbench.git.git_busy || !self.project_ready() {
            return;
        }
        self.workbench.git.git_busy = true;
        let root = if self.workbench.git.git_root.is_empty() {
            self.root()
        } else {
            self.workbench.git.git_root.clone()
        };
        self.dispatch_tool(
            project::Request::Git { root, args, input },
            ToolJob::Git(job),
            ctx,
        );
    }
    pub(in crate::app) fn refresh_git(&mut self, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Git) {
            return;
        }
        if self.workbench.git.git_root.is_empty() {
            self.git_command(
                strings(&["rev-parse", "--show-toplevel"]),
                GitJob::Root,
                None,
                ctx,
            );
        } else {
            self.git_command(
                strings(&[
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--branch",
                    "--untracked-files=normal",
                ]),
                GitJob::Status,
                None,
                ctx,
            );
        }
    }
    pub(in crate::app) fn refresh_git_tab(&mut self, ctx: &egui::Context) {
        let args = match self.workbench.git.git_tab {
            GitTab::Changes => {
                self.refresh_git(ctx);
                return;
            }
            GitTab::History => strings(&[
                "log",
                "-100",
                "--date=short",
                "--format=%H%x00%h%x00%an%x00%ad%x00%s",
            ]),
            GitTab::Branches => strings(&[
                "for-each-ref",
                "--format=%(refname:short)%00%(objectname:short)%00%(upstream:short)%00%(refname)",
                "refs/heads",
                "refs/remotes",
            ]),
            GitTab::Stash => strings(&["stash", "list", "--format=%gd%x00%H%x00%gs"]),
            GitTab::Remotes => strings(&["remote", "-v"]),
            GitTab::Tags => strings(&["tag", "--list", "--sort=-creatordate"]),
            GitTab::Worktrees => strings(&["worktree", "list", "--porcelain"]),
            GitTab::Submodules => strings(&["submodule", "status", "--recursive"]),
        };
        self.git_command(args, GitJob::Tab, None, ctx);
    }
    fn git_stash(
        &mut self,
        action: project::StashAction,
        reference: String,
        oid: String,
        ctx: &egui::Context,
    ) {
        if self.workbench.git.git_busy || !self.project_ready() {
            return;
        }
        if self.documents.iter().any(Document::dirty) {
            self.notify(
                "Save or close modified editor buffers before this Git operation.",
                true,
            );
            return;
        }
        self.workbench.git.git_busy = true;
        self.dispatch_tool(
            project::Request::GitStash {
                root: self.workbench.git.git_root.clone(),
                action,
                reference,
                oid,
            },
            ToolJob::Git(GitJob::Mutate {
                label: match action {
                    project::StashAction::Apply => "Apply stash",
                    project::StashAction::Pop => "Pop stash",
                    project::StashAction::Drop => "Drop stash",
                }
                .into(),
                clear_message: false,
            }),
            ctx,
        );
    }
    pub(in crate::app) fn git_mutate(
        &mut self,
        args: Vec<String>,
        label: &str,
        ctx: &egui::Context,
    ) {
        let dirty = self.documents.iter().any(Document::dirty);
        if dirty
            && matches!(
                args.first().map(String::as_str),
                Some(
                    "add"
                        | "commit"
                        | "switch"
                        | "merge"
                        | "rebase"
                        | "stash"
                        | "restore"
                        | "pull"
                        | "cherry-pick"
                        | "revert"
                        | "apply"
                        | "reset"
                        | "rm"
                )
            )
        {
            self.notify(
                "Save or close modified editor buffers before this Git operation.",
                true,
            );
            return;
        }
        self.git_command(
            args,
            GitJob::Mutate {
                label: label.into(),
                clear_message: label == "Commit",
            },
            None,
            ctx,
        );
    }
    pub(in crate::app) fn unstage_args(&self, path: Option<&str>) -> Vec<String> {
        if self.workbench.git.branch.starts_with("No commits yet")
            || self.workbench.git.branch.starts_with("Initial commit")
        {
            let mut args = strings(&["rm", "--cached", "-r", "--ignore-unmatch", "--"]);
            args.push(path.unwrap_or(".").into());
            args
        } else if let Some(path) = path {
            vec![
                "restore".into(),
                "--staged".into(),
                "--".into(),
                path.into(),
            ]
        } else {
            strings(&["reset", "--mixed", "HEAD"])
        }
    }
    pub(in crate::app) fn handle_git(
        &mut self,
        job: GitJob,
        result: Result<project::Response, String>,
        ctx: &egui::Context,
    ) {
        self.workbench.git.git_busy = false;
        self.workbench.git.git_last = Some(Instant::now());
        let output = match result {
            Ok(project::Response::Command(output)) => output,
            Err(error) => {
                self.workbench.git.git_error = error;
                return;
            }
            _ => {
                self.workbench.git.git_error = "Invalid Git response".into();
                return;
            }
        };
        let diff_exit = matches!(job, GitJob::Diff { .. }) && output.code == 1;
        if !output.success() && !diff_exit {
            self.workbench.git.git_error = (output.stderr + &output.stdout).trim().into();
            if matches!(job, GitJob::Mutate { .. }) {
                self.workbench.git.git_output = self.workbench.git.git_error.clone();
                self.workbench.git.show_output = true;
                self.reload_clean_documents(ctx);
                self.refresh_git(ctx);
            }
            return;
        }
        self.workbench.git.git_error.clear();
        if matches!(job, GitJob::Diff { .. } | GitJob::Show) {
            ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("terminal-input")));
        }
        match job {
            GitJob::Root => {
                self.workbench.git.git_root = output.stdout.trim_end_matches('\n').into();
                self.refresh_git(ctx);
            }
            GitJob::Status => {
                (self.workbench.git.branch, self.workbench.git.files) =
                    project::parse_status(&output.stdout);
                if self.workbench.git.git_tab != GitTab::Changes {
                    self.refresh_git_tab(ctx);
                }
            }
            GitJob::Tab => self.workbench.git.git_tab_data = output.stdout,
            GitJob::Diff { path, staged } => {
                self.workbench.git.diff = Some(Diff {
                    path,
                    staged,
                    text: output.stdout,
                    open: true,
                })
            }
            GitJob::Show => {
                self.workbench.git.git_output = output.stdout + &output.stderr;
                self.workbench.git.show_output = true;
            }
            GitJob::Mutate {
                label,
                clear_message,
            } => {
                self.status = format!("Git · {label}");
                self.workbench.git.git_output = output.stdout + &output.stderr;
                if clear_message {
                    self.workbench.git.commit_message.clear();
                    self.workbench.git.amend = false;
                }
                self.workbench.git.diff = None;
                self.reload_clean_documents(ctx);
                self.refresh_git(ctx);
            }
        }
    }
}
