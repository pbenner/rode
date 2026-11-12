//! Bounded line comparison and viewport-clipped gutter geometry.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Added,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Marker {
    /// Zero-based line; deletions sit at the boundary before this line.
    pub line: usize,
    pub count: usize,
    pub kind: Kind,
}

/// Compare Unicode line content, normalizing CRLF as Git's text checkouts do
/// while retaining missing final newlines. A deadline failure omits markers
/// instead of displaying an approximate whole-file replacement. Large documents
/// never enter the diff algorithm.
pub(super) fn markers(old: &str, new: &str) -> Option<Vec<Marker>> {
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
    let mut markers = Vec::new();
    for op in diff.ops() {
        let (tag, old, new) = op.as_tag_tuple();
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
    }
    Some(markers)
}

pub(super) fn paint(
    ui: &mut egui::Ui,
    markers: &[Marker],
    id: u64,
    gutter: egui::Rect,
    rows: editor_view::gutter::Rows<'_>,
    palette: theme::Palette,
) {
    let clip = gutter.intersect(ui.clip_rect());
    if !clip.is_positive() {
        return;
    }
    let painter = ui.painter().with_clip_rect(clip);
    let x = gutter.right() - 8.;
    for marker in markers {
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
                visible,
                egui::Id::new(("git-gutter", id, name, marker.line)),
                Sense::hover(),
            )
            .on_hover_text(label);
        ui_probe::button(
            ui,
            &format!("git-change-{name}:{}", marker.line + 1),
            &response,
        );
    }
}
