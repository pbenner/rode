//! Static UI commands shared by menus, the palette, activity buttons and shortcuts.
//! Dispatch uses the existing guarded app/domain operations; input guards stay with callers.
use super::super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) enum Command {
    New,
    NewEditorWindow,
    BookmarkProject,
    NewNotebook,
    NotebookRun,
    NotebookRunAll,
    NotebookSource,
    Open,
    Folder,
    Connect,
    Tunnels,
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
    Python,
    Format,
    Check,
    Fix,
    Run,
    AgentSessions,
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
    Panel(Panel),
    LocalFolder,
    SaveAll,
    Quit,
    DebugPause,
    DebugOver,
    DebugIn,
    DebugOut,
}

pub(in crate::app) struct Definition {
    pub label: &'static str,
    pub palette_label: &'static str,
    pub icon: Icon,
}

impl Definition {
    fn new(label: &'static str, icon: Icon) -> Self {
        Self {
            label,
            palette_label: label,
            icon,
        }
    }

    fn palette(mut self, label: &'static str) -> Self {
        self.palette_label = label;
        self
    }
}

impl Command {
    pub(in crate::app) const PALETTE: &'static [Self] = &[
        Self::New,
        Self::NewEditorWindow,
        Self::BookmarkProject,
        Self::Open,
        Self::Folder,
        Self::Connect,
        Self::Tunnels,
        Self::Reconnect,
        Self::Disconnect,
        Self::Encrypted,
        Self::LockEncrypted,
        Self::Save,
        Self::SaveAs,
        Self::Sidebar,
        Self::Terminal,
        Self::CopyTerminal,
        Self::Explorer,
        Self::Find,
        Self::Shortcuts,
        Self::TerminalColors,
        Self::UiScaleStep(-0.05),
        Self::UiScaleStep(0.05),
        Self::UiScaleReset,
        Self::Panel(Panel::Git),
        Self::Panel(Panel::Monitor),
        Self::Panel(Panel::Problems),
        Self::Python,
        Self::Panel(Panel::Environment),
        Self::Panel(Panel::Tasks),
        Self::Panel(Panel::Settings),
        Self::Format,
        Self::Check,
        Self::Fix,
        Self::Run,
        Self::Panel(Panel::Agent),
        Self::AgentSessions,
        Self::Panel(Panel::Debug),
        Self::DebugStart,
        Self::DebugStop,
        Self::Breakpoint,
        Self::ReloadImage,
        Self::Markdown,
        Self::Build,
        Self::ReloadPdf,
        Self::NewTerminal,
        Self::Slurm,
        Self::PersistentTerminal,
        Self::Sessions,
        Self::TemporaryTerminal,
        Self::Extensions,
        Self::Hex,
        Self::NewNotebook,
        Self::NotebookRun,
        Self::NotebookRunAll,
        Self::MarpNew,
        Self::LatexBuild,
        Self::LatexPreview,
        Self::LatexPdf,
        Self::LatexOutput,
        Self::LatexStop,
        Self::MarpSlides,
        Self::MarpExport(crate::marp::Format::Html),
        Self::MarpExport(crate::marp::Format::Pdf),
        Self::MarpExport(crate::marp::Format::Pptx),
        Self::NotebookSource,
    ];
    pub(in crate::app) const PANELS: &'static [Self] = &[
        Self::Panel(Panel::Git),
        Self::Panel(Panel::Monitor),
        Self::Panel(Panel::Problems),
        Self::Panel(Panel::Environment),
        Self::Panel(Panel::Tasks),
        Self::Panel(Panel::Settings),
        Self::Panel(Panel::Agent),
        Self::Panel(Panel::Debug),
    ];
    pub(in crate::app) fn definition(self) -> Definition {
        match self {
            Self::NewEditorWindow => Definition::new("New editor window", Icon::Files),
            Self::BookmarkProject => Definition::new("Bookmark current project", Icon::Bookmark)
                .palette("Projects: Toggle current project bookmark"),
            Self::New => Definition::new("New file", Icon::NewFile),
            Self::Open => Definition::new("Open file…", Icon::Search).palette("Open file by path"),
            Self::Folder => {
                Definition::new("Open project folder…", Icon::Folder).palette("Open project folder")
            }
            Self::Connect => {
                Definition::new("Connect via SSH…", Icon::Remote).palette("Remote: Connect via SSH")
            }
            Self::Tunnels => Definition::new("SSH tunnels…", Icon::Remote)
                .palette("SSH: Manage tunnels on this host"),
            Self::Reconnect => Definition::new("Remote: Reconnect SSH", Icon::Remote),
            Self::Disconnect => Definition::new("Remote: Disconnect SSH", Icon::Remote),
            Self::Encrypted => Definition::new("Open encrypted project…", Icon::Folder)
                .palette("Project: Open encrypted project"),
            Self::LockEncrypted => Definition::new("Project: Lock encrypted project", Icon::Folder),
            Self::Save => Definition::new("Save", Icon::Save).palette("Save file"),
            Self::SaveAs => Definition::new("Save as…", Icon::SaveAs).palette("Save file as"),
            Self::Sidebar => Definition::new("Toggle tools sidebar", Icon::ChevronRight),
            Self::Terminal => Definition::new("Toggle terminal", Icon::Terminal),
            Self::CopyTerminal => {
                Definition::new("Copy terminal output", Icon::Copy).palette("Terminal: Copy output")
            }
            Self::Explorer => Definition::new("Explorer", Icon::Files).palette("Toggle explorer"),
            Self::Find => {
                Definition::new("Find / replace", Icon::Search).palette("Find / replace in file")
            }
            Self::Shortcuts => {
                Definition::new("Keyboard shortcuts…", Icon::Settings).palette("Keyboard shortcuts")
            }
            Self::TerminalColors => Definition::new("Terminal colors…", Icon::Palette)
                .palette("Terminal: Colors and contrast"),
            Self::UiScaleStep(step) if step < 0. => {
                Definition::new("Smaller (5%)", Icon::ChevronLeft).palette("Interface: Zoom out")
            }
            Self::UiScaleStep(_) => {
                Definition::new("Larger (5%)", Icon::ChevronRight).palette("Interface: Zoom in")
            }
            Self::UiScaleReset => Definition::new("Reset to system default", Icon::Refresh)
                .palette("Interface: Reset scale"),
            Self::Panel(Panel::Git) => {
                Definition::new("Source control", Icon::Git).palette("Git: Source control")
            }
            Self::Panel(Panel::Monitor) => {
                Definition::new("System monitor", Icon::Monitor).palette("System: Monitor")
            }
            Self::Panel(Panel::Problems) => {
                Definition::new("Problems", Icon::Problems).palette("Code: Problems")
            }
            Self::Python => Definition::new("Python: Select environment", Icon::Environment),
            Self::Panel(Panel::Environment) => Definition::new("Environment", Icon::Environment)
                .palette("Environment: Runtimes and project variables"),
            Self::Panel(Panel::Tasks) => {
                Definition::new("Tasks", Icon::Play).palette("Tasks: Run, build and test")
            }
            Self::Panel(Panel::Settings) => Definition::new("Project settings", Icon::Settings)
                .palette("Project: Settings and sandbox access"),
            Self::Format => {
                Definition::new("Format document", Icon::Format).palette("Code: Format document")
            }
            Self::Check => {
                Definition::new("Check document", Icon::Check).palette("Code: Check document")
            }
            Self::Fix => Definition::new("Ruff: apply safe fixes", Icon::Fix)
                .palette("Ruff: Apply safe fixes"),
            Self::Run => Definition::new("Run current file / project", Icon::Play)
                .palette("Code: Run current file / project"),
            Self::Panel(Panel::Agent) => {
                Definition::new("Agent chat", Icon::Chat).palette("Agent: Chat")
            }
            Self::AgentSessions => Definition::new("Agent: Running sessions", Icon::Chat),
            Self::Panel(Panel::Debug) => {
                Definition::new("Debugger", Icon::Debug).palette("Debug: Show debugger")
            }
            Self::DebugStart => {
                Definition::new("Start / continue", Icon::Debug).palette("Debug: Start / continue")
            }
            Self::DebugStop => {
                Definition::new("Stop / detach", Icon::Stop).palette("Debug: Stop / detach")
            }
            Self::Breakpoint => Definition::new("Toggle breakpoint", Icon::Check)
                .palette("Debug: Toggle breakpoint"),
            Self::ReloadImage => Definition::new("Image: Reload preview", Icon::Refresh),
            Self::Markdown => {
                Definition::new("Markdown preview", Icon::Files).palette("Markdown: Toggle preview")
            }
            Self::Build => Definition::new("Build current file / project", Icon::Play)
                .palette("Code: Build current file / project"),
            Self::ReloadPdf => Definition::new("PDF: Reload preview", Icon::Refresh),
            Self::NewTerminal => Definition::new("Terminal: New terminal", Icon::Terminal),
            Self::Slurm => Definition::new("Terminal: Allocate Slurm resources", Icon::Terminal),
            Self::PersistentTerminal => {
                Definition::new("Terminal: New persistent terminal", Icon::Terminal)
            }
            Self::Sessions => Definition::new("Terminals & Slurm", Icon::Terminal)
                .palette("Terminal: Sessions and Slurm allocations"),
            Self::TemporaryTerminal => {
                Definition::new("Terminal: New temporary terminal", Icon::Terminal)
            }
            Self::Extensions => Definition::new("Extensions…", Icon::Extensions)
                .palette("Extensions: Manage installed extensions"),
            Self::Hex => Definition::new("Open binary / hex…", Icon::Files)
                .palette("Hex: Open file / inspect binary"),
            Self::NewNotebook => Definition::new("New Jupyter notebook", Icon::NewFile)
                .palette("Notebook: New Jupyter notebook"),
            Self::NotebookRun => Definition::new("Notebook: Run selected cell", Icon::Play),
            Self::NotebookRunAll => Definition::new("Notebook: Run all cells", Icon::Play),
            Self::MarpNew => Definition::new("New Marp presentation", Icon::NewFile)
                .palette("Marp: New presentation"),
            Self::LatexBuild => Definition::new("LaTeX: Build PDF", Icon::Play),
            Self::LatexPreview => Definition::new("LaTeX: Build and preview PDF", Icon::Play),
            Self::LatexPdf => Definition::new("LaTeX: Open compiled PDF", Icon::Files),
            Self::LatexOutput => Definition::new("LaTeX: Show build output", Icon::Files),
            Self::LatexStop => Definition::new("LaTeX: Stop build", Icon::Stop),
            Self::MarpSlides => Definition::new("Marp slide preview", Icon::Play)
                .palette("Marp: Toggle slide preview"),
            Self::MarpExport(crate::marp::Format::Html) => {
                Definition::new("Marp: Export HTML", Icon::Files)
            }
            Self::MarpExport(crate::marp::Format::Pdf) => {
                Definition::new("Marp: Export PDF", Icon::Files)
            }
            Self::MarpExport(crate::marp::Format::Pptx) => {
                Definition::new("Marp: Export PowerPoint", Icon::Files)
            }
            Self::NotebookSource => {
                Definition::new("Notebook: Toggle JSON source view", Icon::Files)
            }
            Self::LocalFolder => Definition::new("Open local folder…", Icon::Folder),
            Self::SaveAll => Definition::new("Save all", Icon::SaveAll),
            Self::Quit => Definition::new("Quit", Icon::Exit),
            Self::DebugPause => Definition::new("Pause", Icon::Pause),
            Self::DebugOver => Definition::new("Step over", Icon::StepOver),
            Self::DebugIn => Definition::new("Step into", Icon::StepIn),
            Self::DebugOut => Definition::new("Step out", Icon::StepOut),
        }
    }

    pub(in crate::app) fn shortcut(self) -> Option<egui::KeyboardShortcut> {
        let (modifiers, key) = match self {
            Self::New => (egui::Modifiers::COMMAND, Key::N),
            Self::Open => (egui::Modifiers::COMMAND, Key::P),
            Self::Folder => (egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::O),
            Self::Save => (egui::Modifiers::COMMAND, Key::S),
            Self::SaveAs => (egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::S),
            Self::Sidebar => (egui::Modifiers::CTRL, Key::Backtick),
            Self::Terminal => (egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::T),
            Self::Explorer => (egui::Modifiers::COMMAND, Key::B),
            Self::Find => (egui::Modifiers::COMMAND, Key::F),
            Self::UiScaleReset => (egui::Modifiers::COMMAND, Key::Num0),
            Self::Panel(Panel::Git) => (egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::G),
            Self::Panel(Panel::Problems) => {
                (egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::M)
            }
            Self::Format => (egui::Modifiers::SHIFT | egui::Modifiers::ALT, Key::F),
            Self::Panel(Panel::Agent) => {
                (egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::A)
            }
            Self::DebugStart => (egui::Modifiers::NONE, Key::F5),
            Self::DebugStop => (egui::Modifiers::SHIFT, Key::F5),
            Self::Breakpoint => (egui::Modifiers::NONE, Key::F9),
            Self::Markdown => (egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::R),
            Self::Extensions => (egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::X),
            Self::NotebookRun => (egui::Modifiers::SHIFT, Key::Enter),
            Self::DebugPause => (egui::Modifiers::NONE, Key::F6),
            Self::DebugOver => (egui::Modifiers::NONE, Key::F10),
            Self::DebugIn => (egui::Modifiers::NONE, Key::F11),
            Self::DebugOut => (egui::Modifiers::SHIFT, Key::F11),
            _ => return None,
        };
        Some(egui::KeyboardShortcut::new(modifiers, key))
    }

    pub(in crate::app) fn consume(self, ctx: &egui::Context) -> bool {
        if self == Self::Extensions {
            return consume_extensions_shortcut(ctx);
        }
        self.shortcut()
            .is_some_and(|shortcut| ctx.input_mut(|input| input.consume_shortcut(&shortcut)))
    }
}

impl EditorApp {
    pub(in crate::app) fn command_hint(&self, command: Command) -> String {
        let hint = command
            .shortcut()
            .map(|binding| binding.format(&egui::ModifierNames::NAMES, false))
            .unwrap_or_default();
        self.editing_hint(&hint).to_owned()
    }

    pub(in crate::app) fn command_selected(&self, command: Command, ctx: &egui::Context) -> bool {
        match command {
            Command::Panel(panel) => self.panel_visible(panel),
            Command::Explorer => {
                self.explorer_visible(ctx.viewport_id())
                    && self.pane_visible(navigation::Pane::Explorer, ctx.viewport_id())
            }
            Command::Connect => self.dialog == Dialog::Connect,
            Command::Terminal => self.terminal_any_visible(),
            Command::Extensions => self.dialog == Dialog::Extensions,
            _ => false,
        }
    }

    pub(in crate::app) fn consume_command(
        &mut self,
        command: Command,
        ctx: &egui::Context,
    ) -> bool {
        if command.consume(ctx) {
            self.execute_command(command, ctx);
            true
        } else {
            false
        }
    }

    pub(in crate::app) fn execute_command(&mut self, command: Command, ctx: &egui::Context) {
        match command {
            Command::New => self.submit_editor_operation(
                editor_operations::Operation::Create {
                    viewport: self.command_editor(ctx),
                    template: editor_operations::Template::Source,
                },
                ctx,
            ),
            Command::NewEditorWindow => self.new_editor_window(ctx),
            Command::BookmarkProject => self.toggle_project_bookmark(),
            Command::NewNotebook => self.submit_editor_operation(
                editor_operations::Operation::Create {
                    viewport: self.command_editor(ctx),
                    template: editor_operations::Template::Notebook,
                },
                ctx,
            ),
            Command::NotebookRun => self.run_notebook(false, ctx),
            Command::NotebookRunAll => self.run_notebook(true, ctx),
            Command::NotebookSource => self.toggle_notebook_source(ctx),
            Command::Open => self.set_dialog(Dialog::Open),
            Command::Folder => self.set_dialog(Dialog::Folder),
            Command::Connect => self.set_dialog(Dialog::Connect),
            Command::Tunnels => self.set_dialog(Dialog::Tunnels),
            Command::Disconnect => self.disconnect_ssh(ctx),
            Command::Encrypted => self.encrypted_dialog(),
            Command::LockEncrypted => self.guarded(Action::LockEncrypted, ctx),
            Command::Reconnect => self.reconnect_ssh(ctx),
            Command::Save => self.save_editor(self.command_editor(ctx), ctx, false),
            Command::SaveAs => self.save_editor(self.command_editor(ctx), ctx, true),
            Command::Terminal => self.toggle_terminal(ctx),
            Command::CopyTerminal => self.copy_terminal_output(ctx),
            Command::Sidebar => self.toggle_sidebar(ctx),
            Command::Slurm => self.open_slurm(ctx),
            Command::Explorer => {
                let viewport = ctx.viewport_id();
                let visible = self.explorer_visible(viewport)
                    && self.pane_visible(navigation::Pane::Explorer, viewport);
                self.set_explorer_visible(viewport, !visible);
            }
            Command::Find => {
                self.markdown_source();
                self.editor.show_find = true;
                self.editor.find_focus = true;
            }
            Command::Shortcuts => self.set_dialog(Dialog::Shortcuts),
            Command::Python => self.open_python_environment(ctx),
            Command::Format => self.format_active(true, false, ctx),
            Command::Check => self.format_active(false, false, ctx),
            Command::Fix => self.format_active(true, true, ctx),
            Command::Run => self.run_active(false, ctx),
            Command::AgentSessions => self.open_background_agents(ctx),
            Command::DebugStart => self.start_debugger(ctx),
            Command::DebugStop => self.stop_debugger(),
            Command::Breakpoint => self.toggle_cursor_breakpoint(),
            Command::ReloadImage => {
                if let Some(doc) = self.documents.active() {
                    self.reload_image(doc.id, ctx);
                }
            }
            Command::Markdown => self.toggle_markdown(ctx),
            Command::MarpNew => self.submit_editor_operation(
                editor_operations::Operation::Create {
                    viewport: self.command_editor(ctx),
                    template: editor_operations::Template::Presentation,
                },
                ctx,
            ),
            Command::LatexBuild => self.build_latex(false, ctx),
            Command::LatexPreview => self.build_latex(true, ctx),
            Command::LatexPdf => self.open_latex_pdf(ctx),
            Command::LatexOutput => self.show_latex_output(),
            Command::LatexStop => self.stop_latex(),
            Command::MarpSlides => self.toggle_marp(ctx),
            Command::MarpExport(format) => {
                if let Some(doc) = self.documents.active() {
                    self.marp_export(doc.id, format);
                } else {
                    self.notify("Open a Markdown file to export Marp slides.", false);
                }
            }
            Command::Build => self.run_active(true, ctx),
            Command::PersistentTerminal => self.new_persistent_terminal(ctx),
            Command::NewTerminal => self.new_terminal(ctx),
            Command::Sessions => self.open_sessions(ctx),
            Command::TemporaryTerminal => self.new_temporary_terminal(ctx),
            Command::Extensions => self.set_dialog(Dialog::Extensions),
            Command::Hex => self.set_dialog(Dialog::HexOpen),
            Command::ReloadPdf => {
                if let Some(doc) = self.documents.active() {
                    self.reload_pdf(doc.id, ctx);
                }
            }
            Command::Panel(panel) => self.toggle_panel(panel, ctx),
            Command::UiScaleStep(step) => self.set_ui_scale(self.ui_scale + step, ctx),
            Command::UiScaleReset => self.set_ui_scale(1., ctx),
            Command::TerminalColors => self.terminal_colors.open = true,
            Command::LocalFolder => self.open_local_folder_dialog(),
            Command::SaveAll => self.save_all(ctx),
            Command::Quit => self.guarded(Action::Quit, ctx),
            Command::DebugPause => self.debug_control("pause"),
            Command::DebugOver => self.debug_control("next"),
            Command::DebugIn => self.debug_control("stepIn"),
            Command::DebugOut => self.debug_control("stepOut"),
        }
    }
}
