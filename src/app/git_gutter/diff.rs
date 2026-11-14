//! Bounded line comparison and viewport-clipped gutter geometry.
use super::*;
use std::{fmt::Write as _, ops::Range};

/// Semantic line decoration relative to the captured Git baseline, including deletion boundaries
/// between current lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Added,
    Modified,
    Deleted,
}

/// Zero-based source-line span to decorate; deletion markers address a boundary before `line`
/// rather than an existing removed row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Marker {
    /// Zero-based line; deletions sit at the boundary before this line.
    pub line: usize,
    /// Current lines for additions/modifications, or removed baseline lines for a deletion.
    /// Deleted lines are summarized at one boundary, not painted over this many current rows.
    pub count: usize,
    pub kind: Kind,
}

/// One exact edit operation with zero-based, half-open HEAD/current line ranges.
/// Split replacement markers share this same preview, including final-newline state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Hunk {
    /// Half-open index range into the paired `Changes::markers` vector.
    pub markers: Range<usize>,
    /// Zero-based, half-open line range in the normalized HEAD snapshot.
    pub old: Range<usize>,
    /// Zero-based, half-open line range in the normalized current source snapshot.
    pub new: Range<usize>,
    /// Unified patch for this operation, retaining missing-final-newline information.
    pub patch: String,
}

/// Decorations and previews are produced together from one bounded comparison.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Changes {
    pub markers: Vec<Marker>,
    pub hunks: Vec<Hunk>,
}

/// Compare Unicode line content, normalizing CRLF as Git's text checkouts do
/// while retaining missing final newlines. A deadline failure omits markers
/// instead of displaying an approximate whole-file replacement. Large documents
/// never enter the diff algorithm.
pub(super) fn compare(old: &str, new: &str) -> Option<Changes> {
    if [old, new].iter().any(|s| {
        s.len() > MAX_BYTES
            || s.contains('\0')
            || s.bytes().filter(|b| *b == b'\n').count() >= MAX_LINES
    }) {
        return None;
    }
    // HEAD stores LF for autocrlf/text attributes even when a clean checkout has
    // CRLF. This changes neither the source buffer nor its save/dirty baseline.
    let normalize = |s: &str| s.replace("\r\n", "\n");
    let old = normalize(old);
    let new = normalize(new);
    let deadline = Instant::now() + Duration::from_millis(75);
    let diff = similar::TextDiff::configure()
        .deadline(deadline)
        .diff_lines(&old, &new);
    if Instant::now() >= deadline {
        return None;
    }
    let mut changes = Changes::default();
    for op in diff.ops() {
        let (tag, old, new) = op.as_tag_tuple();
        if tag == similar::DiffTag::Equal {
            continue;
        }
        let markers = &mut changes.markers;
        let first = markers.len();
        let (removed, inserted) = (old.len(), new.len());
        use similar::DiffTag;
        match tag {
            DiffTag::Equal => {}
            DiffTag::Delete => markers.push(Marker {
                line: new.start,
                count: removed,
                kind: Kind::Deleted,
            }),
            DiffTag::Insert => markers.push(Marker {
                line: new.start,
                count: inserted,
                kind: Kind::Added,
            }),
            DiffTag::Replace => {
                let common = removed.min(inserted);
                if common > 0 {
                    markers.push(Marker {
                        line: new.start,
                        count: common,
                        kind: Kind::Modified,
                    });
                }
                if inserted > common {
                    markers.push(Marker {
                        line: new.start + common,
                        count: inserted - common,
                        kind: Kind::Added,
                    });
                }
                if removed > common {
                    markers.push(Marker {
                        line: new.end,
                        count: removed - common,
                        kind: Kind::Deleted,
                    });
                }
            }
        }
        let mut patch = format!(
            "@@ -{},{} +{},{} @@\n",
            old.start + usize::from(!old.is_empty()),
            old.len(),
            new.start + usize::from(!new.is_empty()),
            new.len(),
        );
        for change in diff.iter_changes(op) {
            let prefix = match change.tag() {
                similar::ChangeTag::Delete => '-',
                similar::ChangeTag::Insert => '+',
                similar::ChangeTag::Equal => ' ',
            };
            let _ = write!(patch, "{prefix}{}", change.value());
            if change.missing_newline() {
                patch.push_str("\n\\ No newline at end of file\n");
            }
        }
        changes.hunks.push(Hunk {
            markers: first..markers.len(),
            old,
            new,
            patch,
        });
    }
    Some(changes)
}

/// Paint clipped markers using actual placed source rows and return the clicked marker index.
/// Logical line numbers are zero-based and never inferred from font height.
pub(super) fn paint(
    ui: &mut egui::Ui,
    markers: &[Marker],
    id: u64,
    gutter: egui::Rect,
    rows: editor_view::gutter::Rows<'_>,
    palette: theme::Palette,
) -> Option<usize> {
    let clip = gutter.intersect(ui.clip_rect());
    if !clip.is_positive() {
        return None;
    }
    let painter = ui.painter().with_clip_rect(clip);
    let x = gutter.right() - 8.;
    let mut clicked = None;
    for (index, marker) in markers.iter().enumerate() {
        let span = if marker.kind == Kind::Deleted {
            let boundary = rows.boundary(marker.line);
            egui::Rect::from_min_max(egui::pos2(x, boundary), egui::pos2(x, boundary))
        } else {
            let Some(span) = rows.span(marker.line, marker.count) else {
                continue;
            };
            span
        };
        // Skip offscreen hunks before allocating hover text or probe identities.
        if span.top() > clip.bottom() + 3. || span.bottom() < clip.top() - 3. {
            continue;
        }
        let (rect, label, name, color) = match marker.kind {
            Kind::Added | Kind::Modified => {
                let added = marker.kind == Kind::Added;
                (
                    egui::Rect::from_min_max(
                        egui::pos2(x - 2., span.top()),
                        egui::pos2(x + 2., span.bottom()),
                    ),
                    if added {
                        "Added since HEAD"
                    } else {
                        "Modified since HEAD"
                    }
                    .to_owned(),
                    if added { "added" } else { "modified" },
                    if added {
                        palette.success
                    } else {
                        egui::Color32::from_rgb(60, 145, 235)
                    },
                )
            }
            Kind::Deleted => {
                let padding = 3_f32.min(gutter.height() / 2.);
                let y = span
                    .top()
                    .clamp(gutter.top() + padding, gutter.bottom() - padding);
                (
                    egui::Rect::from_center_size(egui::pos2(x, y), egui::vec2(7., 6.)),
                    format!("{} deleted line(s) since HEAD", marker.count),
                    "deleted",
                    palette.error,
                )
            }
        };
        let visible = rect.intersect(clip);
        if !visible.is_positive() {
            continue;
        }
        if marker.kind == Kind::Deleted {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    rect.left_top(),
                    egui::pos2(rect.right(), rect.center().y),
                    rect.left_bottom(),
                ],
                color,
                Stroke::NONE,
            ));
        } else {
            painter.rect_filled(visible, 0., color);
        }
        let response = ui
            .interact(
                visible.expand2(egui::vec2(2., 0.)).intersect(clip),
                egui::Id::new(("git-gutter", id, name, marker.line)),
                Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!("{label} · Click to view or revert this change"));
        ui_probe::button(
            ui,
            &format!("git-change-{name}:{}", marker.line + 1),
            &response,
        );
        if response.clicked() {
            clicked = Some(index);
        }
    }
    clicked
}
