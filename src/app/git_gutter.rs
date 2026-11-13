//! Read-only Git decorations for the selected source buffer, relative to HEAD.
//!
//! Git plumbing and bounded line diffs run on one worker per window. The captured
//! project policy applies equally to local and SSH reads; the Git panel's explicit
//! host-mode preference never grants background reads. Stable document, request and
//! edit identities prevent late replies from decorating a different source snapshot.
use super::*;
use crate::sandbox::PlanExt as _;

mod diff;
mod peek;
mod repository;
#[cfg(test)]
mod tests;
use diff::Changes;

const REFRESH: Duration = Duration::from_secs(2);
const DEBOUNCE: Duration = Duration::from_millis(150);
const MAX_BYTES: usize = 1024 * 1024;
const MAX_LINES: usize = 50_000;

#[derive(Clone, PartialEq, Eq)]
struct Key {
    id: u64,
    path: String,
}

/// Cache only the selected document. Reset cancels work without releasing its
/// concurrency slot until that worker actually exits, even after a project switch.
#[derive(Default)]
pub(super) struct State {
    key: Option<Key>,
    token: u64,
    pending: Option<u64>,
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    baseline: Option<repository::Baseline>,
    refreshed: Option<Instant>,
    observed: Option<(u64, Instant)>,
    rendered: Option<(u64, Changes)>,
    peek: Option<usize>,
}

pub(super) struct Completed {
    token: u64,
    version: u64,
    refreshed: bool,
    baseline: Option<repository::Baseline>,
    changes: Changes,
}

pub(super) enum PeekAction {
    Navigate(usize),
    Close,
}

/// Clear the global concurrency slot on every exit, including cancellation/panic.
struct Running(Arc<AtomicBool>);
impl Drop for Running {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl State {
    pub(super) fn reset(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.key = None;
        self.token = self.token.wrapping_add(1);
        self.pending = None;
        self.baseline = None;
        self.refreshed = None;
        self.observed = None;
        self.rendered = None;
        self.peek = None;
    }

    fn select(&mut self, key: Key, version: u64, now: Instant) {
        if self.key.as_ref() != Some(&key) {
            self.reset();
            self.key = Some(key);
        }
        if self.observed.is_none_or(|(v, _)| v != version) {
            self.peek = None;
            // Initial display is immediate; subsequent edits are debounced.
            let changed = if self.observed.is_none() {
                now - DEBOUNCE
            } else {
                now
            };
            self.observed = Some((version, changed));
        }
    }

    fn due(&self, now: Instant) -> Option<bool> {
        if self.pending.is_some() || self.busy.load(Ordering::Acquire) {
            return None;
        }
        let (version, changed) = self.observed?;
        if now.duration_since(changed) < DEBOUNCE {
            return None;
        }
        let refresh = self
            .refreshed
            .is_none_or(|t| now.duration_since(t) >= REFRESH);
        (refresh || self.rendered.as_ref().is_none_or(|(v, _)| *v != version)).then_some(refresh)
    }

    pub(super) fn complete(&mut self, result: Completed) {
        if self.pending != Some(result.token) {
            return;
        }
        self.pending = None;
        self.baseline = result.baseline;
        if result.refreshed {
            self.refreshed = Some(Instant::now());
        }
        // Painting also checks the current version, so an edit in this frame or
        // between dispatch and completion cannot temporarily shift old markers.
        if self.peek.is_some()
            && self
                .rendered
                .as_ref()
                .is_none_or(|(v, changes)| *v != result.version || changes != &result.changes)
        {
            self.peek = None;
        }
        self.rendered = Some((result.version, result.changes));
    }

    /// Keep decorations and open previews tied to one document and buffer revision.
    fn current(&self, doc: &Document) -> Option<&Changes> {
        let key = self.key.as_ref()?;
        if key.id != doc.id || doc.path.as_ref() != Some(&key.path) {
            return None;
        }
        self.rendered
            .as_ref()
            .filter(|(version, _)| *version == doc.edit_version)
            .map(|(_, changes)| changes)
    }

    /// Draw only admitted, current markers in the strip between numbers and text.
    pub(super) fn paint(
        &self,
        ui: &mut egui::Ui,
        doc: &Document,
        gutter: egui::Rect,
        rows: editor_view::gutter::Rows<'_>,
        palette: theme::Palette,
    ) -> Option<usize> {
        if let Some(changes) = self.current(doc) {
            let response = ui.interact(
                gutter.intersect(ui.clip_rect()),
                egui::Id::new(("git-gutter-state", doc.id)),
                Sense::hover(),
            );
            ui_probe::button(
                ui,
                if self.baseline.is_some() {
                    "source-git-changes-ready"
                } else {
                    "source-git-unavailable"
                },
                &response,
            );
            let marker = diff::paint(ui, &changes.markers, doc.id, gutter, rows, palette)?;
            return changes
                .hunks
                .iter()
                .position(|hunk| hunk.markers.contains(&marker));
        }
        None
    }
}

impl EditorApp {
    /// Schedule Git reads/diffs independently of the Git panel and editor layout.
    pub(super) fn git_gutter_tick(&mut self, ctx: &egui::Context) {
        let eligible = self.project_ready()
            && self.feature(crate::extensions::Feature::Git)
            && self.navigation.visible(navigation::Pane::Editor);
        let doc = self
            .documents
            .active()
            .filter(|d| !d.is_preview() && d.text.len() <= MAX_BYTES && d.lines <= MAX_LINES);
        let key = eligible.then_some(doc).flatten().and_then(|d| {
            let path = d.path.as_ref()?;
            repository::location(&self.workspace.as_ref()?.root, path)?;
            Some(Key {
                id: d.id,
                path: path.clone(),
            })
        });
        let Some(key) = key else {
            if self.git_gutter.key.is_some() {
                self.git_gutter.reset();
            }
            return;
        };
        let doc = doc.unwrap();
        let now = Instant::now();
        self.git_gutter.select(key.clone(), doc.edit_version, now);
        let delay = if self.git_gutter.pending.is_some() {
            REFRESH
        } else if self
            .git_gutter
            .rendered
            .as_ref()
            .is_some_and(|(v, _)| *v == doc.edit_version)
        {
            self.git_gutter
                .refreshed
                .map_or(REFRESH, |t| REFRESH.saturating_sub(t.elapsed()))
        } else {
            DEBOUNCE
        };
        ctx.request_repaint_after(delay.max(Duration::from_millis(20)));
        let Some(refresh) = self.git_gutter.due(now) else {
            return;
        };
        self.git_gutter.token = self.git_gutter.token.wrapping_add(1);
        let token = self.git_gutter.token;
        self.git_gutter.pending = Some(token);
        self.git_gutter.busy.store(true, Ordering::Release);
        let running = Running(self.git_gutter.busy.clone());
        let cancel = self.git_gutter.cancel.clone();
        let baseline = self.git_gutter.baseline.clone();
        let text = doc.text.clone();
        let version = doc.edit_version;
        let workspace = self.workspace.as_ref().unwrap();
        let root = workspace.root.clone();
        let remote = workspace.remote.clone();
        let plan = workspace.plan.clone();
        let vault_work = self.vaults.work();
        let replies = self.workers.workspace(ctx);
        std::thread::spawn(move || {
            let _running = running;
            let _vault_work = vault_work;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let baseline = if refresh {
                    repository::baseline(&root, &key.path, baseline.as_ref(), |root, args| {
                        if cancel.load(Ordering::Acquire) {
                            return None;
                        }
                        let request = Request::Tool {
                            request: crate::project::Request::Git {
                                root: root.into(),
                                args,
                                input: None,
                            },
                        };
                        let response = match &remote {
                            Some(remote) => remote.request_in(&request, Some(&plan)),
                            None => plan.local_request(&request),
                        }
                        .ok()?;
                        match response.tool? {
                            crate::project::Response::Command(output) => Some(output),
                            _ => None,
                        }
                    })
                } else {
                    baseline
                };
                let changes = if cancel.load(Ordering::Acquire) {
                    Changes::default()
                } else {
                    baseline
                        .as_ref()
                        .and_then(|base| diff::compare(&base.text, &text))
                        .unwrap_or_default()
                };
                (baseline, changes)
            }))
            .unwrap_or_default();
            if !cancel.load(Ordering::Acquire) {
                let _ = replies.send(WorkspaceEvent::GitGutter(Completed {
                    token,
                    version,
                    refreshed: refresh,
                    baseline: result.0,
                    changes: result.1,
                }));
            }
        });
    }
}
