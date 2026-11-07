//! Explicit HTTPS retries. Forms and credentials are transient and never enter
//! preferences, Git configuration, command arguments or the output window.
use super::*;
use project::git_auth::Credentials;

#[derive(Default)]
/// One operation's in-flight credentials or explicit retry form; never serialized.
pub(super) struct State {
    pending: Option<Attempt>,
    prompt: Option<Prompt>,
    /// Distinguish new challenges without exposing entry values in fixture metadata.
    generation: u64,
}
struct Attempt {
    args: Vec<String>,
    job: GitJob,
    credentials: Vec<Credentials>,
}
struct Prompt {
    generation: u64,
    attempt: Attempt,
    entry: Credentials,
    ctx: egui::Context,
    error: String,
}
impl Drop for Prompt {
    fn drop(&mut self) {
        for key in ["git-auth-username", "git-auth-password"] {
            self.ctx
                .data_mut(|data| data.remove::<egui::text_edit::TextEditState>(egui::Id::new(key)));
        }
    }
}
impl State {
    pub(super) fn open(&self) -> bool {
        self.prompt.is_some()
    }

    /// Keep earlier origins only while this exact network operation is pending.
    pub(super) fn begin(
        &mut self,
        args: &[String],
        job: &GitJob,
        credentials: &[Credentials],
        no_input: bool,
    ) {
        self.pending = (no_input && project::git_auth::network(args)).then(|| Attempt {
            args: args.to_vec(),
            job: job.clone(),
            credentials: credentials.to_vec(),
        });
    }

    /// Completion drops credentials; a challenge retains only other approved origins.
    pub(super) fn finish(&mut self, challenge: Option<String>, ctx: &egui::Context) {
        let Some(mut attempt) = self.pending.take() else {
            return;
        };
        if let Some(origin) = challenge {
            self.generation = self.generation.wrapping_add(1);
            attempt
                .credentials
                .retain(|credential| credential.origin != origin);
            self.prompt = Some(Prompt {
                generation: self.generation,
                attempt,
                entry: Credentials {
                    origin,
                    username: String::new(),
                    password: String::new(),
                },
                ctx: ctx.clone(),
                error: String::new(),
            });
        }
    }
}

impl EditorApp {
    /// Ask for one HTTPS origin and dispatch only after explicit submission.
    pub(super) fn git_auth_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut prompt) = self.workbench.git.auth.prompt.take() else {
            return;
        };
        let palette = self.themes.active().palette;
        let mut submit = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("git-auth"))
            .frame(layout::dialog_frame(&ctx.style(), palette))
            .show(ctx, |ui| {
                ui.set_width(420_f32.min((ctx.content_rect().width() - 64.).max(1.)));
                let heading = ui.heading("Git HTTPS authentication");
                ui_probe::button(ui, &format!("git-auth-form:{}", prompt.generation), &heading);
                ui.label("Credentials will be sent only to:");
                ui.monospace(&prompt.entry.origin);
                ui.label("Username");
                let mut username = egui::TextEdit::singleline(&mut prompt.entry.username)
                    .id(egui::Id::new("git-auth-username"))
                    .desired_width(f32::INFINITY)
                    .char_limit(4096)
                    .show(ui);
                username.state.clear_undoer();
                username.state.store(ctx, username.response.id);
                ui_probe::button(ui, "git-auth-username", &username.response);
                ui.label("Password or access token");
                let mut password = egui::TextEdit::singleline(&mut prompt.entry.password)
                    .id(egui::Id::new("git-auth-password"))
                    .password(true)
                    .desired_width(f32::INFINITY)
                    .char_limit(16384)
                    .show(ui);
                password.state.clear_undoer();
                password.state.store(ctx, password.response.id);
                ui_probe::button(ui, "git-auth-password", &password.response);
                ui.add(egui::Label::new("Used for this operation only. Rode does not save these values; credential stores are bypassed for the retry.").wrap());
                if !prompt.error.is_empty() { ui.colored_label(palette.error, &prompt.error); }
                ui.horizontal(|ui| {
                    let button = ui.add_enabled(!self.workbench.git.git_busy && self.project_ready()
                        && !prompt.entry.username.is_empty() && !prompt.entry.password.is_empty(), egui::Button::new("Retry operation"));
                    ui_probe::button(ui, "git-auth-submit", &button);
                    submit = button.clicked();
                    let button = ui.button("Cancel");
                    ui_probe::button(ui, "git-auth-cancel", &button);
                    cancel = button.clicked();
                });
            });
        if cancel || modal.should_close() {
            ctx.request_repaint();
            return;
        }
        if submit {
            if let Err(error) = prompt.entry.validate() {
                prompt.error = error.to_string();
            } else {
                let credentials = &mut prompt.attempt.credentials;
                credentials.retain(|credential| credential.origin != prompt.entry.origin);
                if credentials.len() >= 8 {
                    prompt.error =
                        "This operation exceeds the limit of eight authentication origins.".into();
                } else {
                    credentials.push(prompt.entry.clone());
                    let args = prompt.attempt.args.clone();
                    let job = prompt.attempt.job.clone();
                    let credentials = std::mem::take(credentials);
                    self.git_command_authenticated(args, job, None, credentials, ctx);
                    ctx.request_repaint();
                    return;
                }
            }
        }
        self.workbench.git.auth.prompt = Some(prompt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> GitJob {
        GitJob::Mutate {
            label: "Fetch".into(),
            clear_message: false,
        }
    }

    #[test]
    fn pointer_acknowledges_complete_credential_entry_even_when_input_is_batched() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("widgets.json");
        let ctx = egui::Context::default();
        ui_probe::install_at(&ctx, path.clone());
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.workspace = Some(Workspace {
            root: "/fixture".into(),
            remote: None,
            connected: true,
            ready: true,
            plan: Default::default(),
            sandbox_options: Default::default(),
        });
        app.workbench
            .git
            .auth
            .begin(&["fetch".into()], &job(), &[], true);
        app.workbench
            .git
            .auth
            .finish(Some("https://git.example.test".into()), &ctx);
        let frame = |app: &mut EditorApp, events| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800., 600.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| app.git_auth_dialog(ctx),
            );
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap()
        };
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        for (field, parts, next) in [
            (
                "git-auth-username",
                ["private-", "user"],
                "git-auth-password",
            ),
            (
                "git-auth-password",
                ["private-", "password"],
                "git-auth-submit",
            ),
        ] {
            ctx.memory_mut(|memory| memory.request_focus(egui::Id::new(field)));
            // The fixture first waits for an edit acknowledgement. A slow
            // native frame may then batch the remaining text with the motion.
            let state = frame(&mut app, vec![egui::Event::Text(parts[0].into())]);
            let target = state["buttons"]
                .as_array()
                .unwrap()
                .iter()
                .find(|button| button["name"] == next)
                .unwrap();
            let rect: [f32; 4] = serde_json::from_value(target["rect"].clone()).unwrap();
            let point = egui::pos2((rect[0] + rect[2]) / 2., (rect[1] + rect[3]) / 2.);
            let state = frame(
                &mut app,
                vec![
                    egui::Event::Text(parts[1].into()),
                    egui::Event::PointerMoved(point),
                ],
            );
            assert_eq!(
                ctx.memory(|memory| memory.focused()),
                Some(egui::Id::new(field))
            );
            assert!(
                state["buttons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|button| button["name"] == next
                        && button["enabled"] == true
                        && button["hovered"] == true)
            );
            let snapshot = serde_json::to_string(&state).unwrap();
            assert!(!snapshot.contains("private-"));
        }
        let prompt = app.workbench.git.auth.prompt.as_ref().unwrap();
        assert_eq!(prompt.entry.username, "private-user");
        assert_eq!(prompt.entry.password, "private-password");
    }

    #[test]
    fn repeated_challenges_have_distinct_form_identities_and_empty_entries() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        let mut previous = 0;
        for _ in 0..3 {
            state.begin(&["fetch".into()], &job(), &[], true);
            state.finish(Some("https://git.example.test".into()), &ctx);
            let prompt = state.prompt.as_mut().unwrap();
            assert_ne!(prompt.generation, previous);
            assert!(prompt.entry.username.is_empty() && prompt.entry.password.is_empty());
            previous = prompt.generation;
            prompt.entry.username = "user".into();
            prompt.entry.password = "password".into();
            drop(state.prompt.take());
        }
    }

    #[test]
    fn cancelled_forms_remove_widget_history_and_do_not_retain_entries() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        state.begin(&["fetch".into()], &job(), &[], true);
        state.finish(Some("https://git.example.test".into()), &ctx);
        let prompt = state.prompt.as_mut().unwrap();
        prompt.entry.username = "private-user".into();
        prompt.entry.password = "private-password".into();
        for key in ["git-auth-username", "git-auth-password"] {
            egui::text_edit::TextEditState::default().store(&ctx, egui::Id::new(key));
        }
        drop(state.prompt.take());
        assert!(!state.open());
        assert!(state.pending.is_none());
        for key in ["git-auth-username", "git-auth-password"] {
            assert!(egui::TextEdit::load_state(&ctx, egui::Id::new(key)).is_none());
        }
    }

    #[test]
    fn multi_origin_retries_keep_credentials_only_until_the_operation_finishes() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        let credentials = vec![Credentials {
            origin: "https://first.example.test".into(),
            username: "private-user".into(),
            password: "private-password".into(),
        }];
        state.begin(
            &["fetch".into(), "--all".into()],
            &job(),
            &credentials,
            true,
        );
        state.finish(Some("https://second.example.test".into()), &ctx);
        let prompt = state.prompt.as_ref().unwrap();
        assert_eq!(prompt.attempt.credentials.len(), 1);
        assert!(prompt.entry.username.is_empty());
        assert!(prompt.entry.password.is_empty());
        drop(state.prompt.take());
        state.begin(&["fetch".into()], &job(), &credentials, true);
        state.finish(None, &ctx);
        assert!(state.pending.is_none());
        assert!(!state.open());
        state.begin(&["fetch".into()], &job(), &credentials, true);
        state.finish(Some(credentials[0].origin.clone()), &ctx);
        assert!(
            state
                .prompt
                .as_ref()
                .unwrap()
                .attempt
                .credentials
                .is_empty()
        );
    }
}
