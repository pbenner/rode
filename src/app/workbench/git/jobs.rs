//! Git request scheduling and admitted completion effects, independent of panel rendering.
//!
//! Initial commands, confirmations and credential retries share operation admission.
//! Captured project workers retain policy and generation ownership; only acknowledged
//! results refresh views, reload clean buffers or clear a submitted commit draft.
use super::{
    controller::{Admission, Operation},
    history, identity,
};
use crate::{
    app::{
        Document, EditorApp,
        workbench::{Diff, GitJob, GitTab, ToolJob, strings},
    },
    project,
};
use eframe::egui;
use std::time::Instant;

impl EditorApp {
    /// Background refreshes silently defer when unavailable; rejected source/index
    /// mutations explain the dirty-buffer requirement to the user.
    fn git_operation(&mut self, operation: Operation, ctx: &egui::Context) {
        if let Err(error @ Admission::Dirty) = self.dispatch_git(operation, vec![], ctx) {
            self.notify(error.message(), true);
        }
    }

    /// Compose domain admission with the existing policy-bound project worker.
    /// Credential prompts retain their operation when admission rejects a retry.
    pub(super) fn dispatch_git(
        &mut self,
        operation: Operation,
        credentials: Vec<project::git_auth::Credentials>,
        ctx: &egui::Context,
    ) -> Result<(), Admission> {
        let ready = self.project_ready();
        let dirty = self.documents.iter().any(Document::dirty);
        self.workbench.git.admit(&operation, ready, dirty)?;
        let root = if self.workbench.git.git_root.is_empty() {
            self.root()
        } else {
            self.workbench.git.git_root.clone()
        };
        operation.begin_auth(&mut self.workbench.git.auth, &credentials);
        let (request, job) = operation.into_request(root, credentials);
        self.dispatch_tool(request, ToolJob::Git(job), ctx);
        Ok(())
    }

    /// Serialize a literal-argument Git request against the discovered repository root
    /// and return its captured completion intent through the project worker.
    pub(in crate::app) fn git_command(
        &mut self,
        args: Vec<String>,
        job: GitJob,
        input: Option<String>,
        ctx: &egui::Context,
    ) {
        self.git_operation(Operation::command(args, job, input), ctx);
    }

    /// Discover the repository root first, then refresh bounded NUL-delimited status metadata.
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
    /// Queue only the selected Git view's bounded metadata command.
    pub(in crate::app) fn refresh_git_tab(&mut self, ctx: &egui::Context) {
        let args = match self.workbench.git.git_tab {
            GitTab::Settings => {
                self.git_identity_load(ctx);
                return;
            }
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
    /// Dispatch an identity-checked stash operation through common Git admission.
    pub(super) fn git_stash(
        &mut self,
        action: project::StashAction,
        reference: String,
        oid: String,
        ctx: &egui::Context,
    ) {
        self.git_operation(Operation::stash(action, reference, oid), ctx);
    }
    /// Capture a mutation's literal arguments and display label. Its command effect,
    /// including commit-draft acknowledgement, belongs to the operation controller.
    pub(in crate::app) fn git_mutate(
        &mut self,
        args: Vec<String>,
        label: &str,
        ctx: &egui::Context,
    ) {
        self.git_operation(Operation::mutation(args, label), ctx);
    }
    /// Build the unstage command for a normal or unborn branch, optionally limiting it to one path.
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
    /// Apply the matching serialized Git reply, refresh affected cached views and
    /// reload only clean documents after acknowledged mutations.
    pub(in crate::app) fn handle_git(
        &mut self,
        job: GitJob,
        result: Result<project::Response, String>,
        ctx: &egui::Context,
    ) {
        self.workbench.git.git_busy = false;
        self.workbench.git.committing = false;
        self.workbench.git.git_last = Some(Instant::now());
        let (output, challenge) = match result {
            Ok(project::Response::Command(output)) => (output, None),
            Ok(project::Response::GitAuthenticated(reply)) => (reply.output, reply.challenge),
            Err(error) => {
                self.workbench.git.auth.finish(None, ctx);
                self.workbench.git.git_error = match &job {
                    GitJob::Identity(job) => self.workbench.git.identity.failure(job, &error),
                    _ => error,
                };
                return;
            }
            _ => {
                self.workbench.git.auth.finish(None, ctx);
                self.workbench.git.git_error = "Invalid Git response".into();
                return;
            }
        };
        let challenge = (!output.success())
            .then(|| challenge.or_else(|| project::git_auth::challenge(&output.stderr)))
            .flatten();
        self.workbench.git.auth.finish(challenge, ctx);
        if let GitJob::Identity(job) = job {
            self.git_identity_reply(job, output, ctx);
            return;
        }
        let diff_exit = matches!(job, GitJob::Diff { .. }) && output.code == 1;
        let empty_history = matches!(job, GitJob::Tab)
            && self.workbench.git.git_tab == GitTab::History
            && history::empty_error(&output);
        if !output.success() && !diff_exit && !empty_history {
            self.workbench.git.git_error = (output.stderr + &output.stdout).trim().into();
            if matches!(job, GitJob::Mutate { .. }) {
                self.workbench.git.git_output = self.workbench.git.git_error.clone();
                self.workbench.git.show_output = !identity::missing(&self.workbench.git.git_error)
                    && !self.workbench.git.auth.open();
                self.reload_clean_documents(ctx);
                self.refresh_git(ctx);
            }
            return;
        }
        // Polling Changes must not erase a missing-identity error before the
        // user can follow its Settings action. A successful settings request
        // or another successful Git operation clears it.
        if !matches!(job, GitJob::Root | GitJob::Status | GitJob::Tab)
            || !identity::missing(&self.workbench.git.git_error)
        {
            self.workbench.git.git_error.clear();
        }
        if matches!(job, GitJob::Diff { .. } | GitJob::Show) {
            ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("terminal-input")));
        }
        match job {
            GitJob::Identity(_) => unreachable!(),
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
