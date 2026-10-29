//! Explicit LaTeX builds; the configured compiler runs off the UI thread and
//! inside the same host policy as project files and terminals.
//!
//! Build settings and workspace identity are captured for each cancellable request.
//! Saved-source guards run before compilation; output and source diagnostics are versioned
//! independently of the retained PDF viewer.
use super::*;

pub(super) struct State {
    busy: bool,
    token: u64,
    cancel: Arc<AtomicBool>,
    output_open: bool,
    result: Option<crate::latex::Reply>,
    last_pdf: Option<String>,
    view_output: String,
    error: String,
    pending_jump: Option<(String, usize, usize)>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            busy: false,
            token: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            output_open: false,
            result: None,
            last_pdf: None,
            view_output: String::new(),
            error: String::new(),
            pending_jump: None,
        }
    }
}
impl EditorApp {
    pub(super) fn show_latex_output(&mut self) {
        self.latex.output_open = true;
    }
    pub(super) fn stop_latex(&mut self) {
        if self.latex.busy {
            self.status = "LaTeX build stopped".into();
        }
        self.latex.cancel.store(true, Ordering::Release);
        self.latex.token = self.latex.token.wrapping_add(1);
        self.latex.busy = false;
        self.latex.pending_jump = None;
    }
    pub(super) fn build_latex(&mut self, preview: bool, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Latex) {
            self.notify("Enable LaTeX tools in View → Extensions.", false);
            return;
        }
        if !self.project_ready() {
            self.notify("Open a connected project before building LaTeX.", false);
            return;
        }
        if self.latex.busy {
            self.latex.output_open = true;
            return;
        }
        if self.documents.iter().any(Document::dirty) {
            self.notify(
                "Save modified files before building LaTeX. Builds use the saved project files.",
                true,
            );
            return;
        }
        let Some(doc) = self.documents.get(self.active) else {
            return;
        };
        let path = if doc.is_preview() {
            self.latex.result.as_ref().map(|r| r.source.clone())
        } else {
            doc.path.clone()
        };
        let Some(path) = path else {
            self.notify("Save the LaTeX file before building it.", true);
            return;
        };
        if !crate::latex::is_source(Some(&path)) {
            self.notify("Open a LaTeX source file to build it.", false);
            return;
        }
        let settings = self.workbench.latex_settings().clone();
        if let Err(error) = settings.validate() {
            self.notify(error.to_string(), true);
            return;
        }
        let workspace = self.workspace.as_ref().unwrap();
        let remote = workspace.remote.clone();
        let plan = workspace.plan.clone();
        let invocation = crate::latex::Invocation {
            root: workspace.root.clone(),
            path,
            settings,
        };
        let versions = self
            .documents
            .iter()
            .map(|doc| (doc.id, doc.edit_version))
            .collect();
        self.latex.token = self.latex.token.wrapping_add(1);
        let token = self.latex.token;
        let generation = self.generation;
        self.latex.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.latex.cancel.clone();
        self.latex.busy = true;
        self.status = "Building LaTeX…".into();
        self.latex.error.clear();
        self.latex.output_open = true;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let result = (|| -> anyhow::Result<_> {
                if let Some(remote) = remote {
                    remote.latex_request(&invocation, &plan, &cancel)
                } else {
                    let (command, _helper) = plan.coordinator_command("--latex-helper")?;
                    crate::latex::execute(command, &invocation, &cancel)
                }
            })()
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(Event::Latex {
                generation,
                token,
                preview,
                versions,
                result: Box::new(result),
            });
            ctx.request_repaint();
        });
    }
    pub(super) fn complete_latex(
        &mut self,
        token: u64,
        preview: bool,
        versions: Vec<(u64, u64)>,
        result: Result<crate::latex::Reply, String>,
        ctx: &egui::Context,
    ) {
        if token != self.latex.token || !self.feature(crate::extensions::Feature::Latex) {
            return;
        }
        self.latex.busy = false;
        match result {
            Err(error) => {
                self.latex.error = error.clone();
                self.notify(error, true);
            }
            Ok(reply) => {
                // Compilation uses disk sources. Never overwrite buffers or
                // apply build diagnostics to edits made after the build began.
                for doc in &self.documents {
                    if versions.contains(&(doc.id, doc.edit_version))
                        && crate::latex::is_source(doc.path.as_deref())
                    {
                        let items = reply
                            .diagnostics
                            .iter()
                            .filter(|issue| Some(issue.path.as_str()) == doc.path.as_deref())
                            .map(|issue| issue.diagnostic.clone())
                            .collect();
                        self.workbench
                            .set_build_issues(doc.id, doc.edit_version, items);
                    }
                }
                let pdf = reply.pdf.clone();
                if let Some(path) = &pdf {
                    self.latex.last_pdf = Some(path.clone());
                }
                let success = reply.output.success() && pdf.is_some();
                self.status = if success {
                    "LaTeX PDF built"
                } else {
                    "LaTeX build failed; see build output"
                }
                .into();
                if !reply.note.is_empty() {
                    self.latex.error = reply.note.clone();
                }
                fn tail(text: &str) -> &str {
                    let mut start = text.len().saturating_sub(32 * 1024);
                    while !text.is_char_boundary(start) {
                        start += 1;
                    }
                    &text[start..]
                }
                self.latex.view_output = format!(
                    "{}{}\n{}",
                    if reply.output.stdout.len() > 32 * 1024
                        || reply.output.stderr.len() > 32 * 1024
                    {
                        "[Showing the end of the log. Copy full output for the complete log.]\n"
                    } else {
                        ""
                    },
                    tail(&reply.output.stdout),
                    tail(&reply.output.stderr)
                );
                self.latex.result = Some(reply);
                if preview && success {
                    self.latex.output_open = false;
                    self.open_latex_pdf_path(pdf.unwrap(), ctx);
                }
                self.list(self.root(), ctx);
            }
        }
    }
    fn open_latex_pdf_path(&mut self, path: String, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Pdf) {
            self.notify(
                "Enable PDF previews in View → Extensions to display the compiled PDF.",
                false,
            );
            return;
        }
        if let Some(index) = self
            .documents
            .iter()
            .position(|doc| doc.path.as_deref() == Some(path.as_str()))
        {
            self.active = index;
            self.reload_pdf(self.documents[index].id, ctx);
            self.reveal_tab(self.documents[index].id);
        } else {
            self.open_file(path, ctx);
        }
    }
    pub(super) fn open_latex_pdf(&mut self, ctx: &egui::Context) {
        if let Some(path) = self.latex.last_pdf.clone() {
            self.latex.output_open = false;
            self.open_latex_pdf_path(path, ctx);
        } else {
            self.notify(
                "Build a LaTeX document successfully before opening its PDF.",
                false,
            );
        }
    }
    pub(super) fn latex_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Latex)
            || !self
                .documents
                .get(self.active)
                .is_some_and(|doc| crate::latex::is_source(doc.path.as_deref()))
        {
            return;
        }
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(12, 4))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(self.themes.active().palette.tag("LATEX"));
                    if ui
                        .add_enabled(!self.latex.busy, egui::Button::new("Build PDF"))
                        .clicked()
                    {
                        self.build_latex(false, ctx);
                    }
                    if ui
                        .add_enabled(!self.latex.busy, egui::Button::new("Build & preview"))
                        .clicked()
                    {
                        self.build_latex(true, ctx);
                    }
                    if ui.button("Open PDF").clicked() {
                        self.open_latex_pdf(ctx);
                    }
                    if ui.button("Output").clicked() {
                        self.latex.output_open = true;
                    }
                    if ui.button("Settings").clicked() {
                        self.toggle_panel(Panel::Settings, ctx);
                    }
                    if self.latex.busy {
                        ui.spinner();
                        if ui.button("Stop").clicked() {
                            self.stop_latex();
                        }
                    }
                });
            });
    }
    pub(super) fn latex_window(&mut self, ctx: &egui::Context) {
        if let Some((path, row, column)) = self.latex.pending_jump.clone() {
            if let Some(doc) = self
                .documents
                .iter()
                .find(|doc| doc.path.as_deref() == Some(path.as_str()))
            {
                let id = doc.id;
                self.latex.pending_jump = None;
                self.jump_to_issue(id, row, column);
                self.focus_editor(ctx);
            } else if !self.opening.contains(&path) {
                self.latex.pending_jump = None;
            }
        }
        if !self.latex.output_open
            || self.dialog != Dialog::None
            || self.pending.is_some()
            || self.workbench_modal()
        {
            return;
        }
        let mut open = true;
        let mut jump = None;
        egui::Window::new("LaTeX build output")
            .id(egui::Id::new("latex-output"))
            .open(&mut open)
            .default_size([720., 480.])
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if self.latex.busy {
                        ui.spinner();
                        ui.label("Building on the project host…");
                        if ui.button("Stop build").clicked() {
                            self.stop_latex();
                        }
                    }
                    if !self.latex.busy && ui.button("Build & preview").clicked() {
                        self.build_latex(true, ctx);
                    }
                    if ui.button("Open PDF").clicked() {
                        self.open_latex_pdf(ctx);
                    }
                    if ui.button("Close output").clicked() {
                        self.latex.output_open = false;
                    }
                });
                if !self.latex.error.is_empty() {
                    ui.colored_label(self.themes.active().palette.error, &self.latex.error);
                }
                if let Some(reply) = &self.latex.result {
                    ui.add(
                        egui::Label::new(format!("{} · exit {}", reply.source, reply.output.code))
                            .truncate(),
                    )
                    .on_hover_text(&reply.source);
                    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 6.;
                    egui::ScrollArea::vertical().max_height(180.).show_rows(
                        ui,
                        row_height,
                        reply.diagnostics.len(),
                        |ui, rows| {
                            for issue in &reply.diagnostics[rows] {
                                if ui
                                    .add_sized(
                                        [ui.available_width(), row_height],
                                        egui::Button::new(format!(
                                            "{}:{}  {}",
                                            issue.path,
                                            issue.diagnostic.row,
                                            issue.diagnostic.message
                                        ))
                                        .frame(false)
                                        .truncate(),
                                    )
                                    .on_hover_text(&issue.diagnostic.message)
                                    .clicked()
                                {
                                    jump = Some((
                                        issue.path.clone(),
                                        issue.diagnostic.row,
                                        issue.diagnostic.column,
                                    ));
                                }
                            }
                        },
                    );
                    if ui.button("Copy full output").clicked() {
                        ctx.copy_text(format!("{}\n{}", reply.output.stdout, reply.output.stderr));
                    }
                    egui::ScrollArea::both().max_height(340.).show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.latex.view_output)
                                .font(egui::TextStyle::Monospace)
                                .interactive(false)
                                .desired_width(f32::INFINITY),
                        );
                    });
                }
            });
        self.latex.output_open &= open;
        if let Some((path, row, column)) = jump {
            self.open_file(path.clone(), ctx);
            self.latex.pending_jump = Some((path, row, column));
            self.latex.output_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app(root: &std::path::Path) -> (EditorApp, egui::Context) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let root = root.to_string_lossy().into_owned();
        app.workspace = Some(Workspace {
            root: root.clone(),
            remote: None,
            connected: true,
            ready: true,
            plan: sandbox::Plan {
                root,
                ..Default::default()
            },
            sandbox_options: Default::default(),
        });
        app.new_file();
        app.documents[0].path = Some(root_path(&app).join("main.tex").to_string_lossy().into());
        app.documents[0].text = "saved".into();
        app.documents[0].saved_text = "saved".into();
        app.documents[0].modified = false;
        (app, ctx)
    }
    fn root_path(app: &EditorApp) -> PathBuf {
        PathBuf::from(&app.workspace.as_ref().unwrap().root)
    }
    #[test]
    fn latex_build_rejects_dirty_sources_without_starting_a_process() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, ctx) = app(root.path());
        app.documents[0].text = "keep unsaved LaTeX".into();
        app.documents[0].modified = true;
        app.build_latex(true, &ctx);
        assert!(!app.latex.busy);
        assert_eq!(app.documents[0].text, "keep unsaved LaTeX");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(text, error, _)| *error && text.contains("Save modified"))
        );
    }
    #[test]
    fn latex_results_preserve_new_edits_and_context_changes_cancel_work() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, ctx) = app(root.path());
        let id = app.documents[0].id;
        app.documents[0].edit_version = 2;
        app.documents[0].text = "new edits".into();
        app.documents[0].modified = true;
        app.latex.token = 1;
        app.latex.busy = true;
        app.latex.last_pdf = Some("previous.pdf".into());
        let reply = crate::latex::Reply {
            output: crate::project::Output {
                code: 1,
                ..Default::default()
            },
            diagnostics: vec![crate::latex::Issue {
                path: app.documents[0].path.clone().unwrap(),
                diagnostic: crate::project::Diagnostic {
                    row: 1,
                    column: 1,
                    message: "old issue".into(),
                    ..Default::default()
                },
            }],
            ..Default::default()
        };
        app.complete_latex(1, false, vec![(id, 1)], Ok(reply), &ctx);
        assert_eq!(app.latex.last_pdf.as_deref(), Some("previous.pdf"));
        assert_eq!(app.documents[0].text, "new edits");
        assert!(app.document_issues(&app.documents[0]).is_empty());
        app.latex.busy = true;
        let cancel = app.latex.cancel.clone();
        app.clear_project(true);
        assert!(cancel.load(Ordering::Acquire));
        app.complete_latex(1, true, vec![(id, 2)], Ok(Default::default()), &ctx);
        assert!(app.latex.result.is_none());
        assert!(app.latex.last_pdf.is_none());
        assert!(app.documents[0].dirty());
    }
}
