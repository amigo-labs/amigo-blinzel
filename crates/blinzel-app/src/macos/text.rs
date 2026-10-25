//! Text through Core Text (the system font, shaping, bidirectional text),
//! rasterized into an alpha-only bitmap per string and kept in a small cache
//! (the UI shows few strings: labels, messages, the batch dialog).

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;

use objc2_core_foundation::{
    CFAttributedString, CFBoolean, CFDictionary, CFRange, CFRetained, CFString, CFType, CGPoint,
    CGRect, CGSize,
};
use objc2_core_graphics::{CGBitmapContextCreate, CGImageAlphaInfo, CGPath};
use objc2_core_text::{
    CTFont, CTFontUIFontType, CTFramesetter, kCTFontAttributeName,
    kCTForegroundColorFromContextAttributeName,
};

use super::gpu::{Gpu, TextKey, TextTex};

/// Strings kept as textures.
const CAPACITY: usize = 64;

#[derive(Default)]
pub struct TextCache {
    fonts: HashMap<u32, CFRetained<CTFont>>,
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
                .insert(key.clone(), gpu.text_texture(&coverage, w, h)?);
            self.order.push_back(key.clone());
        }
        self.map.get(&key)
    }

    /// Coverage bitmap of the laid-out text and its size.
    fn render(&mut self, text: &str, px: f32, max_width: f32) -> Option<(Vec<u8>, u32, u32)> {
        if text.is_empty() {
            return None;
        }
        let font = self
            .fonts
            .entry(px.to_bits())
            .or_insert_with(|| {
                // SAFETY: plain font lookup; the system font always exists.
                unsafe {
                    CTFont::new_ui_font_for_language(CTFontUIFontType::System, px as f64, None)
                }
                .expect("system font")
            })
            .clone();
        // SAFETY: the attribute keys are Core Text's constants, with the
        // value types they document (a CTFont, a CFBoolean).
        let attrs = unsafe {
            CFDictionary::<CFString, CFType>::from_slices(
                &[
                    kCTFontAttributeName,
                    kCTForegroundColorFromContextAttributeName,
                ],
                &[font.as_ref(), CFBoolean::new(true).as_ref()],
            )
        };
        let string = CFString::from_str(text);
        // SAFETY: valid string and attributes.
        let attributed =
            unsafe { CFAttributedString::new(None, Some(&string), Some(attrs.as_ref())) }?;
        // SAFETY: a framesetter over a valid attributed string.
        let setter = unsafe { CTFramesetter::with_attributed_string(&attributed) };
        let all = CFRange {
            location: 0,
            length: 0,
        };
        // SAFETY: no frame attributes, no fit range.
        let size = unsafe {
            setter.suggest_frame_size_with_constraints(
                all,
                None,
                CGSize::new(max_width as f64, f64::MAX),
                std::ptr::null_mut(),
            )
        };
        let (w, h) = (size.width.ceil() as u32 + 1, size.height.ceil() as u32);
        if w <= 1 || h == 0 {
            return None;
        }
        let mut coverage = vec![0u8; (w * h) as usize];
        // SAFETY: `coverage` outlives the context and holds `h` rows of `w`
        // bytes; alpha-only contexts take no colour space.
        let ctx = unsafe {
            CGBitmapContextCreate(
                coverage.as_mut_ptr().cast::<c_void>(),
                w as usize,
                h as usize,
                8,
                w as usize,
                None,
                CGImageAlphaInfo::Only.0,
            )
        }?;
        let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64));
        // SAFETY: a rectangle path without transform; the frame draws into
        // the bitmap context created above.
        unsafe {
            let path = CGPath::with_rect(rect, std::ptr::null());
            let frame = setter.frame(all, &path, None);
            frame.draw(&ctx);
        }
        drop(ctx);
        Some((coverage, w, h))
    }
}
