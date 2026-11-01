//! Virtualized hex editing and static binary inspection from bounded snapshots.
//!
//! Immutable snapshots and a shared budget cover worker analysis and save copies. Cache
//! dirty/disassembly state between edits, render visible rows only, and reject results for
//! obsolete buffer versions or decoder parameters.
use super::*;
use crate::binary::{self, Buffer, Report};

#[derive(Clone, Copy, PartialEq)]
/// Selected static inspection mode; switching views never executes the inspected binary.
enum View {
    Hex,
    Overview,
    Sections,
    Symbols,
    Strings,
    Imports,
    Exports,
    Disassembly,
}
#[derive(Clone, Copy, PartialEq, Eq)]
/// Snapshot version and decoder parameters required to accept an asynchronous analysis result.
struct AnalysisKey {
    version: u64,
    offset: usize,
    bits: u32,
}
type AnalysisResult = (AnalysisKey, Arc<Report>, Vec<binary::Instruction>);
/// Editable byte buffer and cached static report, sharing memory accounting with
/// analysis/save snapshots; rendered row caches are invalidated by version/filter changes.
pub(super) struct HexView {
    pub buffer: Buffer,
    pub report: Arc<Report>,
    _lease: Arc<crate::images::Lease>,
    mode: View,
    offset: usize,
    offset_text: String,
    patch: String,
    search: String,
    search_hex: bool,
    filter: String,
    error: String,
    editing: bool,
    little: bool,
    bits: u32,
    version: u64,
    analyzed: u64,
    instructions: Vec<binary::Instruction>,
    worker: Option<mpsc::Receiver<AnalysisResult>>,
    reveal: bool,
    nibble: Option<u8>,
    rows: Vec<(String, Option<usize>)>,
    row_key: Option<(u8, String, u64)>,
    analysis_serial: u64,
}
impl HexView {
    /// Initialize byte editing and inspection from a bounded loaded snapshot without executing it.
    pub fn new(loaded: binary::Loaded) -> Self {
        let bits = if loaded.report.bits == 32 { 32 } else { 64 };
        Self {
            buffer: Buffer::new(loaded.bytes),
            report: loaded.report,
            _lease: loaded.lease,
            mode: View::Hex,
            offset: 0,
            offset_text: "0".into(),
            patch: String::new(),
            search: String::new(),
            search_hex: true,
            filter: String::new(),
            error: String::new(),
            editing: false,
            little: true,
            bits,
            version: 0,
            analyzed: 0,
            instructions: vec![],
            worker: None,
            reveal: false,
            nibble: None,
            rows: vec![],
            row_key: None,
            analysis_serial: 0,
        }
    }
    fn select(&mut self, offset: usize) {
        self.offset = offset.min(self.buffer.bytes.len().saturating_sub(1));
        self.offset_text = format!("{:X}", self.offset);
        self.patch = self
            .buffer
            .bytes
            .get(self.offset)
            .map(|b| format!("{b:02X}"))
            .unwrap_or_default();
        self.reveal = true;
        self.nibble = None;
    }
    /// Start at most one off-thread report/disassembly job using immutable captured bytes.
    fn analyze(&mut self, ctx: &egui::Context) {
        if self.worker.is_some() {
            return;
        }
        let bytes = self.buffer.bytes.clone();
        let lease = self._lease.clone();
        let offset = self.offset;
        let bits = self.bits;
        let version = self.version;
        let key = AnalysisKey {
            version,
            offset,
            bits,
        };
        // Selecting another offset/decoder mode does not change the object
        // metadata, strings, entropy or digest of the buffer.
        let report = (self.analyzed == version).then(|| self.report.clone());
        let (tx, rx) = mpsc::channel();
        self.worker = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _lease = lease;
            let report = report.unwrap_or_else(|| Arc::new(binary::inspect(&bytes)));
            let address = report
                .sections
                .iter()
                .find_map(|s| {
                    let delta = offset.checked_sub(s.offset?)?;
                    (delta < s.file_size as usize)
                        .then(|| s.address.checked_add(delta as u64))
                        .flatten()
                })
                .unwrap_or(offset as u64);
            let instructions =
                binary::disassemble(&bytes, offset, address, bits).unwrap_or_default();
            let _ = tx.send((key, report, instructions));
            ctx.request_repaint();
        });
    }
    /// Accept the object report only for matching bytes. Keep decoded instructions
    /// only if their offset and decoder mode also match the current selection.
    fn complete_analysis(&mut self, (key, report, instructions): AnalysisResult) {
        if key.version != self.version {
            return;
        }
        self.report = report;
        self.analyzed = key.version;
        self.instructions = if key.offset == self.offset && key.bits == self.bits {
            instructions
        } else {
            // The user moved or changed decoder mode while this worker ran.
            // Its object report is still valid, but its instructions are not.
            vec![]
        };
        self.analysis_serial = self.analysis_serial.wrapping_add(1);
    }
}
impl EditorApp {
    /// Queue a policy-scoped binary read or select an already open matching hex tab.
    pub(super) fn open_hex(&mut self, path: String, ctx: &egui::Context) {
        self.editor_intent();
        if !self.feature(crate::extensions::Feature::Binary) {
            self.notify("Enable Hex & binary analysis in View → Extensions.", false);
            return;
        }
        if let Some(index) = self
            .documents
            .iter()
            .position(|d| d.path.as_deref() == Some(&path))
        {
            self.active = index;
            if self.documents[index].hex.is_some() {
                self.focus_editor(ctx);
                return;
            }
            if self.documents[index].dirty() {
                self.notify(
                    "Save or close this file's text edits before reopening as hex.",
                    true,
                );
                return;
            }
            let doc = &self.documents[index];
            self.dispatch(
                Request::ReadHex { path },
                Job::Reload {
                    id: doc.id,
                    version: doc.edit_version,
                },
                ctx,
            );
        } else if self.opening.insert(path.clone()) {
            self.dispatch(
                Request::ReadHex { path: path.clone() },
                Job::Open(path),
                ctx,
            );
        }
    }
    /// Submit immutable edited bytes through protected binary saving with the captured revision.
    pub(super) fn save_hex(&mut self, id: u64, path: String, save_as: bool, ctx: &egui::Context) {
        let Some(doc) = self.documents.iter_mut().find(|d| d.id == id && !d.saving) else {
            return;
        };
        let Some(view) = &doc.hex else {
            return;
        };
        let bytes = view.buffer.bytes.clone();
        let lease = view._lease.clone();
        let expected = if save_as && doc.path.as_deref() != Some(&path) {
            None
        } else {
            doc.revision.clone()
        };
        doc.saving = true;
        self.dispatch(
            Request::WriteBinary {
                path,
                data: String::new(),
                expected,
            },
            Job::SaveBinary {
                id,
                bytes,
                _lease: lease,
            },
            ctx,
        );
    }
    /// Render the active hex/inspection view and apply explicit edit/save/reload actions.
    pub(super) fn hex_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        if self
            .documents
            .get(self.active)
            .is_none_or(|d| d.hex.is_none())
        {
            return false;
        }
        egui::Frame::new()
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| self.hex_content(ui, ctx))
            .inner
    }
    fn hex_content(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        if self
            .documents
            .get(self.active)
            .is_none_or(|d| d.hex.is_none())
        {
            return false;
        }
        let focus = self.take_editor_focus(ui, ctx).is_some();
        let palette = self.themes.active().palette;
        let keyboard = self.dialog == Dialog::None
            && self.pending.is_none()
            && !self.workbench_modal()
            && !egui::Popup::is_any_open(ctx);
        let doc = &mut self.documents[self.active];
        let id = doc.id;
        let mut focus_grid = focus;
        let view = doc.hex.as_mut().unwrap();
        let initial_version = view.version;
        if let Some(rx) = &view.worker {
            match rx.try_recv() {
                Ok(result) => {
                    view.complete_analysis(result);
                    view.worker = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    view.worker = None;
                    view.error = "Binary analysis worker stopped.".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(palette.tag(format!("{} BYTES", view.buffer.bytes.len())));
            for (mode, name) in [
                (View::Hex, "Hex"),
                (View::Overview, "Overview"),
                (View::Sections, "Sections"),
                (View::Symbols, "Symbols"),
                (View::Strings, "Strings"),
                (View::Imports, "Imports"),
                (View::Exports, "Exports"),
                (View::Disassembly, "Disassembly"),
            ] {
                if ui.selectable_label(view.mode == mode, name).clicked() {
                    view.mode = mode;
                }
            }
        });
        let mut reload = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Offset (hex)");
            let jump = ui.add(
                egui::TextEdit::singleline(&mut view.offset_text)
                    .desired_width(90.)
                    .char_limit(16),
            );
            if ui.small_button("Go").clicked()
                || (jump.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
            {
                match usize::from_str_radix(view.offset_text.trim().trim_start_matches("0x"), 16) {
                    Ok(offset) if offset < view.buffer.bytes.len() => {
                        view.select(offset);
                        view.mode = View::Hex;
                        view.error.clear();
                    }
                    _ => view.error = "Offset is outside the file.".into(),
                }
            }
            if ui
                .add_enabled(
                    view.worker.is_none(),
                    egui::Button::new("Analyze / decode here"),
                )
                .clicked()
            {
                view.analyze(ctx);
            }
            if ui
                .add_enabled(!doc.modified && !doc.saving, egui::Button::new("Reload"))
                .clicked()
            {
                reload = true;
            }
            if view.version != view.analyzed {
                ui.colored_label(palette.accent, "Analysis needs refresh");
            }
            if view.worker.is_some() {
                ui.spinner();
            }
        });
        if view.mode == View::Hex {
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut view.editing, "Edit bytes");
                ui.add_enabled_ui(view.editing, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut view.patch)
                            .desired_width(150.)
                            .hint_text("90 90 C3")
                            .char_limit(12288),
                    );
                    if ui.button("Overwrite").clicked() {
                        match binary::parse_hex(&view.patch)
                            .and_then(|bytes| view.buffer.patch(view.offset, bytes))
                        {
                            Ok(()) => {
                                view.version += 1;
                                view.error.clear();
                            }
                            Err(error) => view.error = format!("{error:#}"),
                        }
                    }
                    if ui.button("Undo").clicked() {
                        view.buffer.undo();
                        view.version += 1;
                    }
                    if ui.button("Redo").clicked() {
                        view.buffer.redo();
                        view.version += 1;
                    }
                });
                if ui.button("Copy byte").clicked()
                    && let Some(byte) = view.buffer.bytes.get(view.offset)
                {
                    ctx.copy_text(format!("{byte:02X}"));
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut view.search_hex, "Hex search");
                ui.add(
                    egui::TextEdit::singleline(&mut view.search)
                        .desired_width(180.)
                        .hint_text("Bytes or UTF-8 text")
                        .char_limit(4096),
                );
                if ui.button("Find next").clicked() {
                    let needle = if view.search_hex {
                        binary::parse_hex(&view.search)
                    } else {
                        Ok(view.search.as_bytes().to_vec())
                    };
                    match needle {
                        Ok(needle) => {
                            if let Some(offset) =
                                view.buffer.find(&needle, view.offset.saturating_add(1))
                            {
                                view.select(offset);
                                view.error.clear();
                            } else {
                                view.error = "No matching bytes.".into();
                            }
                        }
                        Err(error) => view.error = format!("{error:#}"),
                    }
                }
                ui.checkbox(&mut view.little, "Little endian");
                let remaining = &view.buffer.bytes[view.offset.min(view.buffer.bytes.len())..];
                if let Some(&byte) = remaining.first() {
                    ui.monospace(format!("u8 {byte} / i8 {}", byte as i8));
                }
                if remaining.len() >= 4 {
                    let b: [u8; 4] = remaining[..4].try_into().unwrap();
                    ui.monospace(format!(
                        "u32 {}",
                        if view.little {
                            u32::from_le_bytes(b)
                        } else {
                            u32::from_be_bytes(b)
                        }
                    ));
                }
            });
        } else if view.mode == View::Disassembly {
            ui.horizontal_wrapped(|ui| {
                for bits in [16, 32, 64] {
                    ui.selectable_value(&mut view.bits, bits, format!("x86 {bits}"));
                }
                if ui.button("Entry point").clicked()
                    && let Some(offset) = view.report.entry_offset
                {
                    view.select(offset);
                    view.analyze(ctx);
                }
                ui.small("Intel syntax · at most 512 instructions · choose mode and decode again");
            });
            if !["I386", "X86_64", "X86_64_X32", ""].contains(&view.report.architecture.as_str()) {
                ui.colored_label(palette.accent,"This object is not x86. Hex/tables work; x86 decoding here is an explicit raw interpretation.");
            }
        } else if view.mode != View::Overview {
            ui.add(
                egui::TextEdit::singleline(&mut view.filter)
                    .hint_text("Filter table…")
                    .char_limit(256),
            );
        }
        if !view.error.is_empty() {
            ui.colored_label(palette.error, &view.error);
        }
        let columns = if ui.available_width() < 650. { 8 } else { 16 };
        let font = FontId::monospace(13.);
        if view.mode == View::Hex {
            let cols = columns;
            ui.monospace(if cols == 16 {
                "OFFSET     00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F   ASCII"
            } else {
                "OFFSET     00 01 02 03 04 05 06 07   ASCII"
            });
            ui.spacing_mut().item_spacing.y = 0.;
            let row_height = 22.;
            let mut scroll = egui::ScrollArea::both()
                .id_salt(("hex-scroll", id))
                .auto_shrink([false, false]);
            if view.reveal {
                scroll = scroll.vertical_scroll_offset((view.offset / cols) as f32 * row_height);
                view.reveal = false;
            }
            scroll.show_rows(
                ui,
                row_height,
                view.buffer.bytes.len().div_ceil(cols),
                |ui, range| {
                    for row in range {
                        let start = row * cols;
                        let end = (start + cols).min(view.buffer.bytes.len());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.;
                            ui.add_sized(
                                [77., 20.],
                                egui::Label::new(
                                    RichText::new(format!("{start:08X}"))
                                        .font(font.clone())
                                        .color(palette.muted),
                                ),
                            );
                            for offset in start..end {
                                let byte = view.buffer.bytes[offset];
                                let changed = view.buffer.saved.get(offset) != Some(&byte);
                                if ui
                                    .add_sized(
                                        [23., 20.],
                                        egui::Button::new(
                                            RichText::new(format!("{byte:02X}"))
                                                .font(font.clone())
                                                .color(if changed {
                                                    palette.accent
                                                } else {
                                                    palette.text
                                                }),
                                        )
                                        .selected(offset == view.offset)
                                        .frame(offset == view.offset),
                                    )
                                    .clicked()
                                {
                                    view.select(offset);
                                    view.reveal = false;
                                    focus_grid = true;
                                }
                            }
                            let ascii: String = view.buffer.bytes[start..end]
                                .iter()
                                .map(|b| {
                                    if b.is_ascii_graphic() || *b == b' ' {
                                        *b as char
                                    } else {
                                        '.'
                                    }
                                })
                                .collect();
                            ui.monospace(ascii);
                        });
                    }
                },
            );
        } else if view.mode == View::Overview {
            egui::ScrollArea::vertical().show(ui,|ui| {
                let r=&view.report;
                ui.heading(format!("{} · {}",r.format,r.architecture));
                for line in [format!("Type: {} · {} bit · {} endian",r.kind,r.bits,if r.little_endian {"little"} else {"big"}),format!("Entry VA: 0x{:X} · file offset: {}",r.entry,r.entry_offset.map(|o| format!("0x{o:X}")).unwrap_or_else(||"unmapped".into())),format!("Entropy: {:.3} / 8 bits per byte",r.entropy),format!("SHA-256: {}",r.sha256)] { ui.add(egui::Label::new(RichText::new(line).monospace()).wrap()); }
                ui.label(&r.note);ui.label("Patching preserves file length. Headers, relocations, checksums and code signatures are not repaired automatically.");
            });
        } else {
            // Tables are bounded at load time; render only visible filtered rows.
            let query = view.filter.to_lowercase();
            let row_key = (view.mode as u8, query.clone(), view.analysis_serial);
            if view.row_key.as_ref() != Some(&row_key) {
                view.rows = match view.mode {
                    View::Sections => view
                        .report
                        .sections
                        .iter()
                        .map(|s| {
                            (
                                format!(
                                    "{:016X}  {:08X}  {}  {}",
                                    s.address, s.size, s.kind, s.name
                                ),
                                s.offset,
                            )
                        })
                        .collect(),
                    View::Symbols => view
                        .report
                        .symbols
                        .iter()
                        .map(|s| {
                            (
                                format!(
                                    "{:016X}  {:08X}  {}  {}",
                                    s.address, s.size, s.kind, s.name
                                ),
                                s.offset,
                            )
                        })
                        .collect(),
                    View::Strings => view
                        .report
                        .strings
                        .iter()
                        .map(|s| (format!("{:08X}  {}", s.offset, s.text), Some(s.offset)))
                        .collect(),
                    View::Imports => view
                        .report
                        .imports
                        .iter()
                        .map(|s| (s.clone(), None))
                        .collect(),
                    View::Exports => view
                        .report
                        .exports
                        .iter()
                        .map(|s| (s.clone(), None))
                        .collect(),
                    View::Disassembly => view
                        .instructions
                        .iter()
                        .map(|i| {
                            (
                                format!(
                                    "{:08X}  {:016X}  {:45} {}",
                                    i.offset, i.address, i.bytes, i.text
                                ),
                                Some(i.offset),
                            )
                        })
                        .collect(),
                    _ => vec![],
                }
                .into_iter()
                .filter(|(s, _)| s.to_lowercase().contains(&query))
                .collect();
                view.row_key = Some(row_key);
            }
            let rows = &view.rows;
            ui.small(format!(
                "{} rows · click a mapped row to inspect its bytes",
                rows.len()
            ));
            let mut selected_offset = None;
            egui::ScrollArea::both()
                .id_salt(("binary-table", id, view.mode as u8))
                .auto_shrink([false, false])
                .show_rows(ui, 22., rows.len(), |ui, range| {
                    for index in range {
                        let (line, offset) = &rows[index];
                        if ui
                            .add(
                                egui::Button::new(RichText::new(line).font(font.clone()))
                                    .frame(false),
                            )
                            .clicked()
                            && let Some(offset) = offset
                        {
                            selected_offset = Some(*offset);
                        }
                    }
                });
            if let Some(offset) = selected_offset {
                view.select(offset);
                view.mode = View::Hex;
            }
        }
        let focus_id = egui::Id::new(("hex", id));
        let response = ui.interact(
            egui::Rect::NOTHING,
            focus_id,
            Sense::focusable_noninteractive(),
        );
        if focus_grid {
            ctx.memory_mut(|m| m.request_focus(focus_id));
        }
        ctx.memory_mut(|m| {
            m.set_focus_lock_filter(
                focus_id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            )
        });
        if keyboard && response.has_focus() && view.mode == View::Hex {
            let mut target = view.offset;
            ctx.input_mut(|i| {
                for (key, delta) in [
                    (Key::ArrowLeft, -1),
                    (Key::ArrowRight, 1),
                    (Key::ArrowUp, -(columns as isize)),
                    (Key::ArrowDown, columns as isize),
                    (Key::PageUp, -256),
                    (Key::PageDown, 256),
                ] {
                    if i.consume_key(egui::Modifiers::NONE, key) {
                        target = target.saturating_add_signed(delta);
                    }
                }
                if i.consume_key(egui::Modifiers::NONE, Key::Home) {
                    target = 0;
                }
                if i.consume_key(egui::Modifiers::NONE, Key::End) {
                    target = view.buffer.bytes.len().saturating_sub(1);
                }
            });
            if target != view.offset {
                view.select(target);
            }
            if view.editing && ctx.input(|i| i.modifiers.is_none()) {
                let text: Vec<String> = ctx.input_mut(|i| {
                    let mut text = vec![];
                    i.events.retain(|event| {
                        if let egui::Event::Text(value) = event {
                            text.push(value.clone());
                            false
                        } else {
                            true
                        }
                    });
                    text
                });
                for digit in text
                    .iter()
                    .flat_map(|s| s.chars())
                    .filter_map(|c| c.to_digit(16))
                {
                    if let Some(high) = view.nibble.take() {
                        if view
                            .buffer
                            .patch(view.offset, vec![(high << 4) | digit as u8])
                            .is_ok()
                        {
                            view.version += 1;
                            view.select(view.offset.saturating_add(1));
                        }
                    } else {
                        view.nibble = Some(digit as u8);
                    }
                }
            }
        }
        if keyboard && response.has_focus() && view.editing {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, Key::Z)) {
                view.buffer.undo();
                view.version += 1;
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, Key::Y)) {
                view.buffer.redo();
                view.version += 1;
            }
        }
        if initial_version != view.version {
            doc.modified = view.buffer.dirty();
            doc.edit_version = doc.edit_version.wrapping_add(1);
        }
        if reload {
            self.open_hex_reload(id, ctx);
        }
        true
    }
    fn open_hex_reload(&mut self, id: u64, ctx: &egui::Context) {
        if let Some(doc) = self.documents.iter().find(|d| d.id == id && !d.dirty())
            && let Some(path) = &doc.path
        {
            self.dispatch(
                Request::ReadHex { path: path.clone() },
                Job::Reload {
                    id,
                    version: doc.edit_version,
                },
                ctx,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};

    #[test]
    fn disassembly_results_match_the_offset_mode_and_buffer_version() {
        let budget = Arc::new(crate::images::Budget::default());
        let loaded = binary::Loaded::decode(&STANDARD.encode([0x90, 0xc3]), &budget).unwrap();
        let mut view = HexView::new(loaded);
        let report = view.report.clone();
        let key = AnalysisKey {
            version: 0,
            offset: 0,
            bits: view.bits,
        };
        let instructions = binary::disassemble(&view.buffer.bytes, 0, 0, view.bits).unwrap();
        view.offset = 1;
        view.complete_analysis((key, report.clone(), instructions.clone()));
        assert!(view.instructions.is_empty());
        view.offset = 0;
        view.bits = 32;
        view.complete_analysis((key, report.clone(), instructions.clone()));
        assert!(view.instructions.is_empty());
        view.bits = key.bits;
        view.complete_analysis((key, report.clone(), instructions.clone()));
        assert!(!view.instructions.is_empty());
        view.version = 1;
        view.complete_analysis((key, report.clone(), vec![]));
        assert!(
            !view.instructions.is_empty(),
            "An obsolete result must not replace the accepted view"
        );
        view.version = 0;
        view.analyze(&egui::Context::default());
        let (_, reused, _) = view
            .worker
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(
            Arc::ptr_eq(&report, &reused),
            "Offset-only decoding must reuse the existing full-file report"
        );
    }

    /// Render-only sample: a large buffer differs at its last byte, while
    /// scrolling remains virtualized. Report timings instead of speed limits.
    #[test]
    #[ignore = "manual hex UI performance profile"]
    fn hex_frame_profile() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        let bytes = Arc::new(vec![0; binary::MAX_BYTES]);
        let loaded = binary::Loaded {
            report: Arc::new(binary::inspect(&bytes)),
            lease: Arc::new(
                app.image_budget
                    .reserve(bytes.len() * 4 + 5 * 1024 * 1024)
                    .unwrap(),
            ),
            bytes,
        };
        let mut view = HexView::new(loaded);
        view.buffer.patch(binary::MAX_BYTES - 1, vec![1]).unwrap();
        view.version = 1;
        app.documents[0].hex = Some(view);
        app.documents[0].modified = true;
        let mut sample = vec![];
        for i in 0..130 {
            let started = Instant::now();
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1380., 900.),
                    )),
                    ..Default::default()
                },
                |ctx| app.editor(ctx),
            );
            if i >= 30 {
                sample.push(started.elapsed().as_secs_f64() * 1000.);
            }
        }
        sample.sort_by(f64::total_cmp);
        println!(
            "Hex 8 MiB unchanged frame: median {:.3} ms, p95 {:.3} ms (100 warmed frames)",
            sample[50], sample[95]
        );
        assert!(app.documents[0].dirty());
    }
    #[test]
    fn closing_a_tab_keeps_inflight_save_bytes_charged_until_the_job_finishes() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].hex = Some(HexView::new(
            binary::Loaded::decode(&STANDARD.encode([0, 1, 2]), &app.image_budget).unwrap(),
        ));
        let view = app.documents[0].hex.as_ref().unwrap();
        let job = Job::SaveBinary {
            id: app.documents[0].id,
            bytes: view.buffer.bytes.clone(),
            _lease: view._lease.clone(),
        };
        app.documents.clear();
        assert!(app.image_budget.used() > 0);
        drop(job);
        assert_eq!(app.image_budget.used(), 0);
    }
    #[test]
    fn clicking_a_byte_retains_keyboard_focus_for_direct_hex_input_and_navigation() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        let loaded =
            binary::Loaded::decode(&STANDARD.encode([0, 1, 2, 3]), &app.image_budget).unwrap();
        let mut view = HexView::new(loaded);
        view.editing = true;
        app.documents[0].hex = Some(view);
        let frame = |app: &mut EditorApp, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000., 600.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.hex_ui(ui, ctx);
                    });
                },
            )
        };
        let output = frame(&mut app, vec![]);
        let point = output
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    if text.galley.text() == "00" {
                        Some(text.pos + egui::vec2(5., 5.))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .unwrap();
        let button = |pressed| egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(
            &mut app,
            vec![egui::Event::PointerMoved(point), button(true)],
        );
        frame(&mut app, vec![button(false)]);
        frame(&mut app, vec![egui::Event::Text("cc".into())]);
        assert_eq!(app.documents[0].hex.as_ref().unwrap().buffer.bytes[0], 0xcc);
        let arrow = egui::Event::Key {
            key: Key::ArrowRight,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        frame(&mut app, vec![arrow]);
        frame(&mut app, vec![egui::Event::Text("dd".into())]);
        assert_eq!(app.documents[0].hex.as_ref().unwrap().buffer.bytes[2], 0xdd);
    }
    #[test]
    fn binary_save_acknowledgements_and_disabling_preserve_newer_byte_edits() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        let id = app.documents[0].id;
        let loaded =
            binary::Loaded::decode(&STANDARD.encode([0, 1, 2, 255]), &app.image_budget).unwrap();
        app.documents[0].hex = Some(HexView::new(loaded));
        let hex = app.documents[0].hex.as_mut().unwrap();
        hex.buffer.patch(1, vec![9]).unwrap();
        let submitted = hex.buffer.bytes.clone();
        hex.buffer.patch(2, vec![8]).unwrap();
        app.documents[0].modified = true;
        app.tx
            .send(Event::Done {
                generation: app.generation,
                job: Job::SaveBinary {
                    id,
                    bytes: submitted,
                    _lease: app.documents[0].hex.as_ref().unwrap()._lease.clone(),
                },
                result: Box::new(Ok(files::Response {
                    path: "/example.bin".into(),
                    revision: "saved".into(),
                    ..Default::default()
                })),
            })
            .unwrap();
        app.poll(&ctx);
        assert!(app.documents[0].dirty());
        assert_eq!(
            app.documents[0]
                .hex
                .as_ref()
                .unwrap()
                .buffer
                .bytes
                .as_slice(),
            [0, 9, 8, 255]
        );
        app.set_extension(crate::extensions::Feature::Binary.id(), false);
        assert!(app.documents[0].dirty());
        assert!(app.documents[0].hex.is_some());
        app.format_active(true, false, &ctx);
        assert!(!app.documents[0].saving);
        assert_eq!(app.documents[0].text, "");
    }
}
