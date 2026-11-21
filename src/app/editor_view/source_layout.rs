//! Native source galleys assembled from immutable styled chunks.
//!
//! A tiny native-cache sentinel is resolved every frame. egui recreates its galley
//! cache together with the font atlas; changed font definitions, DPI or coverage
//! therefore replace the sentinel and invalidate every retained chunk. Ordinary
//! cache eviction leaves atlas coordinates valid. Keeping the sentinel hot gives
//! idle frames an O(1) validation instead of hashing every token in a large file.
use crate::theme::source::Layout;
use eframe::egui::{self, text::LayoutJob};
use std::sync::Arc;

#[derive(Default)]
pub(in crate::app) struct Cache {
    source: Option<Arc<Layout>>,
    sentinel: Option<Arc<egui::Galley>>,
    chunks: Vec<Arc<egui::Galley>>,
    galley: Option<Arc<egui::Galley>>,
    width: f32,
}
impl Cache {
    pub fn layout(&mut self, ui: &egui::Ui, source: Arc<Layout>, width: f32) -> Arc<egui::Galley> {
        let sentinel = ui.fonts_mut(|fonts| {
            fonts.layout_job(LayoutJob::simple(
                "source font cache".into(),
                source.font.clone(),
                egui::Color32::TRANSPARENT,
                f32::INFINITY,
            ))
        });
        let fonts_current = self
            .sentinel
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, &sentinel));
        let width_current = self.width == width;
        if fonts_current
            && width_current
            && self
                .source
                .as_ref()
                .is_some_and(|old| Arc::ptr_eq(old, &source))
            && let Some(galley) = &self.galley
        {
            return galley.clone();
        }
        let chunks: Vec<_> = ui.fonts_mut(|fonts| {
            source
                .chunks
                .iter()
                .enumerate()
                .map(|(index, job)| {
                    if fonts_current
                        && width_current
                        && let Some(old) = self.source.as_ref().and_then(|s| s.chunks.get(index))
                        && Arc::ptr_eq(old, job)
                    {
                        return self.chunks[index].clone();
                    }
                    let mut job = job.as_ref().clone();
                    job.wrap.max_width = width;
                    fonts.layout_job(job)
                })
                .collect()
        });
        let job = LayoutJob::simple(
            source.text.to_string(),
            source.font.clone(),
            source.foreground,
            width,
        );
        let galley = Arc::new(egui::Galley::concat(
            Arc::new(job),
            &chunks,
            ui.pixels_per_point(),
        ));
        self.width = width;
        self.source = Some(source);
        self.sentinel = Some(sentinel);
        self.chunks = chunks;
        self.galley = Some(galley.clone());
        galley
    }
}

#[cfg(test)]
mod tests;
