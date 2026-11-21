//! Styled source chunks with immutable identities. Unchanged token arrays keep
//! their chunk jobs, so a keystroke does not copy every TextFormat in a file.
use super::*;
pub(crate) const CHUNK_LINES: usize = 64;

/// Exact full text plus styled chunks separated by one omitted hard newline.
/// The native layout adapter restores that separator when joining galleys.
pub(crate) struct Layout {
    pub text: Arc<str>,
    pub chunks: Vec<Arc<LayoutJob>>,
    pub font: FontId,
    pub foreground: Color32,
}
struct Chunk {
    job: Arc<LayoutJob>,
    tokens: Vec<Option<Arc<[Token]>>>,
}
#[derive(Default)]
pub(super) struct Projector {
    key: Option<(u32, u64, bool)>,
    chunks: Vec<Chunk>,
}
impl Projector {
    pub fn project(
        &mut self,
        text: Arc<str>,
        language: &str,
        parsed: Option<&Parsed>,
        size: f32,
        settings: &Settings,
    ) -> Arc<Layout> {
        let font = FontId::monospace(size);
        let foreground = settings
            .engine
            .theme
            .settings
            .foreground
            .map(|c| Color32::from_rgb(c.r, c.g, c.b))
            .unwrap_or(Color32::WHITE);
        let plain = TextFormat::simple(font.clone(), foreground);
        let ranges = lines(&text);
        let parsed =
            parsed.filter(|p| p.generation == settings.generation && p.language == language);
        let (prefix, suffix) = parsed.map_or((0, 0), |p| common(p, &text, &ranges));
        let key = (size.to_bits(), settings.generation, settings.glow);
        let mut chunks = Vec::new();
        for begin in (0..ranges.len().max(1)).step_by(CHUNK_LINES) {
            let end = (begin + CHUNK_LINES).min(ranges.len());
            let start_byte = ranges.get(begin).map_or(0, |r| r.start);
            let end_byte = end
                .checked_sub(1)
                .and_then(|index| ranges.get(index))
                .map_or(0, |r| r.end)
                - usize::from(end < ranges.len());
            let tokens: Vec<_> = (begin..end)
                .map(|index| {
                    parsed.and_then(|p| {
                        let old_index = if index < prefix {
                            index
                        } else if index >= ranges.len() - suffix {
                            p.ranges.len() - (ranges.len() - index)
                        } else {
                            return None;
                        };
                        Some(p.tokens[old_index].clone())
                    })
                })
                .collect();
            let old = self.chunks.get(chunks.len()).filter(|old| {
                self.key == Some(key)
                    && old.job.text == text[start_byte..end_byte]
                    && old.tokens.len() == tokens.len()
                    && old.tokens.iter().zip(&tokens).all(|(a, b)| match (a, b) {
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                        (None, None) => true,
                        _ => false,
                    })
            });
            let job = if let Some(old) = old {
                old.job.clone()
            } else {
                let mut job = LayoutJob {
                    text: text[start_byte..end_byte].into(),
                    ..Default::default()
                };
                let mut append = |range: Range<usize>, format: TextFormat| {
                    if let Some(last) = job.sections.last_mut()
                        && last.format == format
                        && last.byte_range.end == range.start
                    {
                        last.byte_range.end = range.end;
                    } else {
                        job.sections.push(LayoutSection {
                            leading_space: 0.,
                            byte_range: range,
                            format,
                        });
                    }
                };
                for (index, tokens) in (begin..end).zip(&tokens) {
                    let range = &ranges[index];
                    if let Some(tokens) = tokens.as_ref().filter(|t| !t.is_empty()) {
                        for token in tokens.iter() {
                            let start =
                                (range.start + token.range.start).min(end_byte) - start_byte;
                            let end = (range.start + token.range.end).min(end_byte) - start_byte;
                            append(start..end, token_format(token.style, &font, settings.glow));
                        }
                    } else {
                        append(
                            range.start - start_byte..range.end.min(end_byte) - start_byte,
                            plain.clone(),
                        );
                    }
                }
                if job.sections.is_empty() {
                    job.sections.push(LayoutSection {
                        leading_space: 0.,
                        byte_range: 0..0,
                        format: plain.clone(),
                    });
                }
                Arc::new(job)
            };
            chunks.push(Chunk { job, tokens });
        }
        self.key = Some(key);
        let jobs = chunks.iter().map(|c| c.job.clone()).collect();
        self.chunks = chunks;
        Arc::new(Layout {
            text,
            chunks: jobs,
            font,
            foreground,
        })
    }
}
