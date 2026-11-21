//! Incremental source syntax with a bounded, latest-request worker per view.
//!
//! Parsing never owns an editor buffer or egui state. Checkpoints retain full
//! multiline parser/highlighter state; an unchanged suffix is reused only when
//! both states converge. Pending layouts reuse unchanged lines and draw edits
//! in the default foreground until the matching result is ready.
use super::syntax::{Engine, token_format};
use eframe::egui::{
    self, Color32, FontId,
    text::{LayoutJob, LayoutSection, TextFormat},
};
use std::{
    ops::Range,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use syntect::{
    easy::HighlightLines,
    highlighting::{HighlightState, Style},
    parsing::ParseState,
};

mod layout;
pub(crate) use layout::Layout;
const CHECKPOINT_LINES: usize = 64;

#[derive(Clone)]
pub(crate) struct Settings {
    pub(super) engine: Engine,
    pub(super) generation: u64,
    pub(super) glow: bool,
}
#[derive(Clone, PartialEq)]
struct Token {
    range: Range<usize>,
    style: Style,
}
#[derive(Clone)]
struct Checkpoint {
    line: usize,
    highlight: HighlightState,
    parse: ParseState,
}
struct Parsed {
    source: Arc<str>,
    language: String,
    generation: u64,
    ranges: Vec<Range<usize>>,
    tokens: Vec<Arc<[Token]>>,
    checkpoints: Vec<Checkpoint>,
}

/// Include the line terminator, as required by the embedded newline grammars.
fn lines(text: &str) -> Vec<Range<usize>> {
    let mut start = 0;
    text.split_inclusive('\n')
        .map(|line| {
            let range = start..start + line.len();
            start = range.end;
            range
        })
        .collect()
}

/// Disjoint common regions; line content comparisons are byte-exact, including CRLF.
fn common(old: &Parsed, text: &str, ranges: &[Range<usize>]) -> (usize, usize) {
    let same = |a: usize, b: usize| old.source[old.ranges[a].clone()] == text[ranges[b].clone()];
    let mut prefix = 0;
    while prefix < old.ranges.len().min(ranges.len()) && same(prefix, prefix) {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old.ranges.len().min(ranges.len()) - prefix
        && same(old.ranges.len() - suffix - 1, ranges.len() - suffix - 1)
    {
        suffix += 1;
    }
    (prefix, suffix)
}

/// Transactional parse: cancellation discards this attempt and keeps the last
/// completed cache. A line insertion may shift checkpoint indices; convergence
/// uses the old suffix's checkpoint positions rather than a fixed modulo grid.
fn parse(
    source: Arc<str>,
    language: &str,
    settings: &Settings,
    previous: Option<&Parsed>,
    cancelled: impl Fn() -> bool,
) -> Option<Parsed> {
    let engine = &settings.engine;
    let theme = &engine.theme;
    let syntax = engine
        .ps
        .find_syntax_by_name(language)
        .or_else(|| engine.ps.find_syntax_by_extension(language))
        .unwrap_or_else(|| engine.ps.find_syntax_plain_text());
    let ranges = lines(&source);
    let previous =
        previous.filter(|old| old.generation == settings.generation && old.language == language);
    let (prefix, suffix) = previous.map_or((0, 0), |old| common(old, &source, &ranges));
    let restart = previous.and_then(|old| {
        old.checkpoints
            .iter()
            .rev()
            .find(|checkpoint| checkpoint.line <= prefix)
    });
    let start = restart.map_or(0, |checkpoint| checkpoint.line);
    let mut highlighter = restart.map_or_else(
        || HighlightLines::new(syntax, theme),
        |checkpoint| {
            HighlightLines::from_state(
                theme,
                checkpoint.highlight.clone(),
                checkpoint.parse.clone(),
            )
        },
    );
    let mut tokens: Vec<Arc<[Token]>> =
        previous.map_or_else(Vec::new, |old| old.tokens[..start].to_vec());
    let mut checkpoints: Vec<Checkpoint> = previous.map_or_else(Vec::new, |old| {
        old.checkpoints
            .iter()
            .filter(|c| c.line < start)
            .cloned()
            .collect()
    });
    let mut index = start;
    let old_suffix = previous.map_or(0, |old| old.ranges.len() - suffix);
    while index < ranges.len() {
        if cancelled() {
            return None;
        }
        let old_checkpoint = previous.and_then(|old| {
            if suffix == 0 || index < ranges.len() - suffix {
                return None;
            }
            let mapped = old_suffix + index - (ranges.len() - suffix);
            old.checkpoints
                .binary_search_by_key(&mapped, |c| c.line)
                .ok()
                .map(|i| (&old.checkpoints[i], mapped))
        });
        if index == start
            || (index - start).is_multiple_of(CHECKPOINT_LINES)
            || old_checkpoint.is_some()
        {
            let (highlight, state) = highlighter.state();
            if let Some((checkpoint, mapped)) = old_checkpoint
                && checkpoint.highlight == highlight
                && checkpoint.parse == state
                && let Some(old) = previous
            {
                tokens.extend_from_slice(&old.tokens[mapped..]);
                checkpoints.extend(
                    old.checkpoints
                        .iter()
                        .filter(|c| c.line >= mapped)
                        .cloned()
                        .map(|mut c| {
                            c.line = index + c.line - mapped;
                            c
                        }),
                );
                break;
            }
            checkpoints.push(Checkpoint {
                line: index,
                highlight: highlight.clone(),
                parse: state.clone(),
            });
            highlighter = HighlightLines::from_state(theme, highlight, state);
        }
        let line = &source[ranges[index].clone()];
        // Grammar errors produce plain tokens for this line, never an incomplete
        // or invalid byte range in the source widget.
        let mut offset = 0;
        let highlighted = highlighter.highlight_line(line, &engine.ps).ok();
        let spans: Vec<_> = highlighted
            .into_iter()
            .flatten()
            .map(|(style, text)| {
                let range = offset..offset + text.len();
                offset = range.end;
                Token { range, style }
            })
            .collect();
        let reused = previous.and_then(|old| {
            let old_index = if index < prefix {
                index
            } else if index >= ranges.len() - suffix {
                old.ranges.len() - (ranges.len() - index)
            } else {
                return None;
            };
            (old.tokens[old_index].as_ref() == spans.as_slice())
                .then(|| old.tokens[old_index].clone())
        });
        tokens.push(reused.unwrap_or_else(|| spans.into()));
        index += 1;
    }
    if cancelled() {
        return None;
    }
    Some(Parsed {
        source,
        language: language.into(),
        generation: settings.generation,
        ranges,
        tokens,
        checkpoints,
    })
}

struct Request {
    source: Arc<str>,
    language: String,
    settings: Settings,
    revision: u64,
    ctx: egui::Context,
}
#[derive(Default)]
struct Mailbox {
    request: Option<Request>,
    result: Option<(u64, Arc<Parsed>)>,
    stopped: bool,
}
#[derive(Default)]
struct Shared {
    mailbox: Mutex<Mailbox>,
    ready: Condvar,
    revision: AtomicU64,
}
struct Worker {
    shared: Arc<Shared>,
}
impl Worker {
    fn start() -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("source-syntax".into())
            .spawn(move || {
                let mut previous: Option<Arc<Parsed>> = None;
                loop {
                    let mut mailbox = worker.mailbox.lock().unwrap();
                    while mailbox.request.is_none() && !mailbox.stopped {
                        mailbox = worker.ready.wait(mailbox).unwrap();
                    }
                    if mailbox.stopped {
                        break;
                    }
                    let request = mailbox.request.take().unwrap();
                    drop(mailbox);
                    let parsed = parse(
                        request.source,
                        &request.language,
                        &request.settings,
                        previous.as_deref(),
                        || worker.revision.load(Ordering::Relaxed) != request.revision,
                    );
                    if let Some(parsed) = parsed {
                        let parsed = Arc::new(parsed);
                        previous = Some(parsed.clone());
                        let mut mailbox = worker.mailbox.lock().unwrap();
                        if !mailbox.stopped
                            && worker.revision.load(Ordering::Relaxed) == request.revision
                        {
                            mailbox.result = Some((request.revision, parsed));
                            drop(mailbox);
                            request.ctx.request_repaint();
                        }
                    }
                }
            })?;
        Ok(Self { shared })
    }
    fn submit(
        &self,
        source: Arc<str>,
        language: String,
        settings: Settings,
        ctx: &egui::Context,
    ) -> u64 {
        let revision = self.shared.revision.fetch_add(1, Ordering::Relaxed) + 1;
        let mut mailbox = self.shared.mailbox.lock().unwrap();
        mailbox.result = None;
        mailbox.request = Some(Request {
            source,
            language,
            settings,
            revision,
            ctx: ctx.clone(),
        });
        self.shared.ready.notify_one();
        revision
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let mut mailbox = self.shared.mailbox.lock().unwrap();
        mailbox.stopped = true;
        mailbox.request = None;
        mailbox.result = None;
        self.shared.revision.fetch_add(1, Ordering::Relaxed);
        self.shared.ready.notify_one();
    }
}

/// One lazy worker and one latest source/result per editor viewport. Closing a
/// view cancels its worker; no queued source history or per-keystroke threads.
#[derive(Default)]
pub(crate) struct Source {
    worker: Option<Worker>,
    synchronous: bool,
    source: Option<Arc<str>>,
    language: String,
    generation: u64,
    revision: u64,
    parsed: Option<Arc<Parsed>>,
    job: Option<(u32, bool, Arc<Layout>)>,
    projector: layout::Projector,
}
impl Source {
    /// Full exact text is always returned, including when syntax is pending.
    /// Only a matching generation/language/source may replace the parsed view.
    pub(crate) fn job(
        &mut self,
        ctx: &egui::Context,
        text: &str,
        language: &str,
        size: f32,
        settings: Settings,
    ) -> Arc<Layout> {
        if self.source.as_deref() != Some(text)
            || self.language != language
            || self.generation != settings.generation
        {
            let source: Arc<str> = text.into();
            if self.worker.is_none() && !self.synchronous {
                match Worker::start() {
                    Ok(worker) => self.worker = Some(worker),
                    Err(_) => self.synchronous = true,
                }
            }
            self.job = None;
            self.language = language.into();
            self.generation = settings.generation;
            self.source = Some(source.clone());
            if let Some(worker) = &self.worker {
                self.revision = worker.submit(source, language.into(), settings.clone(), ctx);
            } else {
                // Resource exhaustion must not disable syntax permanently.
                self.parsed = parse(source, language, &settings, self.parsed.as_deref(), || {
                    false
                })
                .map(Arc::new);
            }
        }
        if let Some(worker) = &self.worker {
            let result = worker.shared.mailbox.lock().unwrap().result.take();
            if let Some((revision, parsed)) = result
                && revision == self.revision
                && parsed.generation == self.generation
                && parsed.language == self.language
                && self.source.as_deref() == Some(parsed.source.as_ref())
            {
                self.parsed = Some(parsed);
                self.job = None;
            }
        }
        if self
            .job
            .as_ref()
            .is_none_or(|(font, glow, _)| *font != size.to_bits() || *glow != settings.glow)
        {
            self.job = Some((
                size.to_bits(),
                settings.glow,
                self.projector.project(
                    self.source.as_ref().unwrap().clone(),
                    &self.language,
                    self.parsed.as_deref(),
                    size,
                    &settings,
                ),
            ));
        }
        self.job.as_ref().unwrap().2.clone()
    }

    /// Whether the latest exact source and palette have completed syntax.
    pub(crate) fn settled(&self) -> bool {
        self.parsed.as_ref().is_some_and(|p| {
            self.source.as_deref() == Some(p.source.as_ref())
                && self.language == p.language
                && self.generation == p.generation
        })
    }
}

#[cfg(test)]
mod tests;
