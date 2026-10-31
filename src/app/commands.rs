//! Typed palette registration shared by native commands and installed adapters.
//!
//! Commands resolve to typed actions and existing guards. Installed contributions become
//! explicit invocations; palette discovery never starts their executable.
use super::*;

enum PaletteAction {
    Style(navigation::Style),
    Navigation(navigation::NavAction),
    New,
    NewNotebook,
    NotebookRun,
    NotebookRunAll,
    NotebookSource,
    Open,
    Folder,
    Connect,
    Reconnect,
    Disconnect,
    Encrypted,
    LockEncrypted,
    Save,
    SaveAs,
    Terminal,
    CopyTerminal,
    Sidebar,
    Slurm,
    Explorer,
    Find,
    Shortcuts,
    UiScaleStep(f32),
    UiScaleReset,
    TerminalColors,
    Git,
    Monitor,
    Problems,
    Python,
    Environment,
    Tasks,
    Settings,
    Format,
    Check,
    Fix,
    Run,
    Agent,
    AgentSessions,
    Debug,
    DebugStart,
    DebugStop,
    Breakpoint,
    ReloadImage,
    Markdown,
    MarpNew,
    MarpSlides,
    LatexBuild,
    LatexPreview,
    LatexPdf,
    LatexOutput,
    LatexStop,
    MarpExport(crate::marp::Format),
    Build,
    ReloadPdf,
    PersistentTerminal,
    NewTerminal,
    Sessions,
    TemporaryTerminal,
    Extensions,
    Hex,
    Theme(String),
    Syntax(String),
    Extension(String, String),
}

impl EditorApp {
    pub(super) fn command_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.label(palette.tag("COMMAND PALETTE"));
        let list_keys = navigation::List::keys(ctx);
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.dialog_input)
                .desired_width(f32::INFINITY)
                .hint_text("Type a command…"),
        );
        if self.dialog_focus {
            response.request_focus();
            self.dialog_focus = false;
        }
        let commands = [
            (PaletteAction::New, "New file", "Ctrl+N"),
            (PaletteAction::Open, "Open file by path", "Ctrl+P"),
            (PaletteAction::Folder, "Open project folder", "Ctrl+Shift+O"),
            (PaletteAction::Connect, "Remote: Connect via SSH", ""),
            (PaletteAction::Reconnect, "Remote: Reconnect SSH", ""),
            (PaletteAction::Disconnect, "Remote: Disconnect SSH", ""),
            (
                PaletteAction::Encrypted,
                "Project: Open encrypted project",
                "",
            ),
            (
                PaletteAction::LockEncrypted,
                "Project: Lock encrypted project",
                "",
            ),
            (PaletteAction::Save, "Save file", "Ctrl+S"),
            (PaletteAction::SaveAs, "Save file as", "Ctrl+Shift+S"),
            (PaletteAction::Sidebar, "Toggle tools sidebar", "Ctrl+~"),
            (PaletteAction::Terminal, "Toggle terminal", "Ctrl+Shift+T"),
            (PaletteAction::CopyTerminal, "Terminal: Copy output", ""),
            (PaletteAction::Explorer, "Toggle explorer", "Ctrl+B"),
            (PaletteAction::Find, "Find / replace in file", "Ctrl+F"),
            (PaletteAction::Shortcuts, "Keyboard shortcuts", ""),
            (
                PaletteAction::TerminalColors,
                "Terminal: Colors and contrast",
                "",
            ),
            (PaletteAction::UiScaleStep(-0.05), "Interface: Zoom out", ""),
            (PaletteAction::UiScaleStep(0.05), "Interface: Zoom in", ""),
            (
                PaletteAction::UiScaleReset,
                "Interface: Reset scale",
                "Ctrl+0",
            ),
            (PaletteAction::Git, "Git: Source control", "Ctrl+Shift+G"),
            (PaletteAction::Monitor, "System: Monitor", ""),
            (PaletteAction::Problems, "Code: Problems", "Ctrl+Shift+M"),
            (PaletteAction::Python, "Python: Select environment", ""),
            (
                PaletteAction::Environment,
                "Environment: Runtimes and project variables",
                "",
            ),
            (PaletteAction::Tasks, "Tasks: Run, build and test", ""),
            (
                PaletteAction::Settings,
                "Project: Settings and sandbox access",
                "",
            ),
            (
                PaletteAction::Format,
                "Code: Format document",
                "Shift+Alt+F",
            ),
            (PaletteAction::Check, "Code: Check document", ""),
            (PaletteAction::Fix, "Ruff: Apply safe fixes", ""),
            (PaletteAction::Run, "Code: Run current file / project", ""),
            (PaletteAction::Agent, "Agent: Chat", "Ctrl+Shift+A"),
            (PaletteAction::AgentSessions, "Agent: Running sessions", ""),
            (PaletteAction::Debug, "Debug: Show debugger", ""),
            (PaletteAction::DebugStart, "Debug: Start / continue", "F5"),
            (PaletteAction::DebugStop, "Debug: Stop / detach", "Shift+F5"),
            (PaletteAction::Breakpoint, "Debug: Toggle breakpoint", "F9"),
            (PaletteAction::ReloadImage, "Image: Reload preview", ""),
            (
                PaletteAction::Markdown,
                "Markdown: Toggle preview",
                "Ctrl+Shift+R",
            ),
            (
                PaletteAction::Build,
                "Code: Build current file / project",
                "",
            ),
            (PaletteAction::ReloadPdf, "PDF: Reload preview", ""),
            (PaletteAction::NewTerminal, "Terminal: New terminal", ""),
            (
                PaletteAction::Slurm,
                "Terminal: Allocate Slurm resources",
                "",
            ),
            (
                PaletteAction::PersistentTerminal,
                "Terminal: New persistent terminal",
                "",
            ),
            (
                PaletteAction::Sessions,
                "Terminal: Sessions and Slurm allocations",
                "",
            ),
            (
                PaletteAction::TemporaryTerminal,
                "Terminal: New temporary terminal",
                "",
            ),
            (
                PaletteAction::Extensions,
                "Extensions: Manage installed extensions",
                "Ctrl+Shift+X",
            ),
            (PaletteAction::Hex, "Hex: Open file / inspect binary", ""),
            (
                PaletteAction::NewNotebook,
                "Notebook: New Jupyter notebook",
                "",
            ),
            (
                PaletteAction::NotebookRun,
                "Notebook: Run selected cell",
                "Shift+Enter",
            ),
            (PaletteAction::NotebookRunAll, "Notebook: Run all cells", ""),
            (PaletteAction::MarpNew, "Marp: New presentation", ""),
            (PaletteAction::LatexBuild, "LaTeX: Build PDF", ""),
            (
                PaletteAction::LatexPreview,
                "LaTeX: Build and preview PDF",
                "",
            ),
            (PaletteAction::LatexPdf, "LaTeX: Open compiled PDF", ""),
            (PaletteAction::LatexOutput, "LaTeX: Show build output", ""),
            (PaletteAction::LatexStop, "LaTeX: Stop build", ""),
            (PaletteAction::MarpSlides, "Marp: Toggle slide preview", ""),
            (
                PaletteAction::MarpExport(crate::marp::Format::Html),
                "Marp: Export HTML",
                "",
            ),
            (
                PaletteAction::MarpExport(crate::marp::Format::Pdf),
                "Marp: Export PDF",
                "",
            ),
            (
                PaletteAction::MarpExport(crate::marp::Format::Pptx),
                "Marp: Export PowerPoint",
                "",
            ),
            (
                PaletteAction::NotebookSource,
                "Notebook: Toggle JSON source view",
                "",
            ),
        ];

        let mut commands: Vec<_> = commands
            .into_iter()
            .map(|(action, name, shortcut)| {
                (
                    action,
                    name.to_string(),
                    self.editing_hint(shortcut).to_string(),
                )
            })
            .collect();
        commands.extend(navigation::Style::ALL.into_iter().map(|style| {
            (
                PaletteAction::Style(style),
                format!("Keyboard style: {}", style.name()),
                String::new(),
            )
        }));
        commands.extend(
            self.navigation
                .bindings
                .commands()
                .map(|(action, name, shortcut)| {
                    (
                        PaletteAction::Navigation(action),
                        format!("Navigation: {name}"),
                        shortcut.to_owned(),
                    )
                }),
        );
        commands.extend(self.extensions.registry.commands().into_iter().map(
            |(extension, command, title)| {
                (
                    PaletteAction::Extension(extension, command),
                    title,
                    String::new(),
                )
            },
        ));
        commands.extend(self.themes.entries.iter().map(|entry| {
            (
                PaletteAction::Theme(entry.id.clone()),
                format!("Theme: {}", entry.name),
                String::new(),
            )
        }));
        commands.extend(self.themes.syntax_entries.iter().map(|entry| {
            (
                PaletteAction::Syntax(entry.id.clone()),
                format!("Syntax theme: {}", entry.name),
                String::new(),
            )
        }));
        let query = self.dialog_input.to_lowercase();
        let filtered: Vec<_> = commands
            .iter()
            .enumerate()
            .filter(|(_, (_, name, _))| name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();
        let reveal = self
            .navigation
            .list
            .update(&query, filtered.len(), list_keys.0);
        let mut selected = if list_keys.1 {
            filtered.get(self.navigation.list.index).copied()
        } else {
            None
        };
        egui::ScrollArea::vertical()
            .id_salt("command-results")
            .max_height(360.)
            .show(ui, |ui| {
                for (row, index) in filtered.iter().copied().enumerate() {
                    let (_, name, shortcut) = &commands[index];
                    let response = ui.add_sized(
                        [ui.available_width(), 30.],
                        egui::Button::new(format!("{name}     {shortcut}"))
                            .selected(row == self.navigation.list.index),
                    );
                    if row == self.navigation.list.index && reveal {
                        response.scroll_to_me(Some(Align::Center));
                    }
                    if response.clicked() {
                        selected = Some(index);
                    }
                }
                if filtered.is_empty() {
                    ui.label("No matching commands");
                }
            });
        if let Some(index) = selected {
            self.dialog = Dialog::None;
            match commands.remove(index).0 {
                PaletteAction::Navigation(action) => self.navigate(action, ctx),
                PaletteAction::Style(style) => self.navigation.bindings.select_style(style),
                PaletteAction::Theme(id) => self.select_theme(&id, ctx),
                PaletteAction::Syntax(id) => self.select_syntax_theme(&id, ctx),
                PaletteAction::UiScaleStep(step) => self.set_ui_scale(self.ui_scale + step, ctx),
                PaletteAction::UiScaleReset => self.set_ui_scale(1., ctx),
                PaletteAction::TerminalColors => self.terminal_colors.open = true,
                PaletteAction::Extension(extension, command) => {
                    self.extension_command(&extension, &command, ctx)
                }
                PaletteAction::New => self.new_file(),
                PaletteAction::NewNotebook => self.new_notebook(),
                PaletteAction::NotebookRun => self.run_notebook(false, ctx),
                PaletteAction::NotebookRunAll => self.run_notebook(true, ctx),
                PaletteAction::NotebookSource => self.toggle_notebook_source(ctx),
                PaletteAction::Open => self.set_dialog(Dialog::Open),
                PaletteAction::Folder => self.set_dialog(Dialog::Folder),
                PaletteAction::Connect => self.set_dialog(Dialog::Connect),
                PaletteAction::Disconnect => self.disconnect_ssh(ctx),
                PaletteAction::Encrypted => self.encrypted_dialog(),
                PaletteAction::LockEncrypted => self.guarded(Action::LockEncrypted, ctx),
                PaletteAction::Reconnect => self.reconnect_ssh(ctx),
                PaletteAction::Save => self.save_active(ctx, false),
                PaletteAction::SaveAs => self.save_active(ctx, true),
                PaletteAction::Terminal => self.toggle_terminal(ctx),
                PaletteAction::CopyTerminal => self.copy_terminal_output(ctx),
                PaletteAction::Sidebar => self.toggle_sidebar(ctx),
                PaletteAction::Slurm => self.open_slurm(ctx),
                PaletteAction::Explorer => self.show_explorer = !self.show_explorer,
                PaletteAction::Find => {
                    self.markdown_source();
                    self.show_find = true;
                    self.find_focus = true;
                }
                PaletteAction::Shortcuts => self.set_dialog(Dialog::Shortcuts),
                PaletteAction::Git => self.toggle_panel(Panel::Git, ctx),
                PaletteAction::Monitor => self.toggle_panel(Panel::Monitor, ctx),
                PaletteAction::Problems => self.toggle_panel(Panel::Problems, ctx),
                PaletteAction::Python => self.open_python_environment(ctx),
                PaletteAction::Environment => self.toggle_panel(Panel::Environment, ctx),
                PaletteAction::Tasks => self.toggle_panel(Panel::Tasks, ctx),
                PaletteAction::Settings => self.toggle_panel(Panel::Settings, ctx),
                PaletteAction::Format => self.format_active(true, false, ctx),
                PaletteAction::Check => self.format_active(false, false, ctx),
                PaletteAction::Fix => self.format_active(true, true, ctx),
                PaletteAction::Run => self.run_active(false, ctx),
                PaletteAction::Agent => self.toggle_panel(Panel::Agent, ctx),
                PaletteAction::AgentSessions => self.open_background_agents(ctx),
                PaletteAction::Debug => self.toggle_panel(Panel::Debug, ctx),
                PaletteAction::DebugStart => self.start_debugger(ctx),
                PaletteAction::DebugStop => self.stop_debugger(),
                PaletteAction::Breakpoint => self.toggle_cursor_breakpoint(),
                PaletteAction::ReloadImage => {
                    if let Some(doc) = self.documents.get(self.active) {
                        self.reload_image(doc.id, ctx);
                    }
                }
                PaletteAction::Markdown => self.toggle_markdown(ctx),
                PaletteAction::MarpNew => self.new_marp(),
                PaletteAction::LatexBuild => self.build_latex(false, ctx),
                PaletteAction::LatexPreview => self.build_latex(true, ctx),
                PaletteAction::LatexPdf => self.open_latex_pdf(ctx),
                PaletteAction::LatexOutput => self.show_latex_output(),
                PaletteAction::LatexStop => self.stop_latex(),
                PaletteAction::MarpSlides => self.toggle_marp(ctx),
                PaletteAction::MarpExport(format) => {
                    if let Some(doc) = self.documents.get(self.active) {
                        self.marp_export(doc.id, format);
                    } else {
                        self.notify("Open a Markdown file to export Marp slides.", false);
                    }
                }
                PaletteAction::Build => self.run_active(true, ctx),
                PaletteAction::PersistentTerminal => self.new_persistent_terminal(ctx),
                PaletteAction::NewTerminal => self.new_terminal(ctx),
                PaletteAction::Sessions => self.open_sessions(ctx),
                PaletteAction::TemporaryTerminal => self.new_temporary_terminal(ctx),
                PaletteAction::Extensions => self.set_dialog(Dialog::Extensions),
                PaletteAction::Hex => self.set_dialog(Dialog::HexOpen),
                PaletteAction::ReloadPdf => {
                    if let Some(doc) = self.documents.get(self.active) {
                        self.reload_pdf(doc.id, ctx);
                    }
                }
            }
        }
    }
}
