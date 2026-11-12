//! Markdown styled as one text buffer, retaining selection across block boundaries.
use crate::theme;
use eframe::egui::{self, FontId, Stroke, TextFormat, text::LayoutJob};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::{ops::Range, sync::Arc};

mod formulas;
mod math;
mod tables;

/// One selectable buffer with native formula artwork at stable character positions.
pub(super) struct MessageLayout {
    pub job: LayoutJob,
    formulas: Vec<PlacedFormula>,
    tables: Vec<tables::Table>,
}

struct PlacedFormula {
    character: usize,
    source: String,
    size: egui::Vec2,
    artwork: Artwork,
}

enum Artwork {
    Formula(egui::TextureHandle),
    Source(Arc<egui::Galley>),
}

impl MessageLayout {
    pub fn has_tables(&self) -> bool {
        !self.tables.is_empty()
    }

    /// Wrap prose to the viewport while keeping table columns together in one selectable buffer.
    pub fn galley(&self, ui: &egui::Ui, width: f32, prose_offset: f32) -> Arc<egui::Galley> {
        tables::galley(ui, &self.job, &self.tables, width, prose_offset)
    }

    /// Expand formula placeholders back to their original TeX for clipboard use.
    pub fn copy(&self, range: Range<usize>) -> String {
        let mut out = String::new();
        let mut formulas = self.formulas.iter().peekable();
        for (i, ch) in self.job.text.chars().enumerate().take(range.end) {
            if formulas
                .peek()
                .is_some_and(|formula| formula.character == i)
            {
                let formula = formulas.next().unwrap();
                if i >= range.start {
                    out.push_str(&formula.source);
                }
            } else if i >= range.start {
                out.push(ch);
            }
        }
        out
    }

    /// Paint inside the space reserved by the text widget, retaining its selection hit regions.
    pub fn paint(
        &self,
        ui: &egui::Ui,
        output: &egui::text_edit::TextEditOutput,
        palette: theme::Palette,
    ) {
        for table in &self.tables {
            table.paint(ui, output, palette);
        }
        for formula in &self.formulas {
            let cursor = output.galley.pos_from_cursor(egui::text::CCursor {
                index: formula.character,
                prefer_next_row: true,
            });
            let rect = egui::Rect::from_min_size(
                output.galley_pos
                    + egui::vec2(cursor.left(), cursor.center().y - formula.size.y / 2.),
                formula.size,
            );
            if !ui.is_rect_visible(rect) {
                continue;
            }
            match &formula.artwork {
                Artwork::Formula(texture) => {
                    ui.painter().image(
                        texture.id(),
                        rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                        palette.text,
                    );
                }
                Artwork::Source(galley) => {
                    ui.painter().galley(rect.min, galley.clone(), palette.text);
                }
            }
        }
    }
}

struct Layout {
    job: LayoutJob,
    format: TextFormat,
    styles: Vec<TextFormat>,
    lists: Vec<Option<u64>>,
    links: Vec<(String, usize)>,
    code: Option<String>,
    gap: usize,
    table: Option<usize>,
    table_cell_start: usize,
    tables: Vec<tables::Table>,
    characters: usize,
    formulas: Vec<PlacedFormula>,
}

impl Layout {
    fn append(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.flush_gap();
        self.job.append(text, 0., self.format.clone());
        self.characters += text.chars().count();
    }

    fn flush_gap(&mut self) {
        if !self.job.text.is_empty() && self.gap > 0 {
            let existing = self
                .job
                .text
                .chars()
                .rev()
                .take_while(|c| *c == '\n')
                .count();
            let gap = self.gap.saturating_sub(existing);
            self.characters += gap;
            self.job.append(&"\n".repeat(gap), 0., self.format.clone());
        }
        self.gap = 0;
    }

    fn separate(&mut self, lines: usize) {
        self.gap = self.gap.max(lines);
    }

    fn formula(&mut self, ui: &egui::Ui, formula: &formulas::Formula) {
        let width = ui
            .available_width()
            .min(ui.clip_rect().right() - ui.next_widget_position().x)
            .max(1.);
        let size = self.format.font_id.size.max(14.);
        let (artwork, dimensions) = if let Some((texture, dimensions)) =
            math::texture(ui.ctx(), &formula.source, formula.display, size)
        {
            let scale = (width / dimensions.x.max(1.)).min(1.);
            (Artwork::Formula(texture), dimensions * scale)
        } else {
            let galley = ui.fonts_mut(|fonts| {
                fonts.layout(
                    formula.source.clone(),
                    FontId::monospace(12.),
                    self.format.color,
                    width,
                )
            });
            let dimensions = galley.size();
            (Artwork::Source(galley), dimensions)
        };
        let block = formula.display && self.table.is_none();
        if block {
            self.separate(2);
        }
        self.flush_gap();
        let previous = self.format.clone();
        // A nonbreaking space has no rasterised glyph. Its advance reserves the
        // formula width while explicit line height keeps tall fractions/matrices clear.
        let space = ui.fonts_mut(|fonts| fonts.glyph_width(&FontId::monospace(100.), '\u{a0}'));
        self.format = TextFormat {
            font_id: FontId::monospace(dimensions.x.max(1.) * 100. / space.max(1.)),
            line_height: Some(dimensions.y.max(size) + 2.),
            color: egui::Color32::TRANSPARENT,
            ..Default::default()
        };
        self.append("\u{a0}");
        self.formulas.push(PlacedFormula {
            character: self.characters - 1,
            source: formula.source.clone(),
            size: dimensions,
            artwork,
        });
        self.format = previous;
        if block {
            self.separate(2);
        }
    }
}

/// Convert supported Markdown events into one themed selectable text layout,
/// including nested emphasis, lists, tables, links and inline/fenced code.
pub(super) fn layout(ui: &egui::Ui, text: &str, palette: theme::Palette) -> MessageLayout {
    let body = egui::TextStyle::Body.resolve(ui.style());
    let mut out = Layout {
        job: LayoutJob::default(),
        format: TextFormat {
            font_id: body.clone(),
            color: palette.text,
            ..Default::default()
        },
        styles: vec![],
        lists: vec![],
        links: vec![],
        code: None,
        gap: 0,
        table: None,
        table_cell_start: 0,
        tables: vec![],
        characters: 0,
        formulas: vec![],
    };
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    let prepared = formulas::prepare(text, options);
    for (event, range) in Parser::new_ext(&prepared.text, options).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                out.styles.push(out.format.clone());
                match tag {
                    Tag::Strong => {
                        out.format.color = palette.accent;
                        out.format.font_id.size += 1.;
                    }
                    Tag::Emphasis => out.format.italics = true,
                    Tag::Strikethrough => {
                        out.format.strikethrough = Stroke::new(1_f32, out.format.color)
                    }
                    Tag::Heading { level, .. } => {
                        out.separate(2);
                        out.format.font_id.size = body.size + 9. - level as u8 as f32;
                        out.format.color = palette.accent;
                    }
                    Tag::CodeBlock(_) => {
                        out.separate(2);
                        out.format.font_id = FontId::monospace(12.);
                        out.format.background = palette.raised;
                        out.code = Some(String::new());
                    }
                    Tag::List(start) => {
                        out.separate(1);
                        out.lists.push(start);
                    }
                    Tag::Item => {
                        out.separate(1);
                        let marker = match out.lists.last_mut() {
                            Some(Some(number)) => {
                                let marker = format!("{number}. ");
                                *number = number.saturating_add(1);
                                marker
                            }
                            _ => "- ".into(),
                        };
                        out.append(&format!(
                            "{}{marker}",
                            "  ".repeat(out.lists.len().saturating_sub(1))
                        ));
                    }
                    Tag::BlockQuote(_) => {
                        out.separate(2);
                        out.format.color = palette.muted;
                        out.format.italics = true;
                    }
                    Tag::Link { dest_url, .. } => {
                        out.links.push((dest_url.into_string(), out.job.text.len()));
                        out.format.color = palette.accent;
                    }
                    Tag::Table(alignments) => {
                        out.separate(2);
                        out.table = Some(out.tables.len());
                        out.tables.push(tables::Table::new(alignments));
                    }
                    Tag::TableHead | Tag::TableRow => {
                        let table = out.table.unwrap();
                        if out.tables[table].rows.is_empty() {
                            out.flush_gap();
                        } else {
                            out.append("\n");
                        }
                        out.tables[table].row(out.job.text.len(), out.characters);
                        if matches!(tag, Tag::TableHead) {
                            out.format.color = palette.accent;
                            out.format.font_id.size += 1.;
                        }
                    }
                    Tag::TableCell => {
                        if !out.tables[out.table.unwrap()]
                            .rows
                            .last()
                            .unwrap()
                            .cells
                            .is_empty()
                        {
                            out.append(" | ");
                        }
                        out.table_cell_start = out.job.text.len();
                    }
                    _ => {}
                }
            }
            Event::End(tag) => {
                match tag {
                    TagEnd::CodeBlock => {
                        if let Some(code) = out.code.take() {
                            out.append(code.trim_end_matches(['\n', '\r']));
                        }
                        out.separate(2);
                    }
                    TagEnd::Paragraph => out.separate(if out.lists.is_empty() { 2 } else { 1 }),
                    TagEnd::Heading(_) | TagEnd::BlockQuote(_) => out.separate(2),
                    TagEnd::TableCell => {
                        out.tables[out.table.unwrap()]
                            .cell(out.table_cell_start..out.job.text.len());
                    }
                    TagEnd::Table => {
                        out.tables[out.table.take().unwrap()].range.end = out.job.text.len();
                        out.separate(2);
                    }
                    TagEnd::List(_) => {
                        out.lists.pop();
                        out.separate(if out.lists.is_empty() { 2 } else { 1 });
                    }
                    TagEnd::Item => out.separate(1),
                    TagEnd::Link => {
                        if let Some((url, start)) = out.links.pop()
                            && out.job.text[start..].trim() != url
                        {
                            out.append(&format!(" ({url})"));
                        }
                    }
                    _ => {}
                }
                if let Some(format) = out.styles.pop() {
                    out.format = format;
                }
            }
            Event::Text(text) => {
                let mut text = text.as_ref();
                if prepared.guards.contains(&range.start) {
                    text = text.strip_prefix('\u{200b}').unwrap_or(text);
                }
                if prepared
                    .guards
                    .contains(&range.end.saturating_sub('\u{200b}'.len_utf8()))
                {
                    text = text.strip_suffix('\u{200b}').unwrap_or(text);
                }
                if let Some(code) = &mut out.code {
                    code.push_str(text);
                } else {
                    out.append(text);
                }
            }
            Event::Code(code) => {
                if let Some(formula) = prepared
                    .offsets
                    .get(&range.start)
                    .and_then(|index| prepared.formulas.get(*index))
                {
                    out.formula(ui, formula);
                    continue;
                }
                let previous = out.format.clone();
                out.format.font_id = FontId::monospace(12.);
                out.format.background = palette.raised;
                out.append(&code);
                out.format = previous;
            }
            Event::SoftBreak | Event::HardBreak => out.append("\n"),
            Event::Rule => {
                out.separate(2);
                out.append("───");
                out.separate(2);
            }
            Event::TaskListMarker(checked) => out.append(if checked { "[x] " } else { "[ ] " }),
            Event::Html(html) | Event::InlineHtml(html) => out.append(&html),
            _ => {}
        }
    }
    for table in &mut out.tables {
        table.prepare(ui, &mut out.job);
    }
    MessageLayout {
        job: out.job,
        formulas: out.formulas,
        tables: out.tables,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_formulas_and_code_keep_all_contents_without_synthetic_spacing() {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for (source, expected) in [
                    (r"\(x\)\(y\)`code`$z$", r"\(x\)\(y\)code$z$"),
                    (r"**\(x\)** and `code`\(y\).", r"\(x\) and code\(y\)."),
                    ("`\u{e000}`\\(x\\)\u{200b}", "\u{e000}\\(x\\)\u{200b}"),
                    (r"\(\text{cost $5 and $6}\)", r"\(\text{cost $5 and $6}\)"),
                    (r"\[\unsupported{x}\]", r"\[\unsupported{x}\]"),
                ] {
                    let layout = layout(ui, source, theme::Palette::default());
                    assert_eq!(layout.copy(0..usize::MAX), expected);
                    assert!(!layout.formulas.is_empty());
                }
            });
        });
    }

    #[test]
    fn equations_reserve_native_text_space_wrap_and_copy_across_blocks() {
        let ctx = egui::Context::default();
        let source = "**Equivariance** \\(v\\mapsto Rv\\) and $x_i^2$.\n\n\\[W Rv=R Wv \\qquad\\text{for every }R,v.\\]\n\n$$\\frac{1}{\\sqrt{x}}$$\n\n```rust\nlet x = 1;\n```";
        let expected = "Equivariance \\(v\\mapsto Rv\\) and $x_i^2$.\n\n\\[W Rv=R Wv \\qquad\\text{for every }R,v.\\]\n\n$$\\frac{1}{\\sqrt{x}}$$\n\nlet x = 1;";
        let mut last = None;
        let frame = |width, events, last: &mut Option<egui::text_edit::TextEditOutput>| {
            let mut ready = false;
            let full = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 800.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let layout = layout(ui, source, theme::Palette::default());
                        assert_eq!(layout.formulas.len(), 4);
                        assert_eq!(layout.copy(0..usize::MAX), expected);
                        ready = layout
                            .formulas
                            .iter()
                            .all(|formula| matches!(formula.artwork, Artwork::Formula(_)));
                        let output = super::super::render_message(
                            ui,
                            source,
                            theme::Palette::default(),
                            false,
                        );
                        // Rendering may finish between these two layouts. Compare
                        // geometry only once the first layout already has every texture.
                        if ready {
                            for formula in &layout.formulas {
                                let at = output.galley.layout_from_cursor(egui::text::CCursor {
                                    index: formula.character,
                                    prefer_next_row: true,
                                });
                                let row = &output.galley.rows[at.row];
                                let glyph = &row.glyphs[at.column];
                                assert!(
                                    (glyph.advance_width - formula.size.x).abs() <= 2.,
                                    "formula advance {} != {}",
                                    glyph.advance_width,
                                    formula.size.x
                                );
                                assert!(row.height() + 1. >= formula.size.y);
                                assert!(
                                    row.rect().right() <= width,
                                    "equation escaped the viewport"
                                );
                            }
                        }
                        *last = Some(output);
                    });
                },
            );
            (full, ready)
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !frame(600., vec![], &mut last).1 {
            assert!(
                std::time::Instant::now() < deadline,
                "formulas did not render"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Resizing reuses the rendered texture, fitting equations to the new viewport.
        for width in (120..600).step_by(17) {
            assert!(frame(width as f32, vec![], &mut last).1);
        }
        assert!(frame(240., vec![], &mut last).1);
        let mut output = last.take().unwrap();
        output
            .state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(output.galley.text().chars().count()),
            )));
        output.state.store(&ctx, output.response.id);
        output.response.request_focus();
        let (full, _) = frame(240., vec![egui::Event::Copy], &mut last);
        assert!(full.platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == expected)
        ));
    }

    fn parsed(text: &str) -> LayoutJob {
        let ctx = egui::Context::default();
        let mut job = LayoutJob::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                job = layout(ui, text, theme::Palette::default()).job
            });
        });
        job
    }

    #[test]
    fn markdown_inline_styles_remove_markers_and_restore_nested_styles() {
        let job = parsed(
            "**Environment** has *italic*, **strong and *nested***, ~~removed~~ and `**literal**`.",
        );
        assert_eq!(
            job.text,
            "Environment has italic, strong and nested, removed and **literal**."
        );
        let format = |needle: &str| {
            &job.sections
                .iter()
                .find(|section| &job.text[section.byte_range.clone()] == needle)
                .unwrap()
                .format
        };
        assert_eq!(
            format("Environment").color,
            theme::Palette::default().accent
        );
        assert!(format("italic").italics);
        assert!(format("nested").italics);
        assert!(!format(" and ").italics);
        assert!(format("removed").strikethrough.width > 0.);
        assert_eq!(
            format("**literal**").font_id.family,
            egui::FontFamily::Monospace
        );
    }

    #[test]
    fn markdown_blocks_keep_readable_lists_code_and_link_destinations() {
        let job = parsed(
            "# Heading\n\n- **First**\n- Second\n  1. Nested\n\n```python\n    print('**literal**')\n```\n\n[Docs](https://example.invalid)",
        );
        assert_eq!(
            job.text,
            "Heading\n\n- First\n- Second\n  1. Nested\n\n    print('**literal**')\n\nDocs (https://example.invalid)"
        );
        assert!(job.sections[0].format.font_id.size > 14.);
        assert_eq!(parsed("~~~text\n**literal**\n~~~").text, "**literal**");
        assert_eq!(
            parsed("```text\nunfinished **code").text,
            "unfinished **code"
        );
        assert_eq!(
            parsed("| A | B |\n|---|---|\n| one | two |").text,
            "A | B\none | two"
        );
    }
}
