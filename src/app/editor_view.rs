//! Active document rendering and in-buffer find/replace controls.
//!
//! Specialized previews delegate to their existing view modules; source edits
//! retain the document's native undo/cursor state and advance its version only
//! after an accepted edit. Find matches use UTF-8 byte ranges, translated to
//! egui character selections at this UI boundary. Host I/O, save acknowledgements
//! and guarded project/window actions remain with the application coordinator.
use super::*;

pub(super) mod gutter;

impl EditorApp {
    /// Keep the empty state independent of tab-strip cursor placement and pane width.
    fn empty_editor(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        let top = ((ui.available_height() - 230.) / 2.).max(20.);
        egui::ScrollArea::vertical()
            .id_salt("empty-editor")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(top);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("Start editing")
                            .size(24.)
                            .strong()
                            .color(palette.heading),
                    );
                    ui.add_space(8.);
                    ui.label(palette.tag("Choose a file in Explorer to start editing."));
                    ui.add_space(16.);
                    ui.label(palette.tag(format!(
                        "{} · Open file    {} · New file",
                        self.command_hint(Command::Open),
                        self.command_hint(Command::New)
                    )));
                });
            });
    }

    /// Keep the landing page's document actions in one aligned group.
    pub(super) fn empty_editor_actions(&mut self, ui: &mut egui::Ui) {
        let width = (ui.available_width() - 32.).clamp(0., 320.);
        let ctx = ui.ctx().clone();
        let new = ui.add_sized(
            egui::vec2(width, 32.),
            IconButton::new(Icon::NewFile, "New file").shortcut("Ctrl+N"),
        );
        ui_probe::button(ui, "editor-empty-new", &new);
        if new.clicked() {
            self.execute_command(Command::New, &ctx);
        }
        let open = ui.add_sized(
            egui::vec2(width, 32.),
            IconButton::new(Icon::Search, "Open file…").shortcut("Ctrl+O"),
        );
        ui_probe::button(ui, "editor-empty-open", &open);
        if open.clicked() {
            self.set_dialog(Dialog::Open);
        }
    }

    /// Render the active specialized or source document, retaining native edit/cursor/undo
    /// state and updating buffer versions only for accepted source changes.
    pub(super) fn editor(&mut self, ctx: &egui::Context) {
        if !self.pane_visible(navigation::Pane::Editor, ctx.viewport_id()) {
            return;
        }
        if self.vaults.locking() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.spinner();
                ui.label("Locking encrypted project…");
            });
            return;
        }
        let palette = self.themes.active().palette;
        let mut breakpoint_toggle = None;
        let mut change_clicked = None;
        let mut peek_action = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.background).inner_margin(if palette.chrome.is_decorated() { 8 } else { 0 }))
            .show(ctx, |ui| {
                palette.paint_panel(ui);
                if self.foreign_modal(ctx) || self.dialog != Dialog::None || self.pending.modal() { ui.disable(); }
                self.pane_ui(ui,navigation::Pane::Editor);
                if ui.available_height() <= 1.0 {
                    return;
                }
                if self.documents.active().is_none() {
                    if ctx.viewport_id() == egui::ViewportId::ROOT {
                        self.welcome(ui);
                    } else {
                        self.empty_editor(ui);
                    }
                    return;
                }
                self.tab_strip(ui, ctx);
                if self.documents.active().is_none() { return; }
                let path = self.documents[self.documents.active_index()]
                    .display_path()
                    .unwrap_or("Untitled");
                let relative = self
                    .workspace
                    .as_ref()
                    .and_then(|w| path.strip_prefix(&w.root))
                    .and_then(|p| p.strip_prefix('/'))
                    .unwrap_or(path);
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(18, 7))
                    .show(ui, |ui| {
                        let response = chrome::bounded_label(ui, palette.tag(relative), ui.available_width(), 18.)
                            .on_hover_text(path);
                        if !self.documents[self.documents.active_index()].preview_loading() {
                            ui_probe::value(ui, "active-document", &response, path);
                        }
                    });
                if self.documents[self.documents.active_index()].is_recovery() {
                    let doc = self.documents.active().unwrap();
                    let message = if doc.is_preview() && doc.notebook.is_none() {
                        "Disconnected SSH preview · Reconnect to reload its source."
                    } else {
                        "Disconnected SSH buffer · Save As a local copy, or reconnect to restore file access."
                    };
                    ui.colored_label(palette.muted, message);
                }
                if self.disabled_document_ui(ui) {
                    return;
                }
                if self.hex_ui(ui, ctx) {
                    return;
                }
                if self.pdf_ui(ui, ctx) {
                    return;
                }
                if self.documents[self.documents.active_index()].image.is_some() {
                    let focus = self.take_editor_focus(ui, ctx).is_some();
                    let doc = self.documents.active_mut().unwrap();
                    let id = doc.id;
                    let reload = doc
                        .image
                        .as_mut()
                        .unwrap()
                        .ui(ui, ctx, id, palette, focus)
                        .reload;
                    if reload {
                        self.reload_image(id, ctx);
                    }
                    return;
                }
                if self.notebook_ui(ui, ctx) {
                    return;
                }
                if self.markdown_ui(ui, ctx) {
                    return;
                }
                self.latex_toolbar(ui, ctx);
                if self.editor.show_find {
                    self.find_bar(ui);
                }
                let peek_height = self.editor.git_gutter.peek_height(self.documents.active().unwrap(), ui.available_height());
                let source_height = if peek_height > 0. { (ui.available_height() - peek_height).max(40.) } else { f32::INFINITY };
                let queued_focus = self.take_editor_focus(ui, ctx);
                let shortcut_style=self.navigation.bindings.style();
                if shortcut_style!=navigation::Style::Standard {ui.label(palette.tag(self.editor.input.label(shortcut_style)));}
                let source_stale=self.debug_source_stale(self.documents.active().unwrap());
                if source_stale {ui.colored_label(palette.accent,"Source changed since debugging started; execution locations may be stale. Restart debugging to use these changes.");}
                let debug_line=self.debug_line().filter(|(p,_)|self.documents[self.documents.active_index()].path.as_deref()==Some(*p)).filter(|_|!source_stale).map(|(_,line)|line);
                let debug_breakpoints=self.documents[self.documents.active_index()].path.as_ref().and_then(|p|self.workbench.debugger_settings().breakpoints.get(p)).map_or(&[][..],Vec::as_slice);
                let debug_enabled=self.feature(crate::extensions::Feature::Debugger);
                let debug_verified=self.documents[self.documents.active_index()].path.as_ref().and_then(|p|self.debugger.client.as_ref().and_then(|c|c.breakpoints.get(p))).map_or(&[][..],Vec::as_slice);
                let themes = &self.themes;
                let diagnostics = self.document_issues(self.documents.active().unwrap());
                let git_gutter = &self.editor.git_gutter;
                let input=&mut self.editor.input;
                let doc = self.documents.active_mut().unwrap();
                input.bind_source(doc.id,shortcut_style);
                let id = egui::Id::new(("document", doc.id));
                let jump_requested = doc.selection.is_some();
                let jump_cursor = doc.selection.map(|(_, end)| end);
                if let Some((start, end)) = doc.selection.take() {
                    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(start),
                            egui::text::CCursor::new(end),
                        )));
                    state.store(ctx, id);
                }
                let font_size = self.font_size;
                ui.style_mut()
                    .text_styles
                    .insert(egui::TextStyle::Monospace, FontId::monospace(font_size));
                let lang = syntax_token(
                    doc.display_path()
                        .or_else(|| doc.marp.as_ref().map(|_| "untitled.marp.md")),
                )
                .to_owned();
                let highlight = doc.text.len() < 512 * 1024;
                let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _wrap: f32| {
                    let mut job = if highlight {
                        themes.highlight(ui, buffer.as_str(), &lang, font_size)
                    } else {
                        egui::text::LayoutJob::simple(
                            buffer.as_str().into(),
                            FontId::monospace(font_size),
                            palette.text,
                            f32::INFINITY,
                        )
                    };
                    job.wrap.max_width = f32::INFINITY;
                    ui.fonts_mut(|fonts| fonts.layout_job(job))
                };
                egui::ScrollArea::both()
                    .id_salt(("editor-scroll", doc.id))
                    .auto_shrink([false, false])
                    .max_height(source_height)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.horizontal_top(|ui| {
                            let digits=doc.line_count().max(1).ilog10()+1;
                            let digit_width=ui.fonts_mut(|f|f.glyph_width(&FontId::monospace(font_size),'0'));
                            let gutter_width=46.0+(digits as f32*digit_width).max(24.0);
                            let (gutter, _) = ui.allocate_exact_size(
                                egui::vec2(gutter_width, 0.0),
                                Sense::hover(),
                            );
                            if queued_focus == Some(doc.id)
                                || (jump_requested
                                    && ui.memory(|m| m.allows_interaction(ui.layer_id())))
                            {
                                ui.memory_mut(|memory| memory.request_focus(id));
                                ctx.request_repaint();
                            }
                            let mut output = doc.edit_source(|source| {
                                egui::TextEdit::multiline(source)
                                .id(id)
                                .code_editor()
                                .cursor_at_end(false)
                                .font(FontId::monospace(font_size))
                                .desired_width(ui.available_width().max(80.0))
                                .desired_rows(2)
                                .frame(false)
                                .margin(egui::vec2(6.0, 0.0))
                                .layouter(&mut layouter)
                                .show(ui)
                            });
                            input.after_source(ui,doc.id,&mut output,&doc.text,palette);
                            ui_probe::button(ui, "source-editor", &output.response);
                            if output.cursor_range.is_some_and(|range| range.primary.index == 0 && range.secondary.index == 0) {
                                ui_probe::button(ui, "source-editor-at-start", &output.response);
                            }
                            if doc.text.len() <= 64 * 1024 {
                                ui_probe::value(ui, "source-editor-value", &output.response, &doc.text);
                            }
                            if !debug_breakpoints.is_empty() {
                                ui_probe::button(ui, "source-editor-breakpoints", &output.response);
                            }
                            if jump_requested
                                && let Some(cursor) = output
                                    .cursor_range
                                    .map(|range| range.primary.index)
                                    .or(jump_cursor)
                            {
                                let caret = output
                                    .galley
                                    .pos_from_cursor(egui::text::CCursor::new(cursor))
                                    .translate(output.galley_pos.to_vec2());
                                ui.scroll_to_rect(caret, Some(Align::Center));
                            }
                            let rows = gutter::Rows::new(&output.galley, output.galley_pos);
                            let gutter = egui::Rect::from_min_max(
                                egui::pos2(gutter.left(), rows.bounds().top()),
                                egui::pos2(gutter.right(), rows.bounds().bottom()),
                            );
                            let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(gutter));
                            let visible = rows.visible(ui.clip_rect());
                            for line in visible.clone() {
                                let row = rows.line(line).unwrap();
                                let y = row.center().y;
                                if debug_enabled && let Some(path)=&doc.path {
                                    let rect=egui::Rect::from_min_max(egui::pos2(gutter.left(),row.top()),egui::pos2(gutter.left()+16.,row.bottom()));
                                    let response=ui.interact(rect,id.with(("breakpoint",line)),Sense::click()).on_hover_text("Toggle breakpoint · F9");
                                    if response.clicked(){breakpoint_toggle=Some((path.clone(),line+1));}
                                    if let Some(bp)=debug_breakpoints.iter().find(|b|b.line==line+1){
                                        let index=debug_breakpoints.iter().filter(|b|b.enabled&&b.line<bp.line).count();
                                        let verified=debug_verified.get(index).is_some_and(|b|b.verified);
                                        let pos=egui::pos2(gutter.left()+7.,y);
                                        if bp.enabled && verified {painter.circle_filled(pos,4.,palette.error);} else {painter.circle_stroke(pos,4.,Stroke::new(1.5_f32,if bp.enabled{palette.error}else{palette.muted}));}
                                    }
                                }
                                if debug_line==Some(line+1){let x=gutter.left()+18.;painter.add(egui::Shape::convex_polygon(vec![egui::pos2(x-3.,y-5.),egui::pos2(x+3.,y),egui::pos2(x-3.,y+5.)],palette.accent,Stroke::NONE));}
                                painter.text(
                                    egui::pos2(
                                        gutter.right() - 13.0,
                                        y,
                                    ),
                                    egui::Align2::RIGHT_CENTER,
                                    (line + 1).to_string(),
                                    FontId::monospace(font_size),
                                    if line + 1 == doc.cursor.0 {
                                        palette.accent
                                    } else {
                                        palette.gutter
                                    },
                                );
                            }
                            painter.line_segment(
                                [
                                    egui::pos2(gutter.right() - 4.0, ui.clip_rect().top()),
                                    egui::pos2(gutter.right() - 4.0, ui.clip_rect().bottom()),
                                ],
                                Stroke::new(1.0_f32, palette.border),
                            );
                            if !output.response.changed() {
                                change_clicked = git_gutter.paint(ui, doc, gutter, rows, palette);
                                for (issue_index, issue) in diagnostics
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, d)| d.row > visible.start && d.row <= visible.end)
                                {
                                    let color = if issue.severity == "error" {
                                        palette.error
                                    } else {
                                        palette.accent
                                    };
                                    let row = rows.line(issue.row - 1).unwrap();
                                    let marker = egui::pos2(gutter.left() + 21.0, row.center().y);
                                    painter.circle_filled(marker, 3.0, color);
                                    ui.interact(
                                        egui::Rect::from_center_size(
                                            marker,
                                            egui::vec2(12., row.height()),
                                        ),
                                        id.with(("issue", issue_index)),
                                        Sense::hover(),
                                    )
                                    .on_hover_text(format!("{}: {}", issue.code, issue.message));
                                    let offset: usize = doc
                                        .text
                                        .split_inclusive('\n')
                                        .take(issue.row.saturating_sub(1))
                                        .map(|s| s.chars().count())
                                        .sum();
                                    let start = output
                                        .galley
                                        .pos_from_cursor(egui::text::CCursor::new(
                                            offset + issue.column.saturating_sub(1),
                                        ))
                                        .translate(output.galley_pos.to_vec2());
                                    let length = if issue.end_row == issue.row {
                                        issue.end_column.saturating_sub(issue.column).max(1)
                                    } else {
                                        1
                                    };
                                    let end = output
                                        .galley
                                        .pos_from_cursor(egui::text::CCursor::new(
                                            offset + issue.column.saturating_sub(1) + length,
                                        ))
                                        .translate(output.galley_pos.to_vec2());
                                    let underline = egui::Rect::from_min_max(
                                        egui::pos2(start.left(), start.top()),
                                        egui::pos2(
                                            end.right().max(start.left() + 5.),
                                            start.bottom(),
                                        ),
                                    );
                                    let points: Vec<_> = (0..=((underline.width() / 3.) as usize)
                                        .max(2))
                                        .map(|i| {
                                            egui::pos2(
                                                underline.left() + i as f32 * 3.,
                                                underline.bottom() - 1.
                                                    + if i % 2 == 0 { 0. } else { 2. },
                                            )
                                        })
                                        .collect();
                                    ui.painter().with_clip_rect(output.text_clip_rect).add(
                                        egui::Shape::line(points, Stroke::new(1.0_f32, color)),
                                    );
                                    ui.interact(
                                        underline,
                                        id.with(("issue-range", issue_index)),
                                        Sense::hover(),
                                    )
                                    .on_hover_text(format!("{}: {}", issue.code, issue.message));
                                }
                            }
                            if !doc.saving && !doc.dirty() {
                                ui_probe::button(ui, "source-editor-saved", &output.response);
                            }
                            if doc.dirty() {
                                ui_probe::button(ui, "source-editor-modified", &output.response);
                            }
                            if let Some(range) = output.cursor_range
                                && (range.primary.index != doc.last_cursor
                                    || output.response.changed())
                            {
                                doc.last_cursor = range.primary.index;
                                let mut line = 1;
                                let mut col = 1;
                                for c in doc.text.chars().take(range.primary.index) {
                                    if c == '\n' {
                                        line += 1;
                                        col = 1;
                                    } else {
                                        col += 1;
                                    }
                                }
                                doc.cursor = (line, col);
                            }
                            if output.cursor_range.is_some() {
                                ui_probe::button(
                                    ui,
                                    &format!("source-editor-cursor:{}:{}", doc.cursor.0, doc.cursor.1),
                                    &output.response,
                                );
                            }
                        });
                    });
                peek_action = self.editor.git_gutter.peek_ui(ui, self.documents.active().unwrap(), palette, peek_height);
            });
        if let Some(index) = change_clicked {
            if let Some(doc) = self.documents.active() {
                peek_action = self.editor.git_gutter.toggle_peek(doc, index);
            }
            ctx.request_repaint();
        }
        match peek_action {
            Some(git_gutter::PeekAction::Navigate(row)) => {
                if let Some(id) = self.documents.selected_id() {
                    self.jump_to_issue(id, row, 1, ctx);
                }
            }
            Some(git_gutter::PeekAction::Close) => self.focus_editor(ctx),
            Some(git_gutter::PeekAction::Revert(request)) => self.revert_git_change(request, ctx),
            None => {}
        }
        if let Some((path, line)) = breakpoint_toggle {
            self.toggle_breakpoint(path, line);
        }
    }

    /// Edit find/replace state and translate UTF-8 byte matches into native character selections.
    fn find_bar(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        let mut next = false;
        let mut replace = false;
        egui::Frame::new()
            .fill(palette.raised)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                let compact = ui.available_width() < 650.;
                ui.horizontal(|ui| {
                    let field_width = if compact {
                        (ui.available_width() - 168.).max(70.)
                    } else {
                        180.
                    };
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.editor.find)
                            .hint_text("Find in file")
                            .desired_width(field_width),
                    );
                    if self.editor.find_focus {
                        response.request_focus();
                        self.editor.find_focus = false;
                    }
                    ui_probe::button(ui, "source-find", &response);
                    ui_probe::value(ui, "source-find-value", &response, &self.editor.find);
                    next = ui.button("Next ↓").clicked()
                        || ((response.has_focus() || response.lost_focus())
                            && ui.input(|i| i.key_pressed(Key::Enter)));
                    ui.checkbox(&mut self.editor.case_sensitive, "Aa");
                    if !compact {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.editor.replacement)
                                .hint_text("Replace with")
                                .desired_width(140.),
                        );
                        replace = ui.button("Replace all").clicked();
                    }
                    if ui
                        .add_sized(
                            [26., 28.],
                            IconButton::icon(Icon::Close, "Close find").icon_size(14.),
                        )
                        .clicked()
                    {
                        self.editor.show_find = false;
                    }
                });
                if compact {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.editor.replacement)
                                .hint_text("Replace with")
                                .desired_width((ui.available_width() - 108.).max(70.)),
                        );
                        replace = ui.button("Replace all").clicked();
                    });
                }
            });
        if self.editor.find.is_empty() {
            return;
        }
        if replace && let Some(doc) = self.documents.active_mut() {
            let matches = find_matches(&doc.text, &self.editor.find, self.editor.case_sensitive);
            if !matches.is_empty() {
                doc.edit_source(|source| {
                    for (start, end) in matches.iter().rev() {
                        source.replace_range(*start..*end, &self.editor.replacement);
                    }
                });
            }
            self.status = format!("Replaced {} occurrences", matches.len());
        }
        if next {
            ui.ctx().input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::Enter);
            });
        }
        if next && let Some(doc) = self.documents.active_mut() {
            let matches = find_matches(&doc.text, &self.editor.find, self.editor.case_sensitive);
            let cursor = egui::TextEdit::load_state(ui.ctx(), egui::Id::new(("document", doc.id)))
                .and_then(|s| s.cursor.char_range())
                .map(|r| r.primary.index)
                .unwrap_or(0);
            let found = matches
                .iter()
                .find(|(start, _)| doc.text[..*start].chars().count() >= cursor)
                .or(matches.first());
            if let Some((start, end)) = found {
                doc.selection = Some((
                    doc.text[..*start].chars().count(),
                    doc.text[..*end].chars().count(),
                ));
            }
            self.status = format!("{} matches", matches.len());
        }
    }
}
