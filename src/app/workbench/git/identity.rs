//! Git identity forms and serialized config operations on the captured project host.
//! Git owns config discovery/locking; arguments remain data and the project
//! execution mode applies to repository and global writes alike.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Scope {
    #[default]
    Repository,
    Global,
}
impl Scope {
    fn flag(self) -> &'static str {
        match self {
            Self::Repository => "--local",
            Self::Global => "--global",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Repository => "This repository",
            Self::Global => "Global on this host",
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::app) enum Job {
    Load,
    Name { scope: Scope, email: Option<String> },
    Email { scope: Scope, unset: bool },
}
impl Job {
    fn failure(&self, detail: &str) -> String {
        match self {
            Self::Load => format!("Cannot read Git identity: {detail}"),
            Self::Name { .. } => format!("Cannot update Git name; email was not changed: {detail}"),
            Self::Email { unset: true, .. } => format!(
                "Git name is unset in this scope, but email could not be removed. Retry Use inherited identity: {detail}"
            ),
            Self::Email { unset: false, .. } => format!(
                "Git name was updated, but email was not. Retry saving the identity: {detail}"
            ),
        }
    }
}

#[derive(Default)]
struct Values {
    name: Option<String>,
    email: Option<String>,
}
#[derive(Default)]
struct Snapshot {
    configured: Values,
    repository: Values,
    global: Values,
    overrides: bool,
}
impl Snapshot {
    fn parse(text: &str) -> anyhow::Result<Self> {
        let mut result = Self::default();
        let mut fields = text.split_terminator('\0');
        while let Some(scope) = fields.next() {
            let (key, value) = fields
                .next()
                .and_then(|v| v.split_once('\n'))
                .ok_or_else(|| anyhow::anyhow!("Invalid Git identity response"))?;
            if matches!(
                key,
                "author.name" | "author.email" | "committer.name" | "committer.email"
            ) {
                result.overrides = true;
                continue;
            }
            let field = match key {
                "user.name" => &mut result.configured.name,
                "user.email" => &mut result.configured.email,
                _ => anyhow::bail!("Unexpected Git identity key"),
            };
            *field = Some(value.into());
            let scoped = match scope {
                "local" => &mut result.repository,
                "global" => &mut result.global,
                "worktree" | "command" => {
                    result.overrides = true;
                    continue;
                }
                "system" => continue,
                _ => anyhow::bail!("Unexpected Git identity scope"),
            };
            *if key == "user.name" {
                &mut scoped.name
            } else {
                &mut scoped.email
            } = Some(value.into());
        }
        Ok(result)
    }
    fn draft(&self, scope: Scope) -> (String, String) {
        match scope {
            Scope::Repository => (
                self.repository
                    .name
                    .as_ref()
                    .or(self.configured.name.as_ref())
                    .cloned()
                    .unwrap_or_default(),
                self.repository
                    .email
                    .as_ref()
                    .or(self.configured.email.as_ref())
                    .cloned()
                    .unwrap_or_default(),
            ),
            Scope::Global => (
                self.global.name.clone().unwrap_or_default(),
                self.global.email.clone().unwrap_or_default(),
            ),
        }
    }
}

#[derive(Default)]
pub(super) struct State {
    scope: Scope,
    name: String,
    email: String,
    snapshot: Option<Snapshot>,
    dirty: bool,
    notice: String,
}
impl State {
    pub(super) fn invalidate(&mut self) {
        self.snapshot = None;
        self.notice.clear();
    }

    pub(super) fn failure(&mut self, job: &Job, detail: &str) -> String {
        self.notice.clear();
        job.failure(detail)
    }

    fn select_scope(&mut self, scope: Scope) {
        if self.scope != scope {
            self.scope = scope;
            // Scope chooses where to save. Keep an edited identity when the
            // user changes that destination; Discard edits loads saved values.
            if !self.dirty {
                self.reset_draft();
            }
            self.notice.clear();
        }
    }

    fn reset_draft(&mut self) {
        if let Some(snapshot) = &self.snapshot {
            (self.name, self.email) = snapshot.draft(self.scope);
        }
        self.dirty = false;
    }
    fn values(&self) -> anyhow::Result<(String, String)> {
        let name = self.name.trim();
        let email = self.email.trim();
        anyhow::ensure!(
            !name.is_empty() && !email.is_empty(),
            "Enter both a name and email address."
        );
        anyhow::ensure!(
            name.len() <= 4096 && email.len() <= 4096,
            "Name and email must each fit within 4 KiB."
        );
        anyhow::ensure!(
            !name
                .chars()
                .chain(email.chars())
                .any(|c| c.is_control() || matches!(c, '<' | '>')),
            "Name and email must be single lines without angle brackets."
        );
        anyhow::ensure!(
            !email.chars().any(char::is_whitespace),
            "The email address cannot contain spaces."
        );
        Ok((name.into(), email.into()))
    }
}

/// Match Git's C-locale identity diagnostics, without treating unrelated commit
/// failures (hooks, conflicts, signing or authentication) as identity failures.
pub(super) fn missing(error: &str) -> bool {
    let text = error.to_ascii_lowercase();
    [
        "author identity unknown",
        "committer identity unknown",
        "unable to auto-detect email address",
        "empty ident name",
    ]
    .iter()
    .any(|message| text.contains(message))
}

fn read_args() -> Vec<String> {
    strings(&[
        "config",
        "--null",
        "--includes",
        "--show-scope",
        "--get-regexp",
        r"^(user|author|committer)\.(name|email)$",
    ])
}
fn write_args(scope: Scope, key: &str, value: Option<&str>) -> Vec<String> {
    let mut args = strings(&[
        "config",
        scope.flag(),
        if value.is_some() {
            "--replace-all"
        } else {
            "--unset-all"
        },
        "--",
        key,
    ]);
    if let Some(value) = value {
        args.push(value.into());
    }
    args
}

impl EditorApp {
    pub(super) fn git_identity_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let host = self
            .workspace
            .as_ref()
            .and_then(|w| w.remote.as_ref())
            .map(|remote| format!("SSH host: {}", remote.profile.endpoint()))
            .unwrap_or_else(|| "Local host".into());
        let mut save = false;
        let mut inherit = false;
        let sandboxed =
            self.workbench.git_sandbox() && self.workspace.as_ref().is_some_and(|w| w.plan.active);
        layout::section(ui, "Commit identity", palette, |ui| {
            let state = &mut self.workbench.git.identity;
            ui.label(host);
            ui.small("Name and email recorded in future commits. These settings do not change authentication or existing commits.");
            if let Some(snapshot) = &state.snapshot {
                ui.label(format!(
                    "Configured default: {} <{}>",
                    snapshot.configured.name.as_deref().unwrap_or("not set"),
                    snapshot.configured.email.as_deref().unwrap_or("not set")
                ));
                if snapshot.overrides {
                    ui.small("Additional author, committer or worktree configuration overrides this default. Inspect it in Git terminal if needed.");
                }
            }
            ui.horizontal_wrapped(|ui| {
                for (scope, name) in [
                    (Scope::Repository, "git-identity-repository"),
                    (Scope::Global, "git-identity-global"),
                ] {
                    let response = ui.selectable_label(state.scope == scope, scope.label());
                    ui_probe::button(ui, name, &response);
                    if response.clicked() {
                        state.select_scope(scope);
                    }
                }
            });
            ui.small(match state.scope {
                Scope::Repository => "Stored in this repository's Git configuration. Other repositories keep their settings.",
                Scope::Global => "Applies to all repositories for your account on this host. Repository settings take precedence.",
            });
            ui.label("Name");
            let name = ui.add(
                egui::TextEdit::singleline(&mut state.name)
                    .desired_width(f32::INFINITY)
                    .char_limit(1024),
            );
            if name.changed() {
                state.dirty = true;
                state.notice.clear();
            }
            ui_probe::button(ui, "git-identity-name", &name);
            ui.label("Email");
            let email = ui.add(
                egui::TextEdit::singleline(&mut state.email)
                    .desired_width(f32::INFINITY)
                    .char_limit(1024),
            );
            if email.changed() {
                state.dirty = true;
                state.notice.clear();
            }
            ui_probe::button(ui, "git-identity-email", &email);
            let values = state.values();
            ui.horizontal_wrapped(|ui| {
                let button = ui.add_enabled(state.snapshot.is_some() && values.is_ok(), egui::Button::new("Save identity"))
                    .on_disabled_hover_text(values.as_ref().err().map(ToString::to_string).unwrap_or_else(|| "Wait for Git settings to load.".into()));
                ui_probe::button(ui, "git-identity-save", &button);
                save = button.clicked();
                let inherited = ui.add_enabled(state.snapshot.is_some(), egui::Button::new("Use inherited identity"))
                    .on_hover_text("Remove name and email from the selected scope. Git will use values from its other configuration scopes.");
                ui_probe::button(ui, "git-identity-inherit", &inherited);
                inherit = inherited.clicked();
                if ui.button("Discard edits").clicked() { state.reset_draft(); }
            });
            if state.dirty {
                ui.small("Unapplied identity changes");
            }
            if !state.notice.is_empty() {
                ui.label(&state.notice);
            }
            if state.scope == Scope::Global && sandboxed {
                ui.small("If a sandbox blocks saving, grant write access to your Git configuration or use repository scope.");
            }
        });
        if save {
            self.git_identity_save(false, ctx);
        }
        if inherit {
            self.git_identity_save(true, ctx);
        }
    }

    pub(super) fn git_identity_load(&mut self, ctx: &egui::Context) {
        self.git_command(read_args(), GitJob::Identity(Job::Load), None, ctx);
    }

    fn git_identity_save(&mut self, inherit: bool, ctx: &egui::Context) {
        let state = &mut self.workbench.git.identity;
        let (name, email) = if inherit {
            (None, None)
        } else {
            match state.values() {
                Ok((name, email)) => (Some(name), Some(email)),
                Err(error) => {
                    self.workbench.git.git_error = error.to_string();
                    return;
                }
            }
        };
        state.notice.clear();
        state.dirty = true;
        let scope = state.scope;
        self.git_command(
            write_args(scope, "user.name", name.as_deref()),
            GitJob::Identity(Job::Name { scope, email }),
            None,
            ctx,
        );
    }

    /// Git changes one key per invocation. Keep the busy gate across both writes,
    /// report a partial save honestly, and never clear the staged commit draft.
    pub(super) fn git_identity_reply(
        &mut self,
        job: Job,
        output: project::Output,
        ctx: &egui::Context,
    ) {
        let absent = matches!(job, Job::Load) && output.code == 1;
        let unset = match &job {
            Job::Name { email, .. } => email.is_none(),
            Job::Email { unset, .. } => *unset,
            Job::Load => false,
        };
        if !output.success() && !absent && !(unset && output.code == 5) {
            self.workbench.git.git_error = self
                .workbench
                .git
                .identity
                .failure(&job, &(output.stderr + &output.stdout));
            return;
        }
        self.workbench.git.git_error.clear();
        match job {
            Job::Load => match Snapshot::parse(&output.stdout) {
                Ok(snapshot) => {
                    let state = &mut self.workbench.git.identity;
                    state.snapshot = Some(snapshot);
                    if !state.dirty {
                        state.reset_draft();
                    }
                }
                Err(error) => self.workbench.git.git_error = error.to_string(),
            },
            Job::Name { scope, email } => {
                self.workbench.git.identity.notice = if email.is_none() {
                    "Removing identity"
                } else {
                    "Saving identity"
                }
                .into();
                let unset = email.is_none();
                self.git_command(
                    write_args(scope, "user.email", email.as_deref()),
                    GitJob::Identity(Job::Email { scope, unset }),
                    None,
                    ctx,
                );
            }
            Job::Email { scope, .. } => {
                let state = &mut self.workbench.git.identity;
                state.dirty = false;
                state.notice = format!("Git identity updated · {}", scope.label());
                self.git_identity_load(ctx);
            }
        }
    }
}

#[cfg(test)]
mod tests;
