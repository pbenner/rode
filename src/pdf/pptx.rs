//! Native, read-only PowerPoint interpretation inside the existing private
//! document worker. The editable BetterOffice model stays intact; no conversion
//! to PDF, external application, filesystem mutation or remote font lookup occurs.
use super::renderer::Raster;
use super::*;

mod fonts;
mod text;

pub(super) struct Presentation {
    deck: betteroffice_pptx::Presentation,
    size: [f32; 2],
    warnings: Vec<String>,
}
impl Presentation {
    pub(super) fn open(bytes: &[u8]) -> Result<Self> {
        let mut deck = betteroffice_pptx::Presentation::open(bytes).context(
            "Cannot open this PowerPoint presentation: invalid, encrypted or unsupported PPTX",
        )?;
        ensure!(
            (1..=MAX_PAGES).contains(&deck.slides().len()),
            "PowerPoint previews support 1–10,000 slides."
        );
        let model = deck.model();
        // BetterOffice 0.5.1 snaps its page box to whole points before layout.
        // Match that geometry rather than scaling the unsnapped EMU extent:
        // custom slide sizes could otherwise rasterize beyond the reader limit.
        let size =
            [model.width_emu, model.height_emu].map(|emu| (emu as f64 / 12700.).round() as f32);
        ensure!(
            size.iter()
                .all(|n| n.is_finite() && (0.01..=1_000_000.).contains(n)),
            "Invalid PowerPoint slide dimensions."
        );
        let warnings = fonts::register(&mut deck)?;
        Ok(Self {
            deck,
            size,
            warnings,
        })
    }
    pub(super) fn sizes(&self) -> Vec<[f32; 2]> {
        vec![self.size; self.deck.slides().len()]
    }
    pub(super) fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
    pub(super) fn page_text(&self, page: usize) -> Result<PageText> {
        ensure!(
            (1..=self.deck.slides().len()).contains(&page),
            "Invalid slide text page."
        );
        let rendered = self
            .deck
            .render_slide(page - 1)
            .context("Cannot lay out this PowerPoint slide's text")?;
        text::collect(&rendered.display_list)
    }
    pub(super) fn render(&self, page: usize, side: usize) -> Result<Raster> {
        ensure!(
            (1..=self.deck.slides().len()).contains(&page),
            "Slide is outside this presentation."
        );
        // BetterOffice display lists use CSS pixels (96/inch). Rasterize to
        // the same bounded long edge as PDFs and decode only inside the worker.
        let css_side = (f64::from(self.size[0].max(self.size[1])) * 96. / 72.) as f32;
        let mut scale = side as f32 / css_side;
        // Raster dimensions use ceil. A rounded-up quotient can add a pixel
        // even when the mathematical result is exactly the admitted long edge.
        if css_side * scale > side as f32 {
            scale = scale.next_down();
        }
        let rendered = self
            .deck
            .render_png(
                page - 1,
                &betteroffice_pptx::RenderOptions {
                    scale,
                    max_shadow_pixels: 4 * 1024 * 1024,
                    ..Default::default()
                },
            )
            .context("Cannot render this PowerPoint slide")?;
        ensure!(
            rendered.width > 0
                && rendered.height > 0
                && rendered.width as usize <= side
                && rendered.height as usize <= side,
            "PowerPoint raster exceeds its dimensions limit"
        );
        let image = image::load_from_memory_with_format(&rendered.bytes, image::ImageFormat::Png)
            .context("Cannot decode the native PowerPoint raster")?
            .into_rgba8();
        ensure!(
            image.width() == rendered.width && image.height() == rendered.height,
            "PowerPoint raster dimensions disagree"
        );
        Ok(Raster {
            width: rendered.width as usize,
            height: rendered.height as usize,
            pixels: image.into_raw(),
            skipped_images: rendered.skipped_images,
        })
    }
}
