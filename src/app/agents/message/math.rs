//! Bounded, worker-rendered formula textures. No TeX process, file loader or network access.
use eframe::egui;
use latex_rust::{Dim, MathFont, MathStyle, PngOptions};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
};

const MAX_SOURCE: usize = 4096;
const MAX_PIXELS: usize = 256 * 1024;
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;
const MAX_PENDING: usize = 4;

#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    source: String,
    display: bool,
    size: u16,
    scale: u16,
}

struct Work {
    key: Key,
    reply: mpsc::SyncSender<Result<egui::ColorImage, String>>,
    ctx: egui::Context,
}

enum State {
    Pending(mpsc::Receiver<Result<egui::ColorImage, String>>),
    Ready(egui::TextureHandle),
    Failed,
}

struct Entry {
    state: State,
    frame: u64,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<Key, Entry>,
    worker: Option<mpsc::SyncSender<Work>>,
    bytes: usize,
}

impl Cache {
    fn texture(&mut self, ctx: &egui::Context, key: Key) -> Option<egui::TextureHandle> {
        let frame = ctx.cumulative_frame_nr();
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.frame = frame;
            if let State::Pending(rx) = &entry.state {
                match rx.try_recv() {
                    Ok(Ok(image)) => {
                        self.bytes += image.pixels.len() * 4;
                        entry.state = State::Ready(ctx.load_texture(
                            "agent-math",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                        entry.state = State::Failed
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            return match &entry.state {
                State::Ready(texture) => Some(texture.clone()),
                _ => None,
            };
        }
        // Reserve the maximum result size before submitting work. A fast worker
        // can drain its input queue while completed images still await the UI;
        // count those replies as pending too, rather than relying on queue capacity.
        let pending = self
            .entries
            .values()
            .filter(|entry| matches!(entry.state, State::Pending(_)))
            .count();
        if pending >= MAX_PENDING {
            return None;
        }
        let reserved = (pending + 1) * MAX_PIXELS * 4;
        // Do not churn textures when a transcript itself exceeds the cache budget.
        // Entries used in this or the previous frame stay resident.
        while self.entries.len() >= MAX_ENTRIES || self.bytes + reserved > MAX_CACHE_BYTES {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, entry)| {
                    entry.frame.saturating_add(1) < frame
                        && !matches!(entry.state, State::Pending(_))
                })
                .min_by_key(|(_, entry)| entry.frame)
                .map(|(key, _)| key.clone())?;
            if let Some(Entry {
                state: State::Ready(texture),
                ..
            }) = self.entries.remove(&oldest)
            {
                self.bytes -= texture.size()[0] * texture.size()[1] * 4;
            }
        }
        if self.worker.is_none() {
            let (tx, rx) = mpsc::sync_channel::<Work>(MAX_PENDING);
            std::thread::Builder::new()
                .name("agent-math".into())
                .spawn(move || {
                    let font = MathFont::stix_two_math();
                    while let Ok(work) = rx.recv() {
                        let result = font
                            .as_ref()
                            .map_err(ToString::to_string)
                            .and_then(|font| raster(&work.key, font));
                        let _ = work.reply.send(result);
                        work.ctx.request_repaint();
                    }
                })
                .ok()?;
            self.worker = Some(tx);
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let work = Work {
            key: key.clone(),
            reply: tx,
            ctx: ctx.clone(),
        };
        if self.worker.as_ref()?.try_send(work).is_ok() {
            self.entries.insert(
                key,
                Entry {
                    state: State::Pending(rx),
                    frame,
                },
            );
        }
        None
    }
}

/// Obtain a cached formula, scheduling bounded native raster work on demand.
pub(super) fn texture(
    ctx: &egui::Context,
    source: &str,
    display: bool,
    size: f32,
) -> Option<(egui::TextureHandle, egui::Vec2)> {
    if source.len() > MAX_SOURCE {
        return None;
    }
    let key = Key {
        source: source.into(),
        display,
        size: (size.clamp(8., 48.) * 64.).round() as u16,
        scale: (ctx.pixels_per_point().clamp(1., 3.) * 64.).ceil() as u16,
    };
    let scale = key.scale as f32 / 64.;
    let cache = ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<Cache>>>(egui::Id::new("agent-math-cache"))
            .clone()
    });
    let texture = cache.lock().ok()?.texture(ctx, key)?;
    let size = texture.size_vec2() / scale;
    Some((texture, size))
}

fn raster(key: &Key, font: &MathFont) -> Result<egui::ColorImage, String> {
    if key.source.len() > MAX_SOURCE {
        return Err("Formula exceeds 4096 bytes".into());
    }
    let ast = latex_rust::parse(&key.source).map_err(|error| error.to_string())?;
    let size = Dim::ratio(i64::from(key.size), 64);
    let tree = latex_rust::layout_with_em_size_pt(
        &ast,
        font,
        if key.display {
            MathStyle::Display
        } else {
            MathStyle::Text
        },
        &size,
    )
    .map_err(|error| error.to_string())?;
    let em = &size * &Dim::ratio(i64::from(key.scale), 64);
    let width = (&tree.width * &em)
        .ceil_to_u32()
        .map_err(|error| error.to_string())?
        .max(1);
    let height = ((&tree.height + &tree.depth) * &em)
        .ceil_to_u32()
        .map_err(|error| error.to_string())?
        .max(1);
    if width > 2048 || height > 512 || u64::from(width) * u64::from(height) > MAX_PIXELS as u64 {
        return Err("Formula exceeds the rendering dimensions".into());
    }
    let options = PngOptions {
        font_size_pt: size,
        dpi: Dim::ratio(72 * i64::from(key.scale), 64),
        color: latex_rust::Color::rgb(255, 255, 255),
        display: key.display,
        ..PngOptions::default()
    };
    let png = latex_rust::render_png(&tree, font, &options).map_err(|error| error.to_string())?;
    let pixels = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?
        .into_rgba8();
    // Apply the current theme at paint time without rerendering cached equations.
    let mut image = egui::ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        pixels.as_raw(),
    );
    for pixel in &mut image.pixels {
        *pixel = egui::Color32::from_white_alpha(pixel.a());
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(source: &str) -> Key {
        Key {
            source: source.into(),
            display: false,
            size: 16 * 64,
            scale: 64,
        }
    }

    #[test]
    fn nearly_full_cache_evicts_old_textures_before_reserving_a_new_result() {
        let ctx = egui::Context::default();
        let (worker, requests) = mpsc::sync_channel(MAX_PENDING);
        let mut cache = Cache {
            worker: Some(worker),
            ..Cache::default()
        };
        let image = egui::ColorImage::filled([512, 496], egui::Color32::WHITE);
        for index in 0..8 {
            cache.bytes += image.pixels.len() * 4;
            cache.entries.insert(
                key(&format!("old-{index}")),
                Entry {
                    state: State::Ready(ctx.load_texture(
                        "old-math",
                        image.clone(),
                        egui::TextureOptions::LINEAR,
                    )),
                    frame: 0,
                },
            );
        }
        assert!(cache.bytes < MAX_CACHE_BYTES);
        assert!(cache.bytes + MAX_PIXELS * 4 > MAX_CACHE_BYTES);
        for _ in 0..3 {
            let _ = ctx.run(Default::default(), |_| {});
        }
        let next = key("x");
        assert!(cache.texture(&ctx, next.clone()).is_none());
        let request = requests
            .try_recv()
            .expect("stale textures must not block new formulas");
        request.reply.send(Ok(image)).unwrap();
        assert!(cache.texture(&ctx, next).is_some());
        assert_eq!(cache.entries.len(), 8);
        assert!(cache.bytes <= MAX_CACHE_BYTES);
    }

    #[test]
    fn completed_results_still_count_towards_the_pending_work_limit() {
        let ctx = egui::Context::default();
        let (worker, requests) = mpsc::sync_channel(MAX_PENDING);
        let mut cache = Cache {
            worker: Some(worker),
            ..Cache::default()
        };
        for index in 0..MAX_PENDING {
            assert!(cache.texture(&ctx, key(&format!("x_{index}"))).is_none());
            requests
                .try_recv()
                .unwrap()
                .reply
                .send(Ok(egui::ColorImage::filled([1, 1], egui::Color32::WHITE)))
                .unwrap();
        }
        assert!(cache.texture(&ctx, key("next")).is_none());
        assert!(requests.try_recv().is_err());
        assert_eq!(cache.entries.len(), MAX_PENDING);
        assert!(cache.texture(&ctx, key("x_0")).is_some());
        assert!(cache.texture(&ctx, key("next")).is_none());
        assert!(requests.try_recv().is_ok());
    }

    #[test]
    fn typesets_equivariance_fractions_scripts_and_matrices() {
        let font = MathFont::stix_two_math().unwrap();
        for source in [
            r"v\mapsto Rv",
            r"3\times3",
            r"W Rv=R Wv \qquad\text{for every }R,v.",
            r"\frac{\alpha_i^2}{\sqrt{x}}",
            r"\begin{pmatrix}a&b\\c&d\end{pmatrix}",
        ] {
            let image = raster(
                &Key {
                    source: source.into(),
                    display: true,
                    size: 16 * 64,
                    scale: 128,
                },
                &font,
            )
            .unwrap();
            assert!(image.pixels.iter().any(|pixel| pixel.a() > 0), "{source}");
        }
    }

    #[test]
    fn invalid_deep_and_oversized_formulas_fail_without_allocating_large_images() {
        let font = MathFont::stix_two_math().unwrap();
        for source in [
            r"\notARealCommand{x}".into(),
            "x".repeat(MAX_SOURCE + 1),
            format!("{}x{}", "{".repeat(100), "}".repeat(100)),
            r"\rule{1000000em}{1000000em}".into(),
        ] {
            assert!(
                raster(
                    &Key {
                        source,
                        display: true,
                        size: 16 * 64,
                        scale: 128
                    },
                    &font
                )
                .is_err()
            );
        }
    }
}
