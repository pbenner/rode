//! Native presentation for the small HTML style subset used by notebook titles.
//! Decorations refer to converted Markdown, after which the ordinary asset
//! preparation still owns every image read. No CSS resources are evaluated.
use super::super::{MAX_BYTES, MdEvent, Parser, Tag, TagEnd, options};
use crate::theme;
use eframe::egui::{self, Align, Color32, FontId, Stroke, TextFormat, text::LayoutJob};
use std::ops::Range;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub(in crate::app::markdown) struct Decoration {
    pub range: Range<usize>,
    pub kind: Kind,
}
#[derive(Clone, Copy, Debug)]
pub(in crate::app::markdown) enum Kind {
    Heading { level: u8, align: Align },
    Rule { width: f32, color: Option<Color32> },
}

/// A valid inline text-align overrides the legacy align attribute. Unsupported
/// declarations are ignored rather than treated as a general CSS stylesheet.
pub(super) fn alignment(attribute: &str, style: &str) -> Option<Align> {
    let parse = |value: &str| match value.trim().to_ascii_lowercase().as_str() {
        "left" => Some(Align::Min),
        "center" => Some(Align::Center),
        "right" => Some(Align::Max),
        _ => None,
    };
    let mut align = parse(attribute);
    for (key, value) in declarations(style) {
        if key.eq_ignore_ascii_case("text-align")
            && let Some(value) = parse(value)
        {
            align = Some(value);
        }
    }
    align
}

fn declarations(style: &str) -> impl Iterator<Item = (&str, &str)> {
    style
        .split(';')
        .filter_map(|part| part.split_once(':'))
        .map(|(key, value)| (key.trim(), value.trim()))
}

/// Accept a solid border shorthand in CSS pixels. Bounded widths and literal
/// colors suffice for notebook separators without introducing CSS layout/URLs.
pub(super) fn rule(style: &str) -> Option<Kind> {
    let mut rule = None;
    for (key, value) in declarations(style) {
        if !key.eq_ignore_ascii_case("border") && !key.eq_ignore_ascii_case("border-top") {
            continue;
        }
        let mut width = None;
        let mut color = None;
        let mut solid = false;
        let mut valid = true;
        for token in value.split_ascii_whitespace() {
            let token = token.to_ascii_lowercase();
            if token == "solid" {
                solid = true;
            } else if let Some(value) = token.strip_suffix("px").and_then(|v| v.parse::<f32>().ok())
            {
                if value.is_finite() && (0. ..=8.).contains(&value) {
                    width = Some(value);
                } else {
                    valid = false;
                }
            } else if let Some(value) = literal_color(&token) {
                color = Some(value);
            } else {
                valid = false;
            }
        }
        if valid && solid {
            rule = Some(Kind::Rule {
                width: width.unwrap_or(1.),
                color,
            });
        }
    }
    rule
}

fn literal_color(value: &str) -> Option<Color32> {
    Some(match value {
        "gray" | "grey" => Color32::from_gray(128),
        "silver" => Color32::from_gray(192),
        "darkgray" | "darkgrey" => Color32::from_gray(169),
        "lightgray" | "lightgrey" => Color32::from_gray(211),
        "black" => Color32::BLACK,
        "white" => Color32::WHITE,
        "red" => Color32::RED,
        "green" => Color32::from_rgb(0, 128, 0),
        "blue" => Color32::BLUE,
        value if value.starts_with('#') => Color32::from_hex(value).ok()?,
        _ => return None,
    })
}

#[derive(Default)]
pub(in crate::app::markdown) struct Presentation {
    blocks: Vec<Block>,
}
struct Block {
    kind: Kind,
    runs: Vec<Run>,
}
struct Run {
    text: String,
    style: Inline,
}
#[derive(Clone, Default)]
struct Inline {
    italic: bool,
    strike: bool,
    code: bool,
    link: Option<String>,
}
// A known block tag is required: custom elements can parse as InlineHtml, whose
// renderer intentionally stays literal. The adapter strips original div tags.
const MARKER: &str = "<div data-rode-notebook=\"";
const END: &str = "\"></div>";

impl Presentation {
    /// Replace decorated blocks only after asset preparation. Everything else
    /// stays in one Markdown document, including reference definitions and hooks.
    /// Image-bearing headings retain the shared Markdown image renderer.
    pub fn prepare(source: &mut String, mut decorations: Vec<Decoration>) -> Result<Self, String> {
        if decorations.is_empty() {
            return Ok(Self::default());
        }
        decorations.sort_by_key(|decoration| decoration.range.start);
        let mut events = Parser::new_ext(source, options())
            .into_offset_iter()
            .peekable();
        let mut presentation = Self::default();
        let mut output = String::new();
        let mut cursor = 0;
        for decoration in decorations {
            let range = decoration.range;
            if range.start < cursor || source.get(range.clone()).is_none() {
                continue;
            }
            while events
                .peek()
                .is_some_and(|(_, span)| span.start < range.start)
            {
                events.next();
            }
            let block_events = std::iter::from_fn(|| {
                events.peek().filter(|(_, span)| span.start < range.end)?;
                events.next().map(|(event, _)| event)
            });
            let runs = match decoration.kind {
                Kind::Heading { .. } => match inline(block_events) {
                    Some(runs) => runs,
                    None => continue,
                },
                Kind::Rule { .. } => vec![],
            };
            output.push_str(&source[cursor..range.start]);
            output.push_str(&format!(
                "\n\n{MARKER}{}{END}\n\n",
                presentation.blocks.len()
            ));
            if output.len() > MAX_BYTES {
                return Err("Styled notebook Markdown exceeds its 256 KiB preview limit.".into());
            }
            presentation.blocks.push(Block {
                kind: decoration.kind,
                runs,
            });
            cursor = range.end;
        }
        output.push_str(&source[cursor..]);
        if output.len() > MAX_BYTES {
            return Err("Styled notebook Markdown exceeds its 256 KiB preview limit.".into());
        }
        *source = output;
        Ok(presentation)
    }

    /// Only adapter-generated markers can select native blocks. Original HTML
    /// has already been converted/escaped, and code fences remain literal.
    pub fn ui(
        &self,
        ui: &mut egui::Ui,
        marker: &str,
        size: f32,
        palette: theme::Palette,
    ) -> Option<String> {
        let index = marker
            .trim()
            .strip_prefix(MARKER)?
            .strip_suffix(END)?
            .parse::<usize>()
            .ok()?;
        let block = self.blocks.get(index)?;
        let width = ui.available_width().max(1.);
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.),
            egui::Layout::top_down(Align::Min),
            |ui| {
                ui.set_min_width(width);
                ui.add_space(3.);
                let clicked = match block.kind {
                    Kind::Heading { level, align } => {
                        heading(ui, &block.runs, level, align, size, palette)
                    }
                    Kind::Rule {
                        width: thickness,
                        color,
                    } => {
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(width, thickness.max(1.) + 6.),
                            egui::Sense::hover(),
                        );
                        if thickness > 0. {
                            ui.painter().hline(
                                rect.x_range(),
                                rect.center().y,
                                Stroke::new(thickness, color.unwrap_or(palette.border)),
                            );
                        }
                        None
                    }
                };
                ui.add_space(3.);
                clicked
            },
        )
        .inner
    }
}

fn inline<'a>(events: impl Iterator<Item = MdEvent<'a>>) -> Option<Vec<Run>> {
    let mut runs = vec![];
    let mut style = Inline::default();
    let mut stack = vec![];
    for event in events {
        let text = match event {
            MdEvent::Text(text) => text.into_string(),
            MdEvent::Code(text) => {
                let mut code = style.clone();
                code.code = true;
                runs.push(Run {
                    text: text.into_string(),
                    style: code,
                });
                continue;
            }
            MdEvent::SoftBreak => " ".into(),
            MdEvent::HardBreak => "\n".into(),
            MdEvent::Start(Tag::Heading { .. }) | MdEvent::End(TagEnd::Heading(_)) => continue,
            MdEvent::Start(tag) => {
                stack.push(style.clone());
                match tag {
                    Tag::Emphasis => style.italic = true,
                    Tag::Strikethrough => style.strike = true,
                    Tag::Strong => {} // Headings already use the strong foreground.
                    Tag::Link { dest_url, .. } => style.link = Some(dest_url.into_string()),
                    _ => return None,
                }
                continue;
            }
            MdEvent::End(_) => {
                style = stack.pop().unwrap_or_default();
                continue;
            }
            _ => return None,
        };
        runs.push(Run {
            text,
            style: style.clone(),
        });
    }
    Some(runs)
}

fn heading(
    ui: &mut egui::Ui,
    runs: &[Run],
    level: u8,
    align: Align,
    size: f32,
    palette: theme::Palette,
) -> Option<String> {
    // Match the existing preview's restrained six-level hierarchy.
    let size = size * (1. + (7 - level.clamp(1, 6)) as f32 / 12.);
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    job.halign = align;
    let mut links = vec![];
    let mut characters = 0;
    for run in runs {
        let color = if run.style.link.is_some() {
            palette.accent
        } else {
            ui.visuals().strong_text_color()
        };
        job.append(
            &run.text,
            0.,
            TextFormat {
                font_id: if run.style.code {
                    FontId::monospace(size)
                } else {
                    FontId::proportional(size)
                },
                color,
                italics: run.style.italic,
                underline: if run.style.link.is_some() {
                    Stroke::new(1_f32, color)
                } else {
                    Stroke::NONE
                },
                strikethrough: if run.style.strike {
                    Stroke::new(1_f32, color)
                } else {
                    Stroke::NONE
                },
                ..Default::default()
            },
        );
        let end = characters + run.text.chars().count();
        if let Some(link) = &run.style.link {
            links.push((characters..end, link));
        }
        characters = end;
    }
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.with_layout(
        egui::Layout::top_down(align).with_cross_justify(true),
        |ui| {
            let response = ui.add(egui::Label::new(galley.clone()).selectable(true));
            let origin = match align {
                Align::Min => response.rect.left_top(),
                Align::Center => response.rect.center_top(),
                Align::Max => response.rect.right_top(),
            };
            let mut clicked = None;
            for (index, (range, link)) in links.iter().enumerate() {
                let mut first = 0;
                for (row_index, row) in galley.rows.iter().enumerate() {
                    let start = range.start.max(first);
                    let end = range.end.min(first + row.char_count_excluding_newline());
                    if start < end {
                        let a = galley.pos_from_cursor(egui::text::CCursor {
                            index: start,
                            prefer_next_row: true,
                        });
                        let b = galley.pos_from_cursor(egui::text::CCursor {
                            index: end,
                            prefer_next_row: false,
                        });
                        let rect = a.union(b).translate(origin.to_vec2());
                        let response = ui
                            .interact(
                                rect,
                                response.id.with((index, row_index)),
                                egui::Sense::click(),
                            )
                            .on_hover_text(*link)
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Link,
                                ui.is_enabled(),
                                *link,
                            )
                        });
                        theme::button_focus(ui, &response, None);
                        crate::keyboard::target(ui, &format!("Open {link}"), &response);
                        if response.clicked() {
                            clicked = Some((*link).clone());
                        }
                    }
                    first += row.char_count_including_newline();
                }
            }
            clicked
        },
    )
    .inner
}
