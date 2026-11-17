//! Embedded, static ink illustrations. Placement uses the editor's actual visible
//! text rows, so artwork never changes source contrast or owns an input region.
//! Each alpha-mask texture is shared by every viewport of an egui context; theme
//! switches only change its tint. No files, network requests or timers are used.
//! Source geometry comes from an unscrolled viewport captured by the editor;
//! visible text may suppress the illustration, but never shrink it while scrolling.
use super::Palette;
use eframe::egui::{self, Pos2, Rect, TextureHandle, pos2};
use layout::{Area, Placement, Tone, placements, source_placements};

mod layout;
use serde::Deserialize;

/// Optional embedded illustration, inherited independently of colors and chrome.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Artwork {
    /// Keep the editor canvas unillustrated.
    #[default]
    None,
    /// Original pine/crane, bamboo and plum-blossom ink-wash illustrations.
    SumiE,
    /// Original silver-gray samurai, kabuto helmet and castle ink illustrations.
    Samurai,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
enum Motif {
    Pine,
    Bamboo,
    Plum,
    Warrior,
    Kabuto,
    Castle,
}

impl Motif {
    /// Resolve only compiled-in originals. Archived assets such as the unused
    /// Samurai sword guard are deliberately absent from the executable.
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Pine => include_bytes!("../../assets/themes/sumie-ink.png"),
            Self::Bamboo => include_bytes!("../../assets/themes/sumie-bamboo.png"),
            Self::Plum => include_bytes!("../../assets/themes/sumie-plum.png"),
            Self::Warrior => include_bytes!("../../assets/themes/samurai-warrior.png"),
            Self::Kabuto => include_bytes!("../../assets/themes/samurai-kabuto.png"),
            Self::Castle => include_bytes!("../../assets/themes/samurai-castle.png"),
        }
    }
}

/// Cache successful decoding or failure; repeated frames never retry bad assets.
#[derive(Clone)]
struct InkTexture(Option<TextureHandle>);

/// Treat dark strokes as ink coverage rather than retaining a paper-colored
/// backdrop. The same transparent mask can then take light or dark palette ink.
fn ink_mask(motif: Motif) -> Option<egui::ColorImage> {
    let image = image::load_from_memory_with_format(motif.bytes(), image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let mut rgba = image.into_raw();
    for pixel in rgba.chunks_exact_mut(4) {
        let luminance =
            (54 * u32::from(pixel[0]) + 183 * u32::from(pixel[1]) + 19 * u32::from(pixel[2])) / 256;
        pixel[3] = ((u32::from(pixel[3]) * (255 - luminance) + 127) / 255) as u8;
        pixel[..3].fill(255);
    }
    Some(egui::ColorImage::from_rgba_unmultiplied(size, &rgba))
}

/// Lazily decode once per UI context/motif and share the texture across editor
/// windows. Palette tints stay out of the cache, so theme changes need no decode.
fn texture(ctx: &egui::Context, motif: Motif) -> Option<TextureHandle> {
    let id = egui::Id::new(("embedded-sumi-e-ink", motif));
    if let Some(cached) = ctx.data_mut(|data| data.get_temp::<InkTexture>(id)) {
        return cached.0;
    }
    let texture = ink_mask(motif)
        .map(|image| ctx.load_texture("Theme ink", image, egui::TextureOptions::LINEAR));
    // Cache failure as well: optional decoration must not retry decoding every frame.
    ctx.data_mut(|data| data.insert_temp(id, InkTexture(texture.clone())));
    texture
}

impl Palette {
    /// Paint only in unused source-canvas space. Selection leaves the artwork
    /// visible; dense or narrow buffers suppress it to protect source text.
    /// `viewport` must be captured before scrolling in the same logical screen
    /// coordinates as `ui.clip_rect()`, with the line-number gutter excluded.
    /// `output` supplies translated row positions for collision checks only.
    pub(crate) fn paint_source_artwork(
        self,
        ui: &egui::Ui,
        output: &egui::text_edit::TextEditOutput,
        viewport: Rect,
    ) {
        let pictures = self.artwork.pictures(Area::Source);
        if pictures.is_empty() {
            return;
        }
        // The caller captures this before scrolling the content. Widget/galley
        // positions move during horizontal scrolling and cannot anchor artwork.
        let canvas = viewport.intersect(ui.clip_rect());
        self.paint_placements(
            ui,
            canvas,
            source_placements(canvas, &output.galley, output.galley_pos, pictures),
        );
    }

    /// The file tree owns its scroll region. Its illustration occupies only
    /// remaining space below rows, scrollbars and sidebar controls.
    pub(crate) fn paint_explorer_artwork(self, ui: &egui::Ui) {
        let pictures = self.artwork.pictures(Area::Explorer);
        if pictures.is_empty() {
            return;
        }
        let canvas = ui.available_rect_before_wrap().intersect(ui.clip_rect());
        self.paint_placements(ui, canvas, placements(canvas, pictures, |_| true));
    }

    /// Welcome content reports occupied bounds in logical screen coordinates.
    /// Fit around those controls, shrinking or omitting artwork in smaller views.
    pub(crate) fn paint_welcome_artwork(self, ui: &egui::Ui, canvas: Rect, occupied: Rect) {
        let pictures = self.artwork.pictures(Area::Welcome);
        if pictures.is_empty() {
            return;
        }
        let canvas = canvas.intersect(ui.clip_rect());
        self.paint_placements(
            ui,
            canvas,
            placements(canvas, pictures, |rect| {
                !occupied.expand(16.).intersects(rect)
            }),
        );
    }

    /// Paint cached masks without allocating widget IDs, input regions or layout
    /// space. Clipping affects paint only; selection and focus remain untouched.
    fn paint_placements(self, ui: &egui::Ui, canvas: Rect, placements: [Option<Placement>; 2]) {
        for placed in placements.into_iter().flatten() {
            if let Some(texture) = texture(ui.ctx(), placed.picture.motif) {
                let tint = match placed.picture.tone {
                    Tone::Ink => self.muted.gamma_multiply(0.72),
                    Tone::Silver => self.muted.gamma_multiply(0.86),
                };
                ui.painter().with_clip_rect(canvas).image(
                    texture.id(),
                    placed.rect,
                    Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                    tint,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
