//! Markdown styled as one text buffer, retaining selection across block boundaries.
use crate::theme;
use eframe::egui::{self, FontId, Stroke, TextFormat, text::LayoutJob};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

struct Layout {
    job: LayoutJob,
    format: TextFormat,
    styles: Vec<TextFormat>,
    lists: Vec<Option<u64>>,
    links: Vec<(String, usize)>,
    code: Option<String>,
    gap: usize,
    table_cell: bool,
}

impl Layout {
    fn append(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if !self.job.text.is_empty() && self.gap > 0 {
            let existing = self
                .job
                .text
                .chars()
                .rev()
                .take_while(|c| *c == '\n')
                .count();
            self.job.append(
                &"\n".repeat(self.gap.saturating_sub(existing)),
                0.,
                self.format.clone(),
            );
        }
        self.gap = 0;
        self.job.append(text, 0., self.format.clone());
    }

    fn separate(&mut self, lines: usize) {
        self.gap = self.gap.max(lines);
    }
}

/// Convert supported Markdown events into one themed selectable text layout,
/// including nested emphasis, lists, tables, links and inline/fenced code.
pub(super) fn layout(ui: &egui::Ui, text: &str, palette: theme::Palette) -> LayoutJob {
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
        table_cell: false,
    };
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(text, options) {
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
                    Tag::Table(_) => out.separate(2),
                    Tag::TableHead | Tag::TableRow => {
                        out.separate(1);
                        out.table_cell = false;
                    }
                    Tag::TableCell => {
                        if out.table_cell {
                            out.append(" | ");
                        }
                        out.table_cell = true;
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
                    TagEnd::Heading(_) | TagEnd::BlockQuote(_) | TagEnd::Table => out.separate(2),
                    TagEnd::List(_) => {
                        out.lists.pop();
                        out.separate(if out.lists.is_empty() { 2 } else { 1 });
                    }
                    TagEnd::Item | TagEnd::TableHead | TagEnd::TableRow => out.separate(1),
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
                if let Some(code) = &mut out.code {
                    code.push_str(&text);
                } else {
                    out.append(&text);
                }
            }
            Event::Code(code) => {
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
    out.job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> LayoutJob {
        let ctx = egui::Context::default();
        let mut job = LayoutJob::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| job = layout(ui, text, theme::Palette::default()));
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
