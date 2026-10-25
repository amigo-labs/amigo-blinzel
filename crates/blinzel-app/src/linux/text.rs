//! Text through cosmic-text (shaping, bidirectional text, system fonts),
//! rasterized to a coverage texture per string and kept in a small cache
//! (the UI shows few strings: labels, messages, menus).

use std::collections::{HashMap, VecDeque};

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache};

use super::gpu::{Gpu, TextKey, TextTex};

/// Strings kept as textures.
const CAPACITY: usize = 64;

#[derive(Default)]
pub struct TextCache {
    fonts: Option<(FontSystem, SwashCache)>,
    map: HashMap<TextKey, TextTex>,
    order: VecDeque<TextKey>,
}

impl TextCache {
    /// The texture for `text` at `px` pixels, wrapped at `max_width`.
    pub fn get(&mut self, gpu: &Gpu, text: &str, px: f32, max_width: f32) -> Option<&TextTex> {
        let key = (text.to_string(), px.to_bits(), max_width.to_bits());
        if !self.map.contains_key(&key) {
            let (coverage, w, h) = self.render(text, px, max_width)?;
            if self.order.len() >= CAPACITY
                && let Some(old) = self.order.pop_front()
            {
                self.map.remove(&old);
            }
            self.map
                .insert(key.clone(), gpu.text_texture(&coverage, w, h));
            self.order.push_back(key.clone());
        }
        self.map.get(&key)
    }

    /// Coverage bitmap of the laid-out text and its size.
    fn render(&mut self, text: &str, px: f32, max_width: f32) -> Option<(Vec<u8>, u32, u32)> {
        let (fonts, swash) = self
            .fonts
            .get_or_insert_with(|| (FontSystem::new(), SwashCache::new()));
        let mut buffer = Buffer::new(fonts, Metrics::new(px, (px * 1.3).ceil()));
        let mut b = buffer.borrow_with(fonts);
        b.set_size(Some(max_width), None);
        b.set_text(
            text,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            None,
        );
        b.shape_until_scroll(false);
        let (mut w, mut h) = (0f32, 0f32);
        for run in b.layout_runs() {
            w = w.max(run.line_w);
            h = h.max(run.line_top + run.line_height);
        }
        let (w, h) = (w.ceil() as u32 + 1, h.ceil() as u32);
        if w <= 1 || h == 0 {
            return None;
        }
        let mut coverage = vec![0u8; (w * h) as usize];
        b.draw(swash, Color::rgb(255, 255, 255), |x, y, gw, gh, c| {
            for yy in y.max(0)..(y + gh as i32).min(h as i32) {
                for xx in x.max(0)..(x + gw as i32).min(w as i32) {
                    let i = (yy as u32 * w + xx as u32) as usize;
                    coverage[i] = coverage[i].max(c.a());
                }
            }
        });
        Some((coverage, w, h))
    }
}
