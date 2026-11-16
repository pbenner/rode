//! Bounded illustration sets and collision-free placement in unused UI space.
use super::{Artwork, Motif};
use eframe::egui::{Galley, Pos2, Rect, pos2, vec2};

#[derive(Clone, Copy)]
pub(super) enum Area {
    Source,
    Explorer,
    Welcome,
}

#[derive(Clone, Copy)]
enum Corner {
    BottomRight,
    BottomLeft,
    TopRight,
}

#[derive(Clone, Copy)]
pub(super) enum Tone {
    Ink,
    Silver,
    Gold,
}

#[derive(Clone, Copy)]
pub(super) struct Picture {
    pub motif: Motif,
    pub tone: Tone,
    corner: Corner,
    aspect: f32,
    maximum: f32,
    minimum: f32,
}

impl Picture {
    const fn new(
        motif: Motif,
        tone: Tone,
        corner: Corner,
        aspect: f32,
        maximum: f32,
        minimum: f32,
    ) -> Self {
        Self {
            motif,
            tone,
            corner,
            aspect,
            maximum,
            minimum,
        }
    }
}

impl Artwork {
    /// Fixed, at-most-two illustration budgets; themes never supply file paths
    /// or executable placement rules. Existing Sumi-e geometry stays unchanged.
    pub(super) fn pictures(self, area: Area) -> &'static [Picture] {
        use Corner::{BottomLeft, BottomRight, TopRight};
        use Motif::{Bamboo, Castle, Kabuto, Pine, Plum, Tsuba, Warrior};
        use Tone::{Gold, Ink, Silver};
        const SUMI_SOURCE: &[Picture] = &[Picture::new(Pine, Ink, BottomRight, 1.5, 440., 144.)];
        const SUMI_EXPLORER: &[Picture] =
            &[Picture::new(Bamboo, Ink, BottomRight, 2. / 3., 180., 90.)];
        const SUMI_WELCOME: &[Picture] = &[Picture::new(Plum, Ink, BottomRight, 1.5, 440., 144.)];
        const SAMURAI_SOURCE: &[Picture] = &[
            Picture::new(Warrior, Silver, BottomRight, 1.5, 560., 180.),
            Picture::new(Tsuba, Gold, TopRight, 1., 112., 80.),
        ];
        const SAMURAI_EXPLORER: &[Picture] =
            &[Picture::new(Kabuto, Gold, BottomRight, 2. / 3., 200., 82.)];
        const SAMURAI_WELCOME: &[Picture] = &[
            Picture::new(Castle, Silver, BottomRight, 1.5, 520., 168.),
            Picture::new(Tsuba, Gold, BottomLeft, 1., 156., 84.),
        ];
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

#[derive(Clone, Copy)]
pub(super) struct Placement {
    pub picture: Picture,
    pub rect: Rect,
}

/// Use actual visible text rows rather than the buffer's overall dimensions.
/// Selection does not change these bounds or the resulting artwork positions.
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
    placements(canvas, pictures, |rect| {
        rows.iter().all(|row| {
            !row.rect()
                .translate(position.to_vec2())
                .expand2(vec2(24., 10.))
                .intersects(rect)
        })
    })
}

/// Fit each picture against both content and earlier pictures, with fixed
/// margins and five candidate sizes. This allocates no layout or interaction IDs.
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
            let origin = match picture.corner {
                Corner::BottomRight => bounds.max - size,
                Corner::BottomLeft => pos2(bounds.left(), bounds.bottom() - size.y),
                Corner::TopRight => pos2(bounds.right() - size.x, bounds.top()),
            };
            let rect = Rect::from_min_size(origin, size);
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
