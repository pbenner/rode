//! Bundled code tools feature implementation.
//!
//! Formatting/checking uses unsaved source snapshots and tool generations. Save-after-format
//! applies only to the exact submitted version; unavailable tools leave ordinary source
//! editing and protected saves available.
use super::*;

#[derive(Default)]
/// Versioned diagnostics, debounce timestamps and in-flight/tool-availability caches
/// for this workspace; none of these transient values are preferences.
pub(super) struct State {
    pub(super) issues: HashMap<u64, Issues>,
    pub(super) checking: HashSet<u64>,
    pub(super) observed: Option<(u64, u64, Instant)>,
    pub(super) checked: HashMap<u64, (u64, u64)>,
    pub(super) tool_error: String,
    pub(super) unavailable_tools: HashSet<String>,
}

impl EditorApp {
    /// List only diagnostics for current document versions and route selected one-based locations
    /// back to the source editor; explicit check/format buttons queue host work.
    pub(in crate::app) fn problems_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.horizontal_wrapped(|ui| {
            if ui.button("Check file").clicked() {
                self.format_active(false, false, ctx);
            }
            if ui.button("Format").clicked() {
                self.format_active(true, false, ctx);
            }
            if ui.button("Ruff safe fixes").clicked() {
                self.format_active(true, true, ctx);
            }
        });
        if !self.workbench.code_tools.tool_error.is_empty() {
            ui.colored_label(palette.error, &self.workbench.code_tools.tool_error);
        }
        ui.label(
            RichText::new("Current buffer diagnostics · stale results are discarded")
                .small()
                .color(palette.muted),
        );
        let mut count = 0;
        let mut jump = None;
        for doc in &self.documents {
            let issues = self.document_issues(doc);
            if issues.is_empty() {
                continue;
            }
            count += issues.len();
            ui.add_space(8.);
            ui.label(RichText::new(format!("{}  ({})", doc.name(), issues.len())).strong());
            for issue in issues {
                let text = format!(
                    "{}:{}  {}  {}",
                    issue.row, issue.column, issue.code, issue.message
                );
                let color = if issue.severity == "error" {
                    palette.error
                } else {
                    palette.accent
                };
                if ui
                    .add(
                        egui::Button::new(RichText::new(text).color(color))
                            .wrap()
                            .frame(false),
                    )
                    .clicked()
                {
                    jump = Some((doc.id, issue.row, issue.column));
                }
            }
        }
        if let Some((id, row, col)) = jump {
            self.jump_to_issue(id, row, col);
        }
        if count == 0 {
            ui.label("No current issues reported in open files.");
        }
        if !self.workbench.code_tools.checking.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking buffers…");
            });
        }
        ui.add_space(12.);
        ui.label(RichText::new("Python: Ruff. Go: gofmt. JS/TS/web: Prettier. C/C++/Java/C#: clang-format. Shell: shfmt. Rust: rustfmt. Tools must be installed on the project host; configure automatic checks in Settings.").small().color(palette.muted));
    }
    /// Send a task to the prepared project terminal, creating it asynchronously if needed.
    /// Apply literal environment exports in a subshell so task overrides do not mutate the shell.
    pub(in crate::app) fn send_project_command(&mut self, command: String, ctx: &egui::Context) {
        if !self.project_ready() {
            return;
        }
        let exports = match self.workbench.prefs.environment.exports() {
            Ok(exports) => exports,
            Err(error) => {
                self.notify(error.to_string(), true);
                return;
            }
        };
        let command = if exports.is_empty() {
            command
        } else {
            format!("( {exports} {command}; )")
        };
        self.show_terminal = true;
        self.terminal_intent();
        if self.session_busy()
            || self
                .terminal
                .as_ref()
                .is_none_or(|terminal| !terminal.is_alive())
        {
            self.queue_terminal_command(command, ctx);
            return;
        }
        if let Some(terminal) = &self.terminal
            && let Err(error) = terminal.send((command + "\n").into_bytes())
        {
            self.notify(error.to_string(), true);
        }
        Terminal::focus(ctx);
    }
    /// Dispatch the active document to its notebook/LaTeX/language run or build path
    /// using the selected project host and installed tool configuration.
    pub(in crate::app) fn run_active(&mut self, build: bool, ctx: &egui::Context) {
        if self
            .documents
            .get(self.active)
            .is_some_and(|doc| crate::latex::is_source(doc.path.as_deref()))
        {
            self.build_latex(!build, ctx);
            return;
        }
        if self
            .documents
            .get(self.active)
            .is_some_and(|d| d.notebook.is_some())
        {
            if !build {
                self.run_notebook(false, ctx);
            }
            return;
        }
        if !self.feature(crate::extensions::Feature::Languages) {
            self.notify("Enable Language tools in View → Extensions.", false);
            return;
        }
        if !self.project_ready() {
            self.notify("Open a project folder to run code on its host.", false);
            return;
        }
        let Some(doc) = self.documents.get(self.active) else {
            return;
        };
        if doc.is_preview() {
            return;
        }
        if doc.dirty() || (build && self.documents.iter().any(|d| d.dirty())) {
            self.notify("Save modified files before running or building.", true);
            return;
        }
        let Some(path) = doc.path.as_deref() else {
            self.notify("Save the file before running it.", true);
            return;
        };
        let Some(language) = crate::languages::detect(Some(path)) else {
            self.notify(
                "No run/build command for this file type. Use the project terminal.",
                false,
            );
            return;
        };
        let Some(template) = (if build {
            &language.build
        } else {
            &language.run
        }) else {
            self.notify("No built-in command for this language. Use the project terminal for your workflow.", false);
            return;
        };
        let root = self.root();
        let mut args = template.clone();
        if language.id == "python" {
            let Some(environment) = &self.workbench.python.env else {
                self.notify(
                    "Select a valid project interpreter before running Python.",
                    true,
                );
                return;
            };
            args[0] = environment.executable.clone();
        }
        args[0] = self.workbench.prefs.environment.program(&args[0]);
        let mut command = args
            .iter()
            .map(|arg| ssh::quote(if arg == "{path}" { path } else { arg }))
            .collect::<Vec<_>>()
            .join(" ");
        if template[0] == "tsx"
            && !self
                .workbench
                .prefs
                .environment
                .runtimes
                .contains_key("tsx")
        {
            // Probe on the project host, inside the terminal's active sandbox.
            let executable = ssh::quote(&format!("{root}/node_modules/.bin/tsx"));
            let rest = args[1..]
                .iter()
                .map(|arg| ssh::quote(if arg == "{path}" { path } else { arg }))
                .collect::<Vec<_>>()
                .join(" ");
            command =
                format!("if test -x {executable}; then {executable} {rest}; else {command}; fi");
        }
        self.send_project_command(
            format!("cd -- {} && {{ {command}; }}", ssh::quote(&root)),
            ctx,
        );
    }
    /// Return whether a supported format/check path took responsibility for this save.
    /// `false` lets the caller use the ordinary protected save; `true` includes a save
    /// already in flight. Capture the expected revision before dispatching analysis.
    pub(in crate::app) fn save_with_tools(
        &mut self,
        id: u64,
        path: &str,
        save_as: bool,
        ctx: &egui::Context,
    ) -> bool {
        let language = crate::languages::detect(Some(path));
        let external = self
            .extensions
            .registry
            .provider(
                crate::app::language(Some(path)),
                self.workbench.prefs.format_on_save,
                false,
            )
            .is_some();
        let external = external
            || (self.workbench.prefs.check_on_save
                && self
                    .extensions
                    .registry
                    .provider(crate::app::language(Some(path)), false, true)
                    .is_some());
        let supported = external
            || (self.feature(crate::extensions::Feature::Languages)
                && language.is_some_and(|l| {
                    l.formatter.is_some()
                        && (l.id != "latex" || self.feature(crate::extensions::Feature::Latex))
                        && (self.workbench.prefs.format_on_save
                            || (self.workbench.prefs.check_on_save && l.check))
                }));
        if !supported
            || !self.project_ready()
            || (!self.workbench.prefs.format_on_save && !self.workbench.prefs.check_on_save)
        {
            return false;
        }
        let Some(doc) = self.documents.iter().find(|d| d.id == id) else {
            return false;
        };
        if doc.saving {
            return true;
        }
        if self.workbench.code_tools.checking.contains(&id) {
            // A live diagnostic check never delays a save. The next check will
            // observe the saved version; formatting gets its own job.
            self.workbench.code_tools.checking.remove(&id);
        }
        let expected = if save_as && doc.path.as_deref() != Some(path) {
            None
        } else {
            doc.revision.clone()
        };
        self.analyze_document(
            id,
            path.into(),
            Some(expected),
            self.workbench.prefs.format_on_save,
            false,
            false,
            ctx,
        );
        true
    }
    #[allow(clippy::too_many_arguments)]
    /// Submit an unsaved source snapshot with document version and tool epoch.
    /// `save = None` checks/formats only; `Some(None)` creates a new destination and
    /// `Some(Some(revision))` requests a conflict-checked save after accepted analysis.
    pub(in crate::app) fn analyze_document(
        &mut self,
        id: u64,
        path: String,
        save: Option<Option<String>>,
        format: bool,
        fix: bool,
        manual: bool,
        ctx: &egui::Context,
    ) {
        if !self.project_ready() {
            return;
        }
        let external = if fix {
            None
        } else {
            self.extensions
                .registry
                .provider(crate::app::language(Some(&path)), format, !format)
                .or_else(|| {
                    self.extensions.registry.provider(
                        crate::app::language(Some(&path)),
                        false,
                        true,
                    )
                })
                .cloned()
        };
        if external.is_none() && !self.feature(crate::extensions::Feature::Languages) {
            if manual {
                self.notify(
                    "Enable Language tools or a matching adapter in View → Extensions.",
                    false,
                );
            }
            return;
        }
        let language = crate::languages::detect(Some(&path));
        if external.is_none()
            && language.is_some_and(|l| l.id == "latex")
            && !self.feature(crate::extensions::Feature::Latex)
        {
            if manual {
                self.notify("Enable LaTeX tools in View → Extensions.", false);
            }
            return;
        }
        let tool = language
            .and_then(|l| {
                if format {
                    l.formatter.as_deref()
                } else {
                    l.check_tool()
                }
            })
            .unwrap_or("adapter")
            .to_owned();
        let Some(doc) = self.documents.iter_mut().find(|d| d.id == id) else {
            return;
        };
        if doc.is_preview() {
            self.notify(
                "Image and PDF previews are read-only; code tools apply to text files.",
                false,
            );
            return;
        }
        if save.is_some() {
            doc.saving = true;
        }
        let version = doc.edit_version;
        let original = doc.text.clone();
        let check = crate::languages::detect(Some(&path)).is_some_and(|l| l.check);
        self.workbench.code_tools.checking.insert(id);
        let request = if let Some(manifest) = external {
            project::Request::Extension {
                root: self.root(),
                invocation: self.extensions.registry.invocation(
                    &manifest,
                    if format && manifest.tool.format {
                        "format"
                    } else {
                        "check"
                    }
                    .into(),
                    Some(path.clone()),
                    original.clone(),
                ),
            }
        } else if language.is_some_and(|l| l.id == "latex") && !fix {
            project::Request::LatexAnalyze {
                root: self.root(),
                path: path.clone(),
                text: original.clone(),
                settings: self.workbench.prefs.latex.clone(),
                format,
                check: check && (manual || self.workbench.prefs.check_on_save || save.is_none()),
            }
        } else {
            project::Request::Analyze {
                root: self.root(),
                path: path.clone(),
                text: original.clone(),
                python: self.terminal_python_selection(),
                ruff: self.workbench.prefs.ruff.clone(),
                format,
                check: check && (manual || self.workbench.prefs.check_on_save || save.is_none()),
                fix,
            }
        };
        self.dispatch_tool(
            request,
            ToolJob::Analyze {
                tool,
                id,
                version,
                epoch: self.workbench.epoch,
                original,
                path,
                save,
                manual,
            },
            ctx,
        );
    }
    /// Queue manual checking, formatting or safe fixes for the active editable source document.
    pub(in crate::app) fn format_active(&mut self, format: bool, fix: bool, ctx: &egui::Context) {
        if let Some(doc) = self.documents.get(self.active) {
            if self.workbench.code_tools.checking.contains(&doc.id) || doc.saving {
                return;
            }
            let Some(path) = doc.path.clone() else {
                self.notify(
                    "Save the file once to select its language and project configuration.",
                    true,
                );
                return;
            };
            self.analyze_document(doc.id, path, None, format, fix, true, ctx);
        }
    }
    /// Select a stable document and translate a one-based diagnostic row/column into a source selection.
    pub(in crate::app) fn jump_to_issue(&mut self, id: u64, row: usize, column: usize) {
        if let Some(index) = self.documents.iter().position(|d| d.id == id) {
            self.active = index;
            let doc = &mut self.documents[index];
            let offset: usize = doc
                .text
                .split_inclusive('\n')
                .take(row.saturating_sub(1))
                .map(|s| s.chars().count())
                .sum::<usize>()
                + column.saturating_sub(1);
            doc.selection = Some((offset, offset));
        }
    }
    /// Return diagnostics only when their captured source version matches this document.
    pub(in crate::app) fn document_issues(&self, doc: &Document) -> Vec<Diagnostic> {
        self.workbench
            .code_tools
            .issues
            .get(&doc.id)
            .filter(|i| i.version == doc.edit_version)
            .map(|i| i.items.clone())
            .unwrap_or_default()
    }
}
