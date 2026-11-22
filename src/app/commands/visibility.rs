//! Extension ownership for command presentation. This is independent of contextual
//! availability (for example, whether a file is saved or a build is busy).
//! Querying visibility never activates a tool or changes its lifecycle guards.
use super::*;
use crate::extensions::Feature;

impl EditorApp {
    /// Include enabled adapters when exposing shared formatting preferences; they
    /// remain usable independently of the bundled Language tools extension.
    pub(in crate::app) fn code_tools_enabled(&self) -> bool {
        self.feature(Feature::Languages)
            || self.extensions.registry.entries.iter().any(|entry| {
                self.extensions.registry.enabled(&entry.id)
                    && entry
                        .manifest
                        .as_ref()
                        .is_some_and(|manifest| manifest.tool.format || manifest.tool.check)
            })
    }

    /// Hide commands owned by disabled extensions across menus, the activity rail
    /// and the palette. Core file, terminal, settings and extension controls stay visible.
    pub(in crate::app) fn command_visible(&self, command: Command) -> bool {
        let feature = match command {
            Command::Panel(panel) => return self.panel_enabled(panel),
            Command::BookmarkProject => Feature::ProjectBookmarks,
            Command::NewNotebook
            | Command::NotebookRun
            | Command::NotebookRunAll
            | Command::NotebookSource => Feature::Notebooks,
            Command::MarpNew | Command::MarpSlides | Command::MarpExport(_) => Feature::Marp,
            Command::LatexPreview | Command::LatexPdf => {
                return self.feature(Feature::Latex) && self.feature(Feature::Pdf);
            }
            Command::LatexBuild | Command::LatexOutput | Command::LatexStop => Feature::Latex,
            Command::Markdown => Feature::Markdown,
            Command::ReloadImage => Feature::Images,
            Command::ReloadPdf | Command::PdfFullscreen => Feature::Pdf,
            Command::PptxReload | Command::PptxFullscreen => Feature::Pptx,
            Command::DocxReload | Command::DocxFullscreen => Feature::Docx,
            Command::Python => Feature::Python,
            Command::Hex => Feature::Binary,
            Command::AgentSessions => Feature::Agents,
            Command::DebugStart
            | Command::DebugStop
            | Command::DebugPause
            | Command::DebugOver
            | Command::DebugIn
            | Command::DebugOut
            | Command::Breakpoint => Feature::Debugger,
            Command::Format | Command::Check | Command::Fix => {
                let path = self.documents.active().and_then(|doc| doc.path.as_deref());
                let language = crate::app::language(path);
                if command != Command::Fix
                    && (self
                        .extensions
                        .registry
                        .provider(
                            language,
                            command == Command::Format,
                            command == Command::Check,
                        )
                        .is_some()
                        || self
                            .extensions
                            .registry
                            .provider(language, false, true)
                            .is_some())
                {
                    return true;
                }
                return self.feature(Feature::Languages)
                    && (!crate::latex::is_source(path) || self.feature(Feature::Latex));
            }
            Command::Run | Command::Build => {
                if let Some(doc) = self.documents.active() {
                    if crate::latex::is_source(doc.path.as_deref()) {
                        return self.feature(Feature::Latex)
                            && (command == Command::Build || self.feature(Feature::Pdf));
                    }
                    if doc.notebook().is_some() {
                        return self.feature(Feature::Notebooks);
                    }
                }
                Feature::Languages
            }
            // Keep this list exhaustive so adding a command requires an explicit
            // ownership decision instead of silently exposing a disabled feature.
            Command::KeyboardHints
            | Command::Projects
            | Command::Connections
            | Command::AppSettings
            | Command::Environment
            | Command::FindControl
            | Command::New
            | Command::NewEditorWindow
            | Command::Open
            | Command::Folder
            | Command::Connect
            | Command::Tunnels
            | Command::Reconnect
            | Command::Disconnect
            | Command::Encrypted
            | Command::LockEncrypted
            | Command::Save
            | Command::SaveAs
            | Command::Terminal
            | Command::CopyTerminal
            | Command::Sidebar
            | Command::Slurm
            | Command::Explorer
            | Command::Find
            | Command::Shortcuts
            | Command::WordWrap
            | Command::UiScaleStep(_)
            | Command::UiScaleReset
            | Command::TerminalColors
            | Command::PersistentTerminal
            | Command::NewTerminal
            | Command::Sessions
            | Command::TemporaryTerminal
            | Command::Extensions
            | Command::LocalFolder
            | Command::SaveAll
            | Command::Quit => return true,
        };
        self.feature(feature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_extensions_hide_owned_actions_and_leave_core_controls_available() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = EditorApp::new(&cc, Startup::default());
        for (feature, commands) in [
            (Feature::ProjectBookmarks, vec![Command::BookmarkProject]),
            (Feature::Stocks, vec![Command::Panel(Panel::Stocks)]),
            (Feature::Git, vec![Command::Panel(Panel::Git)]),
            (Feature::Monitor, vec![Command::Panel(Panel::Monitor)]),
            (
                Feature::Transfers,
                vec![
                    Command::Panel(Panel::Transfers),
                    Command::Panel(Panel::SharedFolders),
                ],
            ),
            (
                Feature::Agents,
                vec![Command::Panel(Panel::Agent), Command::AgentSessions],
            ),
            (
                Feature::Debugger,
                vec![
                    Command::Panel(Panel::Debug),
                    Command::DebugStart,
                    Command::DebugOver,
                    Command::Breakpoint,
                ],
            ),
            (Feature::Python, vec![Command::Python]),
            (
                Feature::Languages,
                vec![
                    Command::Format,
                    Command::Check,
                    Command::Fix,
                    Command::Run,
                    Command::Build,
                ],
            ),
            (
                Feature::Notebooks,
                vec![
                    Command::NewNotebook,
                    Command::NotebookRun,
                    Command::NotebookSource,
                ],
            ),
            (Feature::Markdown, vec![Command::Markdown]),
            (
                Feature::Marp,
                vec![
                    Command::MarpNew,
                    Command::MarpSlides,
                    Command::MarpExport(crate::marp::Format::Pdf),
                ],
            ),
            (
                Feature::Latex,
                vec![
                    Command::LatexBuild,
                    Command::LatexPreview,
                    Command::LatexOutput,
                ],
            ),
            (Feature::Images, vec![Command::ReloadImage]),
            (
                Feature::Pdf,
                vec![
                    Command::ReloadPdf,
                    Command::PdfFullscreen,
                    Command::LatexPreview,
                ],
            ),
            (
                Feature::Pptx,
                vec![Command::PptxReload, Command::PptxFullscreen],
            ),
            (
                Feature::Docx,
                vec![Command::DocxReload, Command::DocxFullscreen],
            ),
            (Feature::Binary, vec![Command::Hex]),
        ] {
            app.set_extension(feature.id(), false);
            for command in &commands {
                assert!(!app.command_visible(*command), "{feature:?}: {command:?}");
            }
            for command in [
                Command::Open,
                Command::New,
                Command::Save,
                Command::Terminal,
                Command::Extensions,
                Command::Panel(Panel::Problems),
                Command::Environment,
                Command::Panel(Panel::Tasks),
                Command::Panel(Panel::Settings),
            ] {
                assert!(
                    app.command_visible(command),
                    "Core action disappeared: {command:?}"
                );
            }
            app.set_extension(feature.id(), true);
            for command in commands {
                assert!(app.command_visible(command), "{feature:?}: {command:?}");
            }
        }
    }

    #[test]
    fn enabled_adapters_and_latex_keep_their_independent_actions_visible() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents.active_mut().unwrap().fixture_content().path = Some("/tmp/source.tex".into());
        app.set_extension(Feature::Languages.id(), false);
        assert!(app.command_visible(Command::Run));
        app.set_extension(Feature::Pdf.id(), false);
        assert!(app.command_visible(Command::LatexBuild));
        assert!(!app.command_visible(Command::LatexPreview));
        assert!(!app.command_visible(Command::Run));
        assert!(app.command_visible(Command::Build));
        app.set_extension(Feature::Pdf.id(), true);
        app.set_extension(Feature::Latex.id(), false);
        assert!(!app.command_visible(Command::Run));
        app.documents.active_mut().unwrap().fixture_content().path = Some("/tmp/source.rs".into());
        let manifest:crate::extensions::Manifest=serde_json::from_value(serde_json::json!({"api":1,"id":"test.format","name":"Test formatter","version":"1","description":"fixture","tool":{"program":"unused-test-tool","languages":["rust"],"format":true,"check":true}})).unwrap();
        manifest.validate().unwrap();
        app.extensions
            .registry
            .entries
            .push(crate::extensions::Entry {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
                description: manifest.description.clone(),
                activation: "Explicit",
                feature: None,
                manifest: Some(manifest),
            });
        app.extensions.registry.set_enabled("test.format", true);
        assert!(app.code_tools_enabled());
        assert!(app.command_visible(Command::Format));
        assert!(app.command_visible(Command::Check));
        assert!(!app.command_visible(Command::Fix));
        app.extensions.registry.set_enabled("test.format", false);
        assert!(!app.code_tools_enabled());
        assert!(!app.command_visible(Command::Format));
    }
}
