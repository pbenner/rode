//! A lazy, bounded text index on its own killable interpreter. Parsing and search
//! never hold the raster lock or the UI snapshot lock. Requests coalesce in place.
use super::*;
use std::sync::{Mutex, mpsc};

const INDEX_LIMIT: usize = 64 * 1024 * 1024;
pub(super) const MATCH_LIMIT: usize = 10_000;

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Request {
    pub visible: Vec<usize>,
    pub query: String,
    pub case_sensitive: bool,
    pub revision: u64,
    pub search: bool,
}
#[derive(Default)]
pub(super) struct Snapshot {
    pub pages: std::collections::BTreeMap<usize, Arc<pdf::PageText>>,
    pub matches: Vec<Hit>,
    pub revision: u64,
    pub indexed: usize,
    pub searching: bool,
    pub error: Option<String>,
    pub limited: bool,
}
pub(super) struct Worker {
    request: Arc<Mutex<Request>>,
    pub output: Arc<Mutex<Snapshot>>,
    wake: mpsc::SyncSender<()>,
    cancel: Arc<AtomicBool>,
    task: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
    }
}
impl Worker {
    #[cfg(test)]
    pub(super) fn fixture(snapshot: Snapshot) -> Self {
        let (wake, _) = mpsc::sync_channel(1);
        Self {
            request: Default::default(),
            output: Arc::new(Mutex::new(snapshot)),
            wake,
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
        }
    }
    pub fn new(source: Arc<pdf::Source>, ctx: egui::Context) -> Self {
        let (wake, receiver) = mpsc::sync_channel(1);
        let mut worker = Self {
            request: Default::default(),
            output: Default::default(),
            wake,
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
        };
        let request = worker.request.clone();
        let output = worker.output.clone();
        let cancel = worker.cancel.clone();
        worker.task = Some(std::thread::spawn(move || {
            run(source, ctx, request, output, cancel, receiver)
        }));
        worker
    }
    pub fn stop(mut self) -> Option<std::thread::JoinHandle<()>> {
        self.cancel.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
        self.task.take()
    }
    pub fn request(&self, value: Request) {
        let mut request = self.request.lock().unwrap();
        if *request != value {
            *request = value;
            let _ = self.wake.try_send(());
        }
    }
}

fn run(
    source: Arc<pdf::Source>,
    ctx: egui::Context,
    request: Arc<Mutex<Request>>,
    output: Arc<Mutex<Snapshot>>,
    cancel: Arc<AtomicBool>,
    wake: mpsc::Receiver<()>,
) {
    let mut reader = None;
    let mut retained = 0;
    let mut failed = std::collections::HashSet::new();
    let mut revision = u64::MAX;
    let mut full = false;
    loop {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let wanted = request.lock().unwrap().clone();
        if revision != wanted.revision {
            let pages = output.lock().unwrap().pages.clone();
            let mut matches = Vec::new();
            for (page, text) in pages {
                if cancel.load(Ordering::Acquire) {
                    return;
                }
                if request.lock().unwrap().revision != wanted.revision {
                    break;
                }
                matches.extend(
                    find(
                        &text.text,
                        &wanted.query,
                        wanted.case_sensitive,
                        MATCH_LIMIT - matches.len(),
                    )
                    .into_iter()
                    .map(|range| Hit { page, range }),
                );
                if matches.len() == MATCH_LIMIT {
                    break;
                }
            }
            if request.lock().unwrap().revision != wanted.revision {
                continue;
            }
            let mut result = output.lock().unwrap();
            result.matches = matches;
            result.limited = result.matches.len() == MATCH_LIMIT;
            result.revision = wanted.revision;
            revision = wanted.revision;
            ctx.request_repaint();
        }
        let number = if full {
            None
        } else {
            let result = output.lock().unwrap();
            wanted
                .visible
                .iter()
                .copied()
                .chain((1..=source.pages).filter(|_| {
                    wanted.search && !wanted.query.trim().is_empty() && !result.limited
                }))
                .find(|page| {
                    (1..=source.pages).contains(page)
                        && !result.pages.contains_key(page)
                        && !failed.contains(page)
                })
        };
        let Some(number) = number else {
            output.lock().unwrap().searching = false;
            ctx.request_repaint();
            if wake.recv().is_err() {
                return;
            }
            continue;
        };
        output.lock().unwrap().searching = wanted.search && !wanted.query.trim().is_empty();
        let extracted = (|| -> anyhow::Result<pdf::PageText> {
            if reader.is_none() {
                reader = Some(source.text_reader(&cancel)?);
            }
            reader.as_mut().unwrap().page(number, &cancel)
        })();
        if cancel.load(Ordering::Acquire) {
            return;
        }
        match extracted {
            Ok(page) => {
                retained += page.memory_size();
                if retained > INDEX_LIMIT {
                    let mut result = output.lock().unwrap();
                    result.error =
                        Some("Text index reached its 64 MiB limit; results are partial.".into());
                    result.searching = false;
                    full = true;
                    ctx.request_repaint();
                    continue;
                }
                let ranges = find(
                    &page.text,
                    &wanted.query,
                    wanted.case_sensitive,
                    MATCH_LIMIT,
                );
                let mut result = output.lock().unwrap();
                result.pages.insert(number, Arc::new(page));
                if result.revision == wanted.revision {
                    let available = MATCH_LIMIT - result.matches.len();
                    result
                        .matches
                        .extend(ranges.into_iter().take(available).map(|range| Hit {
                            page: number,
                            range,
                        }));
                    result.matches.sort_by_key(|hit| (hit.page, hit.range[0]));
                    result.limited = result.matches.len() == MATCH_LIMIT;
                }
            }
            Err(error) => {
                reader = None;
                failed.insert(number);
                output.lock().unwrap().error = Some(format!("Page {number}: {error:#}"));
            }
        }
        output.lock().unwrap().indexed += 1;
        ctx.request_repaint();
    }
}

/// Normalize whitespace and case while retaining original UTF-8 boundaries,
/// including lowercase expansions and ligature strings supplied by ToUnicode.
pub(super) fn find(text: &str, query: &str, case_sensitive: bool, limit: usize) -> Vec<[usize; 2]> {
    fn normalize(value: &str, case_sensitive: bool) -> (String, Vec<[usize; 2]>) {
        let mut text = String::new();
        let mut map: Vec<[usize; 2]> = Vec::new();
        for (start, ch) in value.char_indices() {
            let end = start + ch.len_utf8();
            if ch.is_whitespace() && text.ends_with(' ') {
                if let Some(last) = map.last_mut() {
                    *last = [last[0], end];
                }
                continue;
            }
            // Common presentation ligatures should match ordinary typed words.
            let ligature = match ch {
                'ﬀ' => Some("ff"),
                'ﬁ' => Some("fi"),
                'ﬂ' => Some("fl"),
                'ﬃ' => Some("ffi"),
                'ﬄ' => Some("ffl"),
                'ﬅ' | 'ﬆ' => Some("st"),
                _ => None,
            };
            let value = if let Some(ligature) = ligature {
                ligature.into()
            } else if ch.is_whitespace() {
                " ".into()
            } else if case_sensitive {
                ch.to_string()
            } else {
                ch.to_lowercase().collect()
            };
            map.extend(std::iter::repeat_n([start, end], value.len()));
            text.push_str(&value);
        }
        (text, map)
    }
    let (needle, _) = normalize(query.trim(), case_sensitive);
    if needle.is_empty() || limit == 0 {
        return vec![];
    }
    let (haystack, map) = normalize(text, case_sensitive);
    haystack
        .match_indices(&needle)
        .take(limit)
        .map(|(start, value)| [map[start][0], map[start + value.len() - 1][1]])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_preserves_utf8_ranges_case_expansions_whitespace_and_match_limits() {
        let text = "École\n\tİSTANBUL ﬁnish 日本語";
        for (query, expected) in [
            ("éCOLE i\u{307}stanbul", "École\n\tİSTANBUL"),
            ("日本語", "日本語"),
            ("ﬁnish", "ﬁnish"),
            ("finish", "ﬁnish"),
        ] {
            let hits = find(text, query, false, 10);
            assert_eq!(hits.len(), 1);
            assert_eq!(&text[hits[0][0]..hits[0][1]], expected);
        }
        assert!(find(text, "école", true, 10).is_empty());
        assert!(find(text, " \n", false, 10).is_empty());
        assert_eq!(find("a a a", "a", false, 2), [[0, 1], [2, 3]]);
    }
}
