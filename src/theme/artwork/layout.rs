//! Bounded illustration sets and collision-free placement in unused UI space.
//! Geometry uses logical egui screen coordinates within each native viewport,
//! rather than physical desktop pixels. Source
//! illustrations choose a rectangle before checking visible text; scrolling may
//! hide that rectangle, but must not feed back into its size or anchor. Explorer
//! and welcome surfaces may instead fit artwork around their occupied content.
use super::{Artwork, Motif};
use eframe::egui::{Galley, Pos2, Rect, vec2};

/// Surface selecting a fixed native picture set, independent of theme colors.
#[derive(Clone, Copy)]
pub(super) enum Area {
    Source,
    Explorer,
    Welcome,
}

/// Palette-derived ink treatment; Samurai uses the same neutral tone everywhere.
#[derive(Clone, Copy)]
pub(super) enum Tone {
    Ink,
    Silver,
}

/// Embedded motif and its viewport-fit limits. Dimensions are logical points;
/// an image below its minimum readable width is omitted rather than enlarged.
#[derive(Clone, Copy)]
pub(super) struct Picture {
    /// Embedded original, never a path or URL supplied by a theme definition.
    pub motif: Motif,
    /// Semantic palette tint, applied when painting the shared alpha mask.
    pub tone: Tone,
    /// Width divided by height; fitting preserves this ratio.
    aspect: f32,
    /// Largest display width, before clamping to the viewport's available space.
    maximum: f32,
    /// Smallest display width at which fine artwork detail remains readable.
    minimum: f32,
}

impl Picture {
    const fn new(motif: Motif, tone: Tone, aspect: f32, maximum: f32, minimum: f32) -> Self {
        Self {
            motif,
            tone,
            aspect,
            maximum,
            minimum,
        }
    }
}

impl Artwork {
    /// Fixed native picture sets, bounded by the two-slot placement capacity.
    /// Each current surface uses one illustration. Sumi-e retains its original
    /// motifs and size limits; Samurai consistently uses silver-gray ink.
    /// Theme definitions cannot supply file paths or executable placement rules.
    pub(super) fn pictures(self, area: Area) -> &'static [Picture] {
        use Motif::{Bamboo, Castle, Kabuto, Pine, Plum, Warrior};
        use Tone::{Ink, Silver};
        const SUMI_SOURCE: &[Picture] = &[Picture::new(Pine, Ink, 1.5, 440., 144.)];
        const SUMI_EXPLORER: &[Picture] = &[Picture::new(Bamboo, Ink, 2. / 3., 180., 90.)];
        const SUMI_WELCOME: &[Picture] = &[Picture::new(Plum, Ink, 1.5, 440., 144.)];
        const SAMURAI_SOURCE: &[Picture] = &[Picture::new(Warrior, Silver, 1.5, 560., 180.)];
        const SAMURAI_EXPLORER: &[Picture] = &[Picture::new(Kabuto, Silver, 2. / 3., 200., 82.)];
        const SAMURAI_WELCOME: &[Picture] = &[Picture::new(Castle, Silver, 1.5, 520., 168.)];
        match (self, area) {
            (Self::None, _) => &[],
            (Self::SumiE, Area::Source) => SUMI_SOURCE,
            (Self::SumiE, Area::Explorer) => SUMI_EXPLORER,
            (Self::SumiE, Area::Welcome) => SUMI_WELCOME,
            (Self::Samurai, Area::Source) => SAMURAI_SOURCE,
            (Self::Samurai, Area::Explorer) => SAMURAI_EXPLORER,
            (Self::Samurai, Area::Welcome) => SAMURAI_WELCOME,
        }
    }
}

/// A picture's full unclipped display rectangle in viewport/screen coordinates.
#[derive(Clone, Copy)]
pub(super) struct Placement {
    pub picture: Picture,
    pub rect: Rect,
}

/// Use actual visible text rows rather than the buffer's overall dimensions.
/// Size and anchor depend only on the viewport, never the visible source rows.
/// Scrolling or selection cannot resize the picture; collisions only hide it.
/// `canvas` is the unscrolled source viewport, and `position` translates the
/// galley's local rows into the same screen coordinates after both scroll axes.
/// Returned rectangles preserve their aspect ratio and exclude the gutter.
pub(super) fn source_placements(
    canvas: Rect,
    galley: &Galley,
    position: Pos2,
    pictures: &[Picture],
) -> [Option<Placement>; 2] {
    if !canvas.is_finite() || !canvas.is_positive() || !position.is_finite() {
        return [None; 2];
    }
    let first = galley
        .rows
        .partition_point(|row| row.rect().bottom() + position.y < canvas.top() - 10.);
    let last = galley
        .rows
        .partition_point(|row| row.rect().top() + position.y <= canvas.bottom() + 10.);
    let rows = &galley.rows[first..last];
    // Fit geometry without source-dependent constraints. Passing the row test
    // into placements() would reintroduce size changes as long lines scroll by.
    let mut result = placements(canvas, pictures, |_| true);
    for placed in &mut result {
        if placed.is_some_and(|placed| {
            rows.iter().any(|row| {
                row.rect()
                    .translate(position.to_vec2())
                    .expand2(vec2(24., 10.))
                    .intersects(placed.rect)
            })
        }) {
            *placed = None;
        }
    }
    result
}

/// Fit each picture against both content and earlier pictures, with fixed
/// margins and five candidate sizes. This allocates no layout or interaction IDs.
/// Accept at most two native pictures and a clearance predicate in screen
/// coordinates. Invalid/too-small canvases or unreadable pictures produce empty
/// slots. Source rendering uses an unconditional predicate and filters afterward
/// so only non-source surfaces adapt their sizes to changing content.
pub(super) fn placements(
    canvas: Rect,
    pictures: &[Picture],
    clear: impl Fn(Rect) -> bool,
) -> [Option<Placement>; 2] {
    let mut result: [Option<Placement>; 2] = [None; 2];
    if !canvas.is_finite() || !canvas.is_positive() {
        return result;
    }
    let bounds = canvas.shrink(20.);
    if !bounds.is_positive() {
        return result;
    }
    debug_assert!(pictures.len() <= result.len());
    for (index, &picture) in pictures.iter().take(result.len()).enumerate() {
        let width = bounds
            .width()
            .min(picture.maximum)
            .min(bounds.height() * picture.aspect);
        for scale in [1., 0.85, 0.7, 0.55, 0.4] {
            let width = width * scale;
            if width < picture.minimum {
                break;
            }
            let size = vec2(width, width / picture.aspect);
            let rect = Rect::from_min_size(bounds.max - size, size);
            if clear(rect)
                && result[..index]
                    .iter()
                    .flatten()
                    .all(|earlier| !earlier.rect.expand(12.).intersects(rect))
            {
                result[index] = Some(Placement { picture, rect });
                break;
            }
        }
    }
    result
}
