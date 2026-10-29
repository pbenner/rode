//! Same-document Marp slides. Source stays authoritative and uses normal saves.
//!
//! Slide previews are explicit worker snapshots, not a continuously running server. Source,
//! preview and export versions are tracked separately; changing context cancels old rendering
//! without discarding source edits.
use super::*;
use crate::marp::{self, Destination, Format, Invocation, Settings};

pub(super) enum Completed {
    Preview(crate::pdf::Loaded, String),
    Export(marp::Reply),
}
pub(super) struct View {
    pub showing: bool,
    pub pdf: Option<pdf_view::PdfView>,
    attempted: Option<(u64, Settings, String)>,
    busy: bool,
    token: u64,
    cancel: Arc<AtomicBool>,
    exporting: bool,
    export_token: u64,
    export_cancel: Arc<AtomicBool>,
    focus: bool,
    error: String,
    output: String,
}
impl Default for View {
    fn default() -> Self {
        Self {
            showing: false,
            pdf: None,
            attempted: None,
            busy: false,
            token: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            exporting: false,
            export_token: 0,
            export_cancel: Arc::new(AtomicBool::new(false)),
            focus: true,
            error: String::new(),
            output: String::new(),
        }
    }
}
impl View {
    pub fn stop_render(&mut self) {
        if self.busy {
            self.attempted = None;
        }
        self.cancel.store(true, Ordering::Release);
        self.token = self.token.wrapping_add(1);
        self.busy = false;
        if let Some(pdf) = &mut self.pdf {
            pdf.invalidate();
        }
    }
    pub fn invalidate(&mut self) {
        self.stop_render();
        self.attempted = None;
        self.export_cancel.store(true, Ordering::Release);
        self.export_token = self.export_token.wrapping_add(1);
        self.exporting = false;
        self.pdf = None;
        self.showing = false;
    }
}
impl Drop for View {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.export_cancel.store(true, Ordering::Release);
    }
}

impl EditorApp {
    pub(super) fn new_marp(&mut self) {
        if !self.feature(crate::extensions::Feature::Marp) {
            self.notify("Enable Marp presentations in Extensions.", false);
            return;
        }
        self.new_file();
        let doc = &mut self.documents[self.active];
        doc.text = marp::TEMPLATE.into();
        doc.modified = true;
        doc.edit_version = doc.edit_version.wrapping_add(1);
        doc.lines = doc.text.lines().count();
        doc.marp = Some(View::default());
    }
    pub(super) fn toggle_marp(&mut self, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Marp) {
            self.notify("Enable Marp presentations in Extensions.", false);
            return;
        }
        let Some(doc) = self.documents.get(self.active).filter(|doc| {
            !doc.is_preview()
                && (doc.marp.is_some() || doc.path.as_deref().is_some_and(markdown::is_path))
        }) else {
            self.notify(
                "Open a Markdown file or create a Marp presentation first.",
                false,
            );
            return;
        };
        let showing = doc.marp.as_ref().is_some_and(|view| view.showing);
        self.markdown_source();
        let view = self.documents[self.active]
            .marp
            .get_or_insert_with(View::default);
        view.showing = !showing;
        view.focus = true;
        if showing {
            self.focus_editor(ctx);
        }
    }
    pub(super) fn marp_tick(&mut self) {
        for (index, doc) in self.documents.iter_mut().enumerate() {
            if let Some(view) = &mut doc.marp
                && (index != self.active
                    || !view.showing
                    || view
                        .attempted
                        .as_ref()
                        .is_some_and(|(version, _, _)| *version != doc.edit_version))
                && (view.busy || view.pdf.as_ref().is_some_and(|pdf| pdf.image.loading))
            {
                view.stop_render();
            }
        }
    }
    pub(super) fn marp_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let id = self.documents[self.active].id;
        let mut refresh = false;
        let mut export = None;
        let mut settings = false;
        {
            let view = self.documents[self.active].marp.as_mut().unwrap();
            ui.horizontal_wrapped(|ui| {
                refresh = ui
                    .add_enabled(!view.busy, egui::Button::new("Refresh slides").small())
                    .clicked();
                if view.busy {
                    ui.spinner();
                    ui.label("Rendering Marp…");
                    if ui.small_button("Cancel").clicked() {
                        let attempted = view.attempted.clone();
                        view.stop_render();
                        view.attempted = attempted;
                        view.error = "Rendering cancelled. Use Refresh slides to try again.".into();
                    }
                }
                ui.add_enabled_ui(!view.exporting, |ui| {
                    ui.menu_button("Export", |ui| {
                        for (format, label) in [
                            (Format::Html, "HTML"),
                            (Format::Pdf, "PDF"),
                            (Format::Pptx, "PowerPoint (.pptx)"),
                        ] {
                            if ui.button(label).clicked() {
                                export = Some(format);
                                ui.close();
                            }
                        }
                    });
                });
                if view.exporting {
                    ui.spinner();
                    ui.label("Exporting snapshot…");
                    if ui.small_button("Cancel export").clicked() {
                        view.export_cancel.store(true, Ordering::Release);
                        view.export_token = view.export_token.wrapping_add(1);
                        view.exporting = false;
                    }
                }
                settings = ui.small_button("Marp settings").clicked();
                ui.label(palette.tag("PROJECT-HOST RENDERER"));
            });
            if !view.error.is_empty() {
                ui.colored_label(palette.error, &view.error);
            }
            if !view.output.is_empty() {
                ui.collapsing("Marp tool output", |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(100.)
                        .show(ui, |ui| {
                            ui.monospace(&view.output);
                        });
                });
            }
            if refresh {
                view.attempted = None;
            }
        }
        if settings {
            self.toggle_panel(Panel::Settings, ctx);
        }
        if let Some(format) = export {
            self.marp_export(id, format);
        }
        if self
            .workspace
            .as_ref()
            .is_some_and(|w| !w.ready || !w.connected)
        {
            ui.label("Connect and finish opening the project to render slides.");
            return;
        }
        let doc = &self.documents[self.active];
        let path = doc
            .path
            .clone()
            .unwrap_or_else(|| self.join_root("untitled.marp.md"));
        let settings = self.workbench.marp_settings();
        let needs_render = doc.marp.as_ref().is_some_and(|view| {
            !view.busy
                && !view
                    .attempted
                    .as_ref()
                    .is_some_and(|(version, used, source)| {
                        *version == doc.edit_version && used == settings && source == &path
                    })
        });
        if needs_render {
            self.start_marp(id, Format::Pdf, None, ctx);
        }
        let focus = self.take_editor_focus(ui, ctx).is_some()
            || self.documents[self.active]
                .marp
                .as_ref()
                .is_some_and(|view| view.focus);
        let keyboard = self.dialog == Dialog::None
            && self.pending.is_none()
            && !self.workbench_modal()
            && !egui::Popup::is_any_open(ctx);
        let view = self.documents[self.active].marp.as_mut().unwrap();
        let Some(pdf) = &mut view.pdf else {
            ui.label("Slides use an installed Marp CLI and browser on the project host, and desktop Poppler for display.");
            ui.label("The current buffer is rendered without saving it.");
            return;
        };
        view.focus = false;
        let (reload, page) = pdf.ui(ui, id, palette, focus, keyboard, "Slide");
        if reload {
            view.attempted = None;
        } else if let Some((page, entry)) = page {
            self.marp_render_page(id, page, entry, ctx);
        }
    }
    pub(super) fn marp_export(&mut self, id: u64, format: Format) {
        if !self.feature(crate::extensions::Feature::Marp) {
            self.notify("Enable Marp presentations in Extensions.", false);
            return;
        }
        if !self.documents.iter().any(|doc| {
            doc.id == id
                && !doc.is_preview()
                && (doc.marp.is_some() || doc.path.as_deref().is_some_and(markdown::is_path))
        }) {
            self.notify("Open a Markdown file to export Marp slides.", false);
            return;
        }
        self.set_dialog(Dialog::MarpExport(id, format));
    }
    pub(super) fn marp_export_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Dialog::MarpExport(id, format) = self.dialog else {
            return;
        };
        ui.heading(format!(
            "Export Marp · {}",
            format.extension().to_ascii_uppercase()
        ));
        ui.label(
            "Export the current buffer on its project host. The Markdown source stays unsaved.",
        );
        let input = ui.add(
            egui::TextEdit::singleline(&mut self.dialog_input)
                .desired_width(f32::INFINITY)
                .hint_text("Output path"),
        );
        if self.dialog_focus {
            input.request_focus();
            self.dialog_focus = false;
        }
        ui.checkbox(&mut self.marp_overwrite, "Replace existing export");
        ui.small("Existing exports are checked before conversion and replaced atomically; a concurrent change prevents replacement.");
        ui.horizontal(|ui| {
            if ui.button("Export snapshot").clicked()
                || ((input.has_focus() || input.lost_focus())
                    && ui.input(|i| i.key_pressed(Key::Enter)))
            {
                let destination = Destination {
                    path: self.dialog_input.trim().to_owned(),
                    overwrite: self.marp_overwrite,
                };
                self.dialog = Dialog::None;
                self.start_marp(id, format, Some(destination), ctx);
                self.focus_editor(ctx);
            }
            if ui.button("Cancel").clicked() {
                self.dialog = Dialog::None;
                self.focus_editor(ctx);
            }
        });
    }
    fn start_marp(
        &mut self,
        id: u64,
        format: Format,
        destination: Option<Destination>,
        ctx: &egui::Context,
    ) {
        if !self.feature(crate::extensions::Feature::Marp) {
            return;
        }
        if self
            .documents
            .iter()
            .any(|doc| doc.id == id && doc.recovery.is_some())
        {
            self.notify(
                "Reconnect or Save As this disconnected presentation before using Marp.",
                true,
            );
            return;
        }
        if self
            .workspace
            .as_ref()
            .is_some_and(|w| !w.ready || !w.connected)
        {
            self.notify(
                "Connect and finish opening the project before using Marp.",
                true,
            );
            return;
        }
        let root = self
            .workspace
            .as_ref()
            .map(|w| w.root.clone())
            .unwrap_or_else(|| {
                self.documents
                    .iter()
                    .find(|doc| doc.id == id)
                    .and_then(|doc| doc.path.as_ref())
                    .and_then(|path| std::path::Path::new(path).parent())
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| self.root())
            });
        let Some(doc) = self.documents.iter_mut().find(|doc| doc.id == id) else {
            return;
        };
        let view = doc.marp.get_or_insert_with(View::default);
        let exporting = destination.is_some();
        if (exporting && view.exporting) || (!exporting && view.busy) {
            return;
        }
        let path = doc.path.clone().unwrap_or_else(|| {
            std::path::Path::new(&root)
                .join("untitled.marp.md")
                .to_string_lossy()
                .into()
        });
        let settings = self.workbench.marp_settings().clone();
        if let Err(error) = settings.validate() {
            self.notify(error.to_string(), true);
            return;
        }
        let (token, cancel) = if exporting {
            view.export_token = view.export_token.wrapping_add(1);
            view.export_cancel = Arc::new(AtomicBool::new(false));
            view.exporting = true;
            (view.export_token, view.export_cancel.clone())
        } else {
            view.token = view.token.wrapping_add(1);
            view.cancel = Arc::new(AtomicBool::new(false));
            view.busy = true;
            view.attempted = Some((doc.edit_version, settings.clone(), path.clone()));
            (view.token, view.cancel.clone())
        };
        view.error.clear();
        let invocation = Invocation {
            root,
            path,
            text: doc.text.clone(),
            settings,
            format,
            destination,
        };
        let page = view.pdf.as_ref().map_or(1, |pdf| pdf.page);
        let version = doc.edit_version;
        let generation = self.generation;
        let plan = self.workspace.as_ref().map(|w| w.plan.clone());
        let remote = self.workspace.as_ref().and_then(|w| w.remote.clone());
        let gate = self.marp_gate.clone();
        let budget = self.image_budget.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let limit = ctx.input(|i| i.max_texture_side);
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let result = (|| -> anyhow::Result<Completed> {
                let _gate = gate.lock().unwrap_or_else(|error| error.into_inner());
                anyhow::ensure!(!cancel.load(Ordering::Acquire), "Marp task cancelled.");
                let reply = if let Some(remote) = remote {
                    remote.marp_request(&invocation, plan.as_ref(), &cancel)?
                } else {
                    // Even normal-mode tools use a lifetime coordinator: GUI
                    // exit closes stdin, allowing it to shut down Marp's browser.
                    let plan = plan.unwrap_or_else(|| crate::sandbox::Plan {
                        root: invocation.root.clone(),
                        ..Default::default()
                    });
                    let (command, _helper) = plan.coordinator_command("--marp-helper")?;
                    marp::execute(command, &invocation, &cancel)?
                };
                if exporting {
                    return Ok(Completed::Export(reply));
                }
                let _io = budget.io.lock().unwrap_or_else(|error| error.into_inner());
                let loaded = crate::pdf::load_cancellable(
                    reply
                        .data
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("Marp preview did not return a PDF"))?,
                    page,
                    &budget,
                    limit,
                    &cancel,
                )?;
                anyhow::ensure!(
                    loaded.source.pages <= 512,
                    "Marp preview supports at most 512 slides."
                );
                Ok(Completed::Preview(loaded, reply.warnings))
            })()
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(Event::Marp {
                generation,
                id,
                version,
                token,
                exporting,
                result: Box::new(result),
            });
            ctx.request_repaint();
        });
    }
    pub(super) fn marp_complete(
        &mut self,
        id: u64,
        version: u64,
        token: u64,
        exporting: bool,
        result: Result<Completed, String>,
        ctx: &egui::Context,
    ) {
        let Some(doc) = self.documents.iter_mut().find(|doc| doc.id == id) else {
            return;
        };
        let Some(view) = &mut doc.marp else {
            return;
        };
        if exporting {
            if token != view.export_token {
                return;
            }
            view.exporting = false;
        } else {
            if token != view.token {
                return;
            }
            view.busy = false;
            if doc.edit_version != version {
                return;
            }
        }
        match result {
            Ok(Completed::Preview(loaded, output)) => {
                if let Some(pdf) = &mut view.pdf {
                    pdf.replace(loaded);
                } else {
                    view.pdf = Some(pdf_view::PdfView::new(loaded));
                }
                view.output = output;
                view.focus = true;
            }
            Ok(Completed::Export(reply)) => {
                view.output = reply.warnings;
                let path = reply.path.unwrap_or_default();
                let parent = std::path::Path::new(&path)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned());
                let message = format!(
                    "Exported {}{}",
                    path,
                    if doc.edit_version != version {
                        " (submitted snapshot; buffer has newer edits)"
                    } else {
                        ""
                    }
                );
                self.notify(message, false);
                if let Some(parent) = parent
                    && self.expanded.contains(&parent)
                {
                    self.list(parent, ctx);
                }
            }
            Err(error) => {
                view.error = error.clone();
                self.notify(error, true);
            }
        }
    }
    fn marp_render_page(
        &mut self,
        id: u64,
        page: usize,
        entry: pdf_view::PageEntry,
        ctx: &egui::Context,
    ) {
        let Some(doc) = self.documents.iter_mut().find(|doc| doc.id == id) else {
            return;
        };
        let Some(pdf) = doc.marp.as_mut().and_then(|view| view.pdf.as_mut()) else {
            return;
        };
        let Some((source, cancel, token)) = pdf.begin_page(page, entry) else {
            return;
        };
        let version = doc.edit_version;
        let generation = self.generation;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let budget = self.image_budget.clone();
        let limit = ctx.input(|i| i.max_texture_side);
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let _io = budget.io.lock().unwrap_or_else(|error| error.into_inner());
            let result = source
                .render(page, &budget, limit, &cancel)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(Event::MarpPage {
                generation,
                id,
                version,
                token,
                page,
                result,
            });
            ctx.request_repaint();
        });
    }
    pub(super) fn marp_page(
        &mut self,
        id: u64,
        version: u64,
        token: u64,
        page: usize,
        result: Result<crate::images::Decoded, String>,
    ) {
        let Some(doc) = self
            .documents
            .iter_mut()
            .find(|doc| doc.id == id && doc.edit_version == version)
        else {
            return;
        };
        if let Some(pdf) = doc.marp.as_mut().and_then(|view| view.pdf.as_mut())
            && let Some(error) = pdf.complete_page(token, page, result)
        {
            self.notify(error, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disconnected_presentation_never_starts_a_local_renderer() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_marp();
        app.documents[0].path = Some("/remote/slides.marp.md".into());
        app.workspace = Some(crate::app::tests::remote_workspace());
        app.detach_ssh(&ctx);
        app.start_marp(app.documents[0].id, Format::Pdf, None, &ctx);
        assert!(!app.documents[0].marp.as_ref().unwrap().busy);
        assert!(
            app.notice
                .as_ref()
                .unwrap()
                .0
                .contains("disconnected presentation")
        );
    }

    #[test]
    fn new_presentations_keep_source_save_and_unsaved_guards() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_marp();
        let doc = &app.documents[app.active];
        assert!(doc.dirty() && !doc.is_preview());
        assert_eq!(doc.text, marp::TEMPLATE);
        assert!(doc.saved_text.is_empty());
        app.save_active(&ctx, false);
        assert!(matches!(app.dialog, Dialog::Save(_)));
        assert!(app.dialog_input.ends_with("untitled.marp.md"));
        app.dialog = Dialog::None;
        app.guarded(Action::Quit, &ctx);
        assert!(matches!(app.pending, Some(Action::Quit)));
        assert!(!app.quitting);
    }

    #[test]
    fn export_errors_do_not_clear_an_independent_preview_job() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_marp();
        let doc = &mut app.documents[app.active];
        let id = doc.id;
        let version = doc.edit_version;
        let view = doc.marp.as_mut().unwrap();
        view.busy = true;
        view.exporting = true;
        view.token = 1;
        view.export_token = 1;
        app.marp_complete(id, version, 1, true, Err("Export refused".into()), &ctx);
        let view = app.documents[app.active].marp.as_ref().unwrap();
        assert!(view.busy && !view.exporting);
        assert_eq!(app.documents[app.active].text, marp::TEMPLATE);
    }

    #[test]
    fn switching_to_source_retains_a_completed_preview_but_cancels_inflight_work() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_marp();
        let doc = &mut app.documents[app.active];
        let version = doc.edit_version;
        let view = doc.marp.as_mut().unwrap();
        view.showing = true;
        view.attempted = Some((version, Settings::default(), "deck.md".into()));
        app.markdown_source();
        let view = app.documents[app.active].marp.as_mut().unwrap();
        assert!(!view.showing && view.attempted.is_some());
        view.busy = true;
        let cancel = view.cancel.clone();
        app.markdown_source();
        let view = app.documents[app.active].marp.as_ref().unwrap();
        assert!(cancel.load(Ordering::Acquire) && !view.busy && view.attempted.is_none());
    }
}
