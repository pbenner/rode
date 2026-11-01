//! Notebook toolbar, cell editors and virtualized output presentation.
//!
//! Editors grow to fit their source. Only the notebook owns scrolling; cached
//! cell heights skip offscreen layouts without creating nested scroll areas.

use super::*;

impl EditorApp {
    /// Render cell controls, growing source editors and bounded outputs. Defer run/host
    /// actions until the document borrow ends so rendering never directly launches a process.
    pub(in crate::app) fn notebook_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        let index = self.active;
        let Some(mut view) = self
            .documents
            .get_mut(index)
            .and_then(|d| d.notebook.take())
        else {
            return false;
        };
        if self.python_environments().is_none() && self.python_environment_error().is_empty() {
            self.refresh_environment(false, ctx);
        }
        view.pictures(ctx);
        let palette = self.themes.active().palette;
        let font_size = self.font_size;
        let mut action = None;
        let mut jobs = Vec::new();
        let mut changed = false;
        let mut link = None;
        let focus = if view.source_mode {
            false
        } else {
            self.take_editor_focus(ui, ctx).is_some()
        };
        if focus {
            view.focus = Some(view.active);
        }
        let id = self.documents[index].id;
        let path = self.documents[index]
            .path
            .clone()
            .unwrap_or_else(|| self.join_root("untitled.ipynb"));
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(12, 5))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if view.source_mode {
                        ui.label(palette.tag("JSON SOURCE"));
                        if ui.button("Back to cells").clicked() {
                            action = Some(Action::Source);
                        }
                        return;
                    }
                    let busy = view.kernel.as_ref().is_some_and(Kernel::busy);
                    if ui
                        .add_enabled(!busy, IconButton::new(Icon::Play, "Run cell"))
                        .clicked()
                    {
                        action = Some(Action::Run(false));
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Run all"))
                        .clicked()
                    {
                        action = Some(Action::Run(true));
                    }
                    if ui
                        .add_enabled(view.kernel.is_some(), egui::Button::new("Interrupt"))
                        .clicked()
                    {
                        action = Some(Action::Interrupt);
                    }
                    ui.separator();
                    let insert_at = view.active_index().map_or(view.book.cells.len(), |at| at + 1);
                    if ui.button("+ Code cell").on_hover_text("Insert a code cell below the selected cell").clicked() {
                        action = Some(Action::Add("code", insert_at));
                    }
                    if ui.button("+ Markdown cell").on_hover_text("Insert a text cell below the selected cell").clicked() {
                        action = Some(Action::Add("markdown", insert_at));
                    }
                    ui.menu_button("Notebook…", |ui| {
                        if ui.button("Clear all outputs").clicked() {
                            action = Some(Action::Clear(None));
                            ui.close();
                        }
                        if ui.add_enabled(view.kernel.is_some(), egui::Button::new("Shut down kernel")).clicked() {
                            action = Some(Action::Shutdown);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Edit JSON source").clicked() {
                            action = Some(Action::Source);
                            ui.close();
                        }
                    });
                });
                if !view.source_mode {
                    ui.horizontal_wrapped(|ui| {
                        let status = view.kernel.as_ref().map(|k| k.status.as_str()).unwrap_or("Not started");
                        if view.kernel.as_ref().is_some_and(Kernel::busy) { ui.spinner(); }
                        ui.label(palette.tag(status)).on_hover_text(&view.status);
                        ui.label(if view.kernel_name == "python3" { "Python · selected environment" } else { &view.kernel_name });
                        let execution = view.kernel.as_ref().map(|k| k.execution.clone()).unwrap_or_else(||
                            view.execution_target.as_ref().map(|t| format!("Slurm job {}", t.id)).unwrap_or_else(|| "Project host".into()));
                        ui.label(egui::RichText::new(execution).small().color(palette.muted));
                    });
                    egui::CollapsingHeader::new("Kernel settings")
                        .id_salt(("notebook-settings", id))
                        .show(ui, |ui| {

                    ui.horizontal_wrapped(|ui| {
                        let on = view.kernel.as_ref().map_or(view.persistent, Kernel::persistent);
                        if ui.add_enabled(
                            view.kernel.as_ref().is_none_or(|k| !k.pending()),
                            egui::Button::new(if on { "Persist kernel: on" } else { "Persist kernel: off" }),
                        ).on_hover_text("Keep the kernel, queued cells and outputs on the project host after closing this notebook, exiting RODE or losing SSH. Reattach from Sessions with the same project and sandbox.").clicked() {
                            if !on && self.documents[index].path.is_none() {
                                view.error = "Save this notebook before enabling persistence".into();
                            } else if let Some(kernel) = &mut view.kernel {
                                match kernel.set_persistent(!on) {
                                    Ok(()) => view.persistent = !on,
                                    Err(error) => view.error = error.to_string(),
                                }
                            } else {
                                view.persistent = !on;
                            }
                        }
                        if ui.button("Running kernels…").clicked() {
                            self.open_sessions(ctx);
                        }
                        if view.kernel.is_none() && ui.button("Reattach kernel").clicked() {
                            if let Some(path) = &self.documents[index].path {
                                self.find_notebook_runtime(path.clone(), ctx);
                            } else {
                                view.error = "Save this notebook before reattaching a kernel".into();
                            }
                        }
                        if view.kernel.as_ref().is_some_and(Kernel::persistent)
                            && ui.add_enabled(
                                view.kernel.as_ref().is_none_or(|k| !k.pending()),
                                egui::Button::new("Detach kernel"),
                            ).clicked()
                        {
                            view.detach("Kernel detached · Reattach from Sessions");
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Execution target");
                        let mut selected = view.execution_target.clone();
                        let (queue, _) = self.slurm_programs();
                        let runner = self.workbench.slurm_allocation().runner.clone();
                        egui::ComboBox::from_id_salt(("notebook-execution", id))
                            .selected_text(
                                selected.as_ref()
                                    .map(|t| format!("Slurm job {}", t.id))
                                    .unwrap_or_else(|| "Project host".into()),
                            )
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut selected, None, "Project host");
                                if let Some(snapshot) = &self.sessions.slurm {
                                    for job in &snapshot.jobs {
                                        if job.state == "RUNNING"
                                            && job.id.bytes().all(|b| b.is_ascii_digit())
                                        {
                                            let target = crate::slurm::Target {
                                                id: job.id.clone(),
                                                name: job.name.clone(),
                                                queue: queue.clone(),
                                                runner: runner.clone(),
                                            };
                                            ui.selectable_value(
                                                &mut selected, Some(target),
                                                format!("Job {} · {} · {}", job.id, job.nodes, job.name),
                                            );
                                        }
                                    }
                                }
                            }).response.on_hover_text(
                                "Changing execution target stops this kernel and clears its live variables. Run a cell to start a new kernel. Slurm kernels share the selected allocation's resources."
                            );
                        view.set_execution_target(selected);
                        if ui.add_enabled(
                            !self.sessions.slurm_busy,
                            egui::Button::new("Refresh Slurm targets"),
                        ).clicked() {
                            self.refresh_slurm(None, ctx);
                        }
                        if ui.button("Allocate…").clicked() {
                            self.open_slurm(ctx);
                        }
                        if let Some(kernel) = &view.kernel {
                            ui.small(&kernel.execution);
                        }
                    });
                    if !self.sessions.slurm_error.is_empty() {
                        ui.colored_label(palette.muted, &self.sessions.slurm_error);
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Python environment");
                        let previous = self.terminal_python_selection();
                        ui.push_id(("notebook-python", id), |ui| self.python_environment_picker(ui, ctx));
                        if previous != self.terminal_python_selection() {
                            view.detach("Notebook environment changed");
                            view.kernel_name = "python3".into();
                            action = Some(Action::Kernel);
                        }
                        if ui.add_enabled(!self.python_environment_busy(), egui::Button::new("Refresh kernels")).clicked() {
                            self.refresh_environment(false, ctx);
                        }
                        if self.python_environment_busy() { ui.spinner(); }
                    });
                    if !self.python_environment_error().is_empty() {
                        ui.colored_label(palette.error, self.python_environment_error());
                    }
                    if let Some(env) = self.python_environments()
                        && !env.kernel_error.is_empty() {
                        ui.colored_label(palette.muted, &env.kernel_error);
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Kernel");
                        let previous = view.kernel_name.clone();
                        egui::ComboBox::from_id_salt(("notebook-kernel", id))
                            .width(300.)
                            .selected_text(if view.kernel_name == "python3" {
                                "Python · selected environment".into()
                            } else {
                                self.python_environments()
                                    .and_then(|env| env.kernels.iter().find(|k| k.name == view.kernel_name))
                                    .map(|k| k.display_name.clone())
                                    .unwrap_or_else(|| view.kernel_name.clone())
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut view.kernel_name, "python3".into(), "Python · selected environment");
                                if let Some(env) = self.python_environments() {
                                    for kernel in &env.kernels {
                                        if kernel.name != "python3" {
                                            ui.selectable_value(&mut view.kernel_name, kernel.name.clone(), format!("{} ({})", kernel.display_name, kernel.name));
                                        }
                                    }
                                }
                            });
                        if previous != view.kernel_name { action = Some(Action::Kernel); }
                    });
                    });
                    if self.show_find {
                        ui.horizontal(|ui| {
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.find)
                                    .hint_text("Find in cell sources…")
                                    .desired_width(220.),
                            );
                            if self.find_focus {
                                response.request_focus();
                                self.find_focus = false;
                            }
                            if (ui.small_button("Next").clicked()
                                || (response.has_focus() || response.lost_focus())
                                    && ui.input(|i| i.key_pressed(Key::Enter)))
                                && !self.find.is_empty()
                            {
                                let start = view.active_index().unwrap_or(0);
                                if let Some((at, range)) = (0..view.book.cells.len())
                                    .map(|n| (start + n + 1) % view.book.cells.len())
                                    .find_map(|at| {
                                        find_matches(
                                            &view.book.cells[at].source,
                                            &self.find,
                                            self.case_sensitive,
                                        )
                                        .first()
                                        .copied()
                                        .map(|r| (at, r))
                                    })
                                {
                                    let cell = &view.book.cells[at];
                                    let a = cell.source[..range.0].chars().count();
                                    let b = cell.source[..range.1].chars().count();
                                    view.active = cell.uid;
                                    view.selection = Some((cell.uid, a, b));
                                    view.focus = Some(cell.uid);
                                    view.reveal = true;
                                    view.cells.entry(cell.uid).or_default().previewing = false;
                                }
                            }
                            if ui.small_button("Close").clicked() {
                                self.show_find = false;
                            }
                        });
                    }
                    if !view.error.is_empty() {
                        ui.colored_label(palette.error, &view.error);
                    }
                }
            });
        if !view.source_mode {
            let mut visible = HashSet::new();
            let scroll_offset = if view.reveal {
                view.reveal = false;
                view.active_index().map(|at| {
                    view.book.cells[..at]
                        .iter()
                        .map(|c| view.cells.get(&c.uid).map(|c| c.height).unwrap_or(150.))
                        .sum::<f32>()
                })
            } else {
                None
            };
            let mut scroll = egui::ScrollArea::vertical()
                .id_salt(("notebook", id))
                .auto_shrink([false, false]);
            if let Some(y) = scroll_offset {
                scroll = scroll.vertical_scroll_offset(y).animated(false);
            }
            let themes = &self.themes;
            let focus_cell = view.focus;
            let selection = view.selection;
            scroll.show_viewport(ui, |ui, viewport| {
                ui.spacing_mut().item_spacing.y = 0.;
                let mut y = 0.;
                let cell_count = view.book.cells.len();
                for at in 0..cell_count {
                    let uid = view.book.cells[at].uid;
                    let height = view.cells.entry(uid).or_default().height;
                    if y + height < viewport.top() - 60. || y > viewport.bottom() + 60. {
                        ui.add_space(height);
                        y += height;
                        continue;
                    }
                    visible.insert(uid);
                    let top = ui.cursor().top();
                    let active = view.active == uid;
                    let cell = &mut view.book.cells[at];
                    let state = view.cells.get_mut(&uid).unwrap();
                    let frame = egui::Frame::new()
                        .fill(if active { palette.raised } else { palette.panel })
                        .stroke(Stroke::new(1_f32, if active { palette.accent } else { palette.border }))
                        .inner_margin(8.)
                        .outer_margin(egui::Margin::symmetric(10, 5));
                    frame.show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.push_id(("notebook-cell", id, uid), |ui| {
                            cell_toolbar(ui, |ui| {
                                if ui.add(egui::Button::new(format!("Cell {}", at + 1))
                                    .selected(active).frame(false).min_size(egui::vec2(52., 28.))).clicked()
                                {
                                    view.active = uid;
                                    view.focus = Some(uid);
                                }
                                let mut kind = cell.kind().to_owned();
                                egui::ComboBox::from_id_salt("kind").width(108.).selected_text(&kind)
                                    .show_ui(ui, |ui| {
                                        for value in ["code", "markdown", "raw"] {
                                            ui.selectable_value(&mut kind, value.into(), value);
                                        }
                                    });
                                if kind != cell.kind() { action = Some(Action::Kind(at, kind)); }
                                if cell.kind() == "code" {
                                    if ui.add_enabled(!view.kernel.as_ref().is_some_and(Kernel::busy),
                                        IconButton::new(Icon::Play, "Run").min_size(egui::vec2(68., 28.)))
                                        .on_hover_text("Run cell").clicked()
                                    {
                                        view.active = uid;
                                        action = Some(Action::Run(false));
                                    }
                                } else if cell.kind() == "markdown" {
                                    if ui.add(egui::Button::new(if state.previewing { "Edit" } else { "Preview" })
                                        .min_size(egui::vec2(68., 28.))).clicked()
                                    {
                                        state.previewing = !state.previewing;
                                        view.active = uid;
                                        view.focus = Some(uid);
                                    }
                                } else {
                                    ui.allocate_space(egui::vec2(68., 28.));
                                }
                                if ui.add_enabled(at > 0, egui::Button::new("Up"))
                                    .on_hover_text("Move cell up").clicked()
                                { action = Some(Action::Move(at, -1)); }
                                if ui.add_enabled(at + 1 < cell_count, egui::Button::new("Down"))
                                    .on_hover_text("Move cell down").clicked()
                                { action = Some(Action::Move(at, 1)); }
                                if ui.button("Delete").on_hover_text("Delete cell").clicked()
                                { action = Some(Action::Delete(at)); }
                                if cell.kind() == "code" {
                                    if ui.button("Clear").on_hover_text("Clear cell outputs").clicked()
                                    { action = Some(Action::Clear(Some(at))); }
                                    let running = view.kernel.as_ref().is_some_and(|k| k.running_uid() == Some(uid));
                                    ui.label(palette.tag(if running { "[*]".into() } else {
                                        format!("[{}]", cell.data["execution_count"].as_u64()
                                            .map(|n| n.to_string()).unwrap_or_default())
                                    })).on_hover_text("Execution count");
                                }
                            });
                            ui.add_space(8.);
                            if cell.kind() == "markdown" && state.previewing {
                                let preview = state.preview.get_or_insert_with(|| markdown::Preview::new(view.images.clone()));
                                let mut embedded = HashMap::new();
                                let mut attachment_data = HashMap::new();
                                if let Some(attachments) = cell.data["attachments"].as_object() {
                                    for (name, bundle) in attachments {
                                        if let Some((data, path)) = image_bundle(bundle) {
                                            let key = format!("attachment:{name}");
                                            embedded.insert(key.clone(), key.clone());
                                            attachment_data.insert(key, (data, path));
                                        }
                                    }
                                }
                                let requests = preview.prepare_embedded(cell.version, &path, &cell.source, &embedded);
                                let token = preview.token;
                                let cancel = preview.cancelled();
                                for (uri, target) in requests {
                                    jobs.push(PictureJob {
                                        target: Target::Markdown { uid, token, uri },
                                        encoded: attachment_data.get(&target).cloned(),
                                        path: target,
                                        cancel: cancel.clone()
                                    });
                                }
                                link = preview.inline_ui(ui, font_size, palette).or(link.take());
                            } else {
                                let language = match cell.kind() {
                                    "code" => view.book.data["metadata"]["language_info"]["name"].as_str().unwrap_or("python"),
                                    "markdown" => "Markdown",
                                    _ => "text",
                                };
                                let selected = selection.filter(|(owner, _, _)| *owner == uid)
                                    .map(|(_, start, end)| (start, end));
                                let edit = source::Editor { themes, document: id, font_size, palette }
                                    .show(ui, cell, language, focus_cell == Some(uid), selected);
                                if selected.is_some() { view.selection = None; }
                                if focus_cell == Some(uid) { view.focus = None; }
                                if edit.focused { view.active = uid; }
                                if edit.submitted {
                                    action = Some(if cell.kind() == "markdown" {
                                        Action::Preview(at)
                                    } else {
                                        Action::Run(false)
                                    });
                                }
                                if let Some(error) = edit.error { view.error = error.into(); }
                                changed |= edit.changed;
                            }
                            if cell.kind() == "code" {
                                for (output_index, output) in cell.outputs().iter().take(notebook::MAX_OUTPUTS).enumerate() {
                                    ui.add_space(6.);
                                    if output["data"].get("image/png").is_some() || output["data"].get("image/jpeg").is_some() {
                                        let key = (uid, cell.outputs_version, output_index);
                                        if !view.pictures.contains_key(&key) && view.pictures.len() < 16 {
                                            let Some((data, image_path)) = image_bundle(&output["data"]) else {
                                                ui.label("Image output exceeds its preview limit or has invalid data; the original is preserved.");
                                                continue;
                                            };
                                            view.pictures.insert(key, Picture::Pending);
                                            jobs.push(PictureJob {
                                                target: Target::Output { uid, version: cell.outputs_version, index: output_index },
                                                encoded: Some((data, image_path)),
                                                path: String::new(),
                                                cancel: Arc::new(AtomicBool::new(false))
                                            });
                                        }
                                        match view.pictures.get(&key) {
                                            Some(Picture::Ready(texture, _lease)) => {
                                                ui.add(egui::Image::new(texture).fit_to_original_size(1.).max_width(ui.available_width()).max_height(600.));
                                            },
                                            Some(Picture::Error(error)) => { ui.colored_label(palette.error, error); },
                                            Some(Picture::Pending) => { ui.label("Loading image output…"); },
                                            None => { ui.label("Only 16 output images are previewed at once; all data is preserved."); }
                                        }
                                    } else {
                                        let text = output_text(output);
                                        let color = if output["output_type"] == "error" || output["name"] == "stderr" { palette.error } else { palette.text };
                                        ui.add(egui::Label::new(RichText::new(text).monospace().color(color)).wrap().selectable(true));
                                    }
                                }
                                if cell.outputs().len() > notebook::MAX_OUTPUTS {
                                    ui.small("Only the first 256 saved outputs are displayed; the rest are preserved.");
                                }
                            }
                        });
                    });
                    ui.push_id(("insert-cell", id, uid), |ui| {
                        egui::Frame::new().inner_margin(egui::Margin { left: 18, right: 18, top: 4, bottom: 8 })
                            .show(ui, |ui| {
                                cell_toolbar(ui, |ui| {
                                    if ui.button("+ Code cell").on_hover_text("Insert a code cell here").clicked()
                                    { action = Some(Action::Add("code", at + 1)); }
                                    if ui.button("+ Markdown cell").on_hover_text("Insert a text cell here").clicked()
                                    { action = Some(Action::Add("markdown", at + 1)); }
                                });
                            });
                    });
                    state.height = (ui.cursor().top() - top).max(60.);
                    y += state.height;
                }
            });
            for (uid, state) in &mut view.cells {
                if !visible.contains(uid) {
                    state.preview = None;
                }
            }
            view.pictures.retain(|(uid, version, _), _| {
                visible.contains(uid)
                    && view
                        .book
                        .cells
                        .iter()
                        .any(|c| c.uid == *uid && c.outputs_version == *version)
            });
            if let Some(selected_action) = action.take() {
                match selected_action {
                    Action::Add(kind, at) => {
                        if view.book.cells.len() < notebook::MAX_CELLS {
                            let cell = Cell::new(kind);
                            view.active = cell.uid;
                            view.focus = Some(cell.uid);
                            view.cells.entry(cell.uid).or_default().previewing = false;
                            view.book.cells.insert(at, cell);
                            view.reveal = true;
                            changed = true;
                        } else {
                            view.error = "Notebook is limited to 1024 cells.".into();
                        }
                    }
                    Action::Delete(at) => {
                        let uid = view.book.cells.remove(at).uid;
                        view.cells.remove(&uid);
                        view.invalidate_pictures(uid);
                        view.active = view
                            .book
                            .cells
                            .get(at)
                            .or_else(|| view.book.cells.last())
                            .map(|c| c.uid)
                            .unwrap_or(0);
                        changed = true;
                    }
                    Action::Move(at, delta) => {
                        let to = at.saturating_add_signed(delta);
                        view.book.cells.swap(at, to);
                        changed = true;
                    }
                    Action::Kind(at, kind) => {
                        view.book.cells[at].change_kind(&kind);
                        changed = true;
                    }
                    Action::Clear(at) => {
                        let cell = at.and_then(|at| view.book.cells.get(at)).map(|cell| {
                            cell.data["id"]
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| format!("uid:{}", cell.uid))
                        });
                        if let Some(kernel) = &mut view.kernel
                            && let Err(error) = kernel.clear_outputs(cell)
                        {
                            view.error = error.to_string();
                        }
                        for (index, cell) in view.book.cells.iter_mut().enumerate() {
                            if at.is_none() || at == Some(index) {
                                cell.clear_outputs();
                            }
                        }
                        view.pictures.clear();
                        view.display_ids.clear();
                        changed = true;
                    }
                    Action::Kernel => match view.book.set_kernel(&view.kernel_name) {
                        Ok(()) => {
                            view.stop("Kernel changed · Run to start");
                            changed = true;
                        }
                        Err(e) => view.error = e.to_string(),
                    },
                    Action::Interrupt => {
                        if let Some(kernel) = &mut view.kernel
                            && let Err(e) = kernel.interrupt()
                        {
                            view.error = e.to_string();
                        }
                    }
                    Action::Shutdown => view.stop("Kernel shut down · Run to restart"),
                    Action::Preview(at) => {
                        view.cells
                            .entry(view.book.cells[at].uid)
                            .or_default()
                            .previewing = true;
                    }
                    other => action = Some(other),
                }
            }
            if changed {
                view.sync(&mut self.documents[index]);
            }
        }
        let source = view.source_mode;
        self.notebook_picture_jobs(&view, jobs, ctx);
        self.documents[index].notebook = Some(view);
        match action {
            Some(Action::Run(all)) => self.run_notebook(all, ctx),
            Some(Action::Source) => self.toggle_notebook_source(ctx),
            _ => {}
        }
        if let Some(link) = link {
            if link.starts_with("https://")
                || link.starts_with("http://")
                || link.starts_with("mailto:")
            {
                ctx.open_url(egui::OpenUrl::new_tab(link));
            } else {
                self.notify("Use the explorer to open local notebook links.", false);
            }
        }
        !source
    }
}

fn cell_toolbar(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.scope(|ui| {
        // Set the row height before layout so selectors, labels and buttons share a centre line.
        ui.spacing_mut().interact_size.y = 28.;
        ui.spacing_mut().button_padding = egui::vec2(8., 4.);
        ui.spacing_mut().item_spacing = egui::vec2(6., 6.);
        ui.horizontal_wrapped(add_contents);
    });
}

fn image_bundle(bundle: &Value) -> Option<(String, String)> {
    for (mime, path) in [("image/png", "output.png"), ("image/jpeg", "output.jpg")] {
        if let Ok(data) = notebook::multiline(&bundle[mime])
            && data.len() <= 2 * 1024 * 1024
        {
            return Some((data, path.into()));
        }
    }
    None
}
fn output_text(output: &Value) -> String {
    let text = match output["output_type"].as_str().unwrap_or("") {
        "stream" => notebook::multiline(&output["text"]).unwrap_or_default(),
        "error" => output["traceback"]
            .as_array()
            .map(|lines| lines.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| format!(
                "{}: {}",
                output["ename"].as_str().unwrap_or("Error"),
                output["evalue"].as_str().unwrap_or("")
            )),
        _ => notebook::multiline(&output["data"]["text/plain"])
            .or_else(|_| notebook::multiline(&output["data"]["text/markdown"]))
            .unwrap_or_else(|_| {
                "[Rich output has no supported text/PNG/JPEG representation; its data is preserved.]".into()
            }),
    };
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else if !c.is_control() || "\n\t\r".contains(c) {
            result.push(c);
        }
        if result.len() >= 64 * 1024 {
            result.push_str(
                "\n[Output display limited to 64 KiB; full output remains in the notebook.]",
            );
            break;
        }
    }
    result
}
