//! Bounded Unicode text collection using the same bundled PDF interpreter.
//! This preserves text attachments without a separate pdftotext executable;
//! glyph drawing order is retained, rather than promising visual table layout.
use anyhow::{Result, ensure};
use hayro::{
    hayro_interpret::*,
    hayro_syntax::Pdf,
    kurbo::{Affine, BezPath, Rect},
};

#[derive(Default)]
struct Text {
    value: String,
    overflow: bool,
}
impl Text {
    fn push(&mut self, value: &str) {
        if self.value.len().saturating_add(value.len()) > rode_agent_core::attachment::CONTENT_LIMIT
        {
            self.overflow = true;
        } else if !self.overflow {
            self.value.push_str(value);
        }
    }
}
impl<'a> Device<'a> for Text {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
    fn draw_glyph_run(&mut self, run: &font::GlyphRun<'_, 'a>, _: DrawProps<'a>, _: &DrawMode) {
        for glyph in run.glyphs() {
            match glyph.as_unicode() {
                Some(hayro_cmap::BfString::Char(ch)) => self.push(ch.encode_utf8(&mut [0; 4])),
                Some(hayro_cmap::BfString::String(value)) => self.push(&value),
                None => {}
            }
        }
        self.push("\n");
    }
}

pub(super) fn extract(document: &Pdf) -> Result<String> {
    let cache = InterpreterCache::new();
    let mut text = Text::default();
    for page in document.pages().iter() {
        let (width, height) = page.render_dimensions();
        let mut context = Context::new(
            Affine::IDENTITY,
            Rect::new(0., 0., width as f64, height as f64),
            &cache,
            page.xref(),
            InterpreterSettings::default(),
        );
        interpret_page(page, &mut context, &mut text);
        ensure!(!text.overflow, "PDF text exceeds the attachment limit.");
        text.push("\n");
    }
    ensure!(!text.overflow, "PDF text exceeds the attachment limit.");
    ensure!(
        !text.value.trim().is_empty(),
        "This PDF contains no extractable text. Attach page images or an OCR text version."
    );
    Ok(text.value)
}
