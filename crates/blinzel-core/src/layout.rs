//! View layout: the three view modes, zoom around the cursor, panning and
//! the view crop ("cut"). All coordinates of the image are in the *oriented*
//! full-resolution space (after EXIF + user rotation); client coordinates
//! are physical pixels of the window's client area.

use crate::geom::{Affine, Orientation, PixelRect, Rect};

/// How image and window relate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ViewMode {
    /// "Original size": the window adapts to the image at 100 %, limited to
    /// the monitor's work area (then the image is fitted).
    FitWindowToImage,
    /// The image is fitted into the window.
    #[default]
    FitImageToWindow,
    /// 1 image pixel = 1 physical screen pixel, centred, pannable.
    ActualSize,
}

impl ViewMode {
    pub fn key(self) -> &'static str {
        match self {
            ViewMode::FitWindowToImage => "fit-window-to-image",
            ViewMode::FitImageToWindow => "fit-image-to-window",
            ViewMode::ActualSize => "actual-size",
        }
    }

    pub fn from_key(s: &str) -> Option<ViewMode> {
        [
            ViewMode::FitWindowToImage,
            ViewMode::FitImageToWindow,
            ViewMode::ActualSize,
        ]
        .into_iter()
        .find(|m| m.key() == s)
    }
}

/// Persistent view preferences.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewOptions {
    pub mode: ViewMode,
    /// In the fitting modes, never enlarge images smaller than the window.
    pub no_upscale: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        ViewOptions {
            mode: ViewMode::FitImageToWindow,
            no_upscale: true,
        }
    }
}

pub const MIN_ZOOM: f64 = 0.01;
pub const MAX_ZOOM: f64 = 64.0;

/// Zoom levels for keyboard zoom.
pub const ZOOM_STEPS: [f64; 21] = [
    0.05, 0.1, 0.125, 0.167, 0.25, 0.333, 0.5, 0.667, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0,
    12.0, 16.0, 24.0, 32.0, 64.0,
];

/// Per-image view state (reset when the image changes).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewState {
    /// User zoom; `None` = the mode decides.
    pub zoom: Option<f64>,
    /// Point of the content shown at the client centre (content
    /// coordinates, i.e. relative to the crop); `None` = centred.
    pub center: Option<(f64, f64)>,
    /// View crop in oriented full-resolution coordinates ("cut").
    pub crop: Option<PixelRect>,
}

/// Result of laying out an image in a client area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// Client pixels per image pixel.
    pub scale: f64,
    /// Client position of the content's top-left corner, snapped to whole
    /// pixels: at 100 % every texel lands on exactly one pixel (no
    /// half-pixel blur), and pixel centres never fall on the content edge.
    pub origin: (f64, f64),
    /// The origin before snapping. Pan and zoom work on it, so the rounding
    /// never accumulates.
    exact_origin: (f64, f64),
    /// Content (crop) size in image pixels.
    pub content: (f64, f64),
    /// Top-left of the content in oriented image coordinates.
    pub crop_origin: (f64, f64),
    pub client: (f64, f64),
    /// The scaled content exceeds the client area in some direction.
    pub overflows: bool,
}

impl Layout {
    /// Oriented image coordinates → client coordinates.
    pub fn image_to_client(&self) -> Affine {
        Affine::translate(-self.crop_origin.0, -self.crop_origin.1)
            .then(&Affine::scale(self.scale, self.scale))
            .then(&Affine::translate(self.origin.0, self.origin.1))
    }

    pub fn client_to_image(&self, p: (f64, f64)) -> (f64, f64) {
        (
            (p.0 - self.origin.0) / self.scale + self.crop_origin.0,
            (p.1 - self.origin.1) / self.scale + self.crop_origin.1,
        )
    }

    /// Where the content lands in the client area.
    pub fn content_rect(&self) -> Rect {
        Rect::new(
            self.origin.0,
            self.origin.1,
            self.content.0 * self.scale,
            self.content.1 * self.scale,
        )
    }

    /// Visible part of the content in oriented image coordinates.
    pub fn visible_image_rect(&self) -> Option<Rect> {
        let tl = self.client_to_image((0.0, 0.0));
        let br = self.client_to_image(self.client);
        let crop = Rect::new(
            self.crop_origin.0,
            self.crop_origin.1,
            self.content.0,
            self.content.1,
        );
        Rect::from_points(tl, br).intersect(&crop)
    }
}

/// The scale a view mode chooses without user zoom.
pub fn mode_scale(content: (f64, f64), client: (f64, f64), opts: &ViewOptions) -> f64 {
    if content.0 <= 0.0 || content.1 <= 0.0 || client.0 <= 0.0 || client.1 <= 0.0 {
        return 1.0;
    }
    let fit = (client.0 / content.0).min(client.1 / content.1);
    match opts.mode {
        ViewMode::ActualSize => 1.0,
        // The window already has the image size; only too large images
        // (limited by the work area) are fitted.
        ViewMode::FitWindowToImage => fit.min(1.0),
        ViewMode::FitImageToWindow => {
            if opts.no_upscale {
                fit.min(1.0)
            } else {
                fit
            }
        }
    }
}

/// Lays out an image of oriented size `image` in `client`.
pub fn compute(
    image: (u32, u32),
    client: (f64, f64),
    opts: &ViewOptions,
    state: &ViewState,
) -> Layout {
    let crop = state
        .crop
        .filter(|c| c.right() <= image.0 && c.bottom() <= image.1 && c.w > 0 && c.h > 0)
        .unwrap_or(PixelRect::new(0, 0, image.0, image.1));
    let content = (crop.w as f64, crop.h as f64);
    let scale = state
        .zoom
        .unwrap_or_else(|| mode_scale(content, client, opts))
        .clamp(MIN_ZOOM, MAX_ZOOM);
    let scaled = (content.0 * scale, content.1 * scale);
    let center = state.center.unwrap_or((content.0 / 2.0, content.1 / 2.0));
    let axis = |scaled: f64, client: f64, c: f64, len: f64| {
        if scaled <= client {
            (client - scaled) / 2.0
        } else {
            // Keep the content covering the client area.
            let half = client / 2.0 / scale;
            let c = c.clamp(half, len - half);
            client / 2.0 - c * scale
        }
    };
    let exact_origin = (
        axis(scaled.0, client.0, center.0, content.0),
        axis(scaled.1, client.1, center.1, content.1),
    );
    Layout {
        scale,
        origin: (exact_origin.0.round(), exact_origin.1.round()),
        exact_origin,
        content,
        crop_origin: (crop.x as f64, crop.y as f64),
        client,
        overflows: scaled.0 > client.0 + 0.5 || scaled.1 > client.1 + 0.5,
    }
}

/// Zooms by `factor` keeping the image point under `cursor` fixed.
pub fn zoom_at(state: &mut ViewState, layout: &Layout, cursor: (f64, f64), new_scale: f64) {
    let new_scale = new_scale.clamp(MIN_ZOOM, MAX_ZOOM);
    let p = (
        (cursor.0 - layout.exact_origin.0) / layout.scale,
        (cursor.1 - layout.exact_origin.1) / layout.scale,
    );
    // Content point that must sit at the client centre so `p` stays put.
    let cx = p.0 - (cursor.0 - layout.client.0 / 2.0) / new_scale;
    let cy = p.1 - (cursor.1 - layout.client.1 / 2.0) / new_scale;
    state.zoom = Some(new_scale);
    state.center = Some((cx, cy));
}

/// Next keyboard zoom step above (`dir > 0`) or below the current scale.
pub fn zoom_step(current: f64, dir: i32) -> f64 {
    if dir > 0 {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|&z| z > current * 1.001)
            .unwrap_or(MAX_ZOOM)
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|&z| z < current / 1.001)
            .unwrap_or(MIN_ZOOM)
    }
}

/// Moves the view by a client-pixel delta (content follows the mouse).
pub fn pan(state: &mut ViewState, layout: &Layout, delta: (f64, f64)) {
    let current = (
        (layout.client.0 / 2.0 - layout.exact_origin.0) / layout.scale,
        (layout.client.1 / 2.0 - layout.exact_origin.1) / layout.scale,
    );
    state.zoom.get_or_insert(layout.scale);
    state.center = Some((
        current.0 - delta.0 / layout.scale,
        current.1 - delta.1 / layout.scale,
    ));
}

/// Client size for [`ViewMode::FitWindowToImage`]: the image at 100 %,
/// shrunk (aspect kept) to fit `max`, at least `min`.
pub fn window_client_size(image: (u32, u32), max: (u32, u32), min: (u32, u32)) -> (u32, u32) {
    let (w, h) = (image.0.max(1) as f64, image.1.max(1) as f64);
    let s = (max.0 as f64 / w).min(max.1 as f64 / h).min(1.0);
    let cw = ((w * s).round() as u32).clamp(min.0.min(max.0), max.0);
    let ch = ((h * s).round() as u32).clamp(min.1.min(max.1), max.1);
    (cw, ch)
}

/// Stage texel coordinates → client pixels, for an image stage of size
/// `stage` (texels) of a source whose stored full size is `full`, shown
/// with `orientation` under `layout`. Shared by the GPU renderer and the
/// CPU reference renderer.
pub fn stage_to_client(
    layout: &Layout,
    orientation: Orientation,
    full: (u32, u32),
    stage: (u32, u32),
) -> Affine {
    let to_full = Affine::scale(
        full.0 as f64 / stage.0.max(1) as f64,
        full.1 as f64 / stage.1.max(1) as f64,
    );
    to_full
        .then(&orientation.transform(full.0 as f64, full.1 as f64))
        .then(&layout.image_to_client())
}

/// The visible content (crop) in stage texel coordinates.
pub fn content_in_stage(
    layout: &Layout,
    orientation: Orientation,
    full: (u32, u32),
    stage: (u32, u32),
) -> Rect {
    let crop = Rect::new(
        layout.crop_origin.0,
        layout.crop_origin.1,
        layout.content.0,
        layout.content.1,
    );
    let to_oriented = Affine::scale(
        full.0 as f64 / stage.0.max(1) as f64,
        full.1 as f64 / stage.1.max(1) as f64,
    )
    .then(&orientation.transform(full.0 as f64, full.1 as f64));
    match to_oriented.inverse() {
        Some(inv) => inv.map_rect(&crop),
        None => Rect::new(0.0, 0.0, stage.0 as f64, stage.1 as f64),
    }
}

/// Stage texels per client pixel (the sampling footprint).
pub fn texels_per_pixel(stage_to_client: &Affine) -> f64 {
    1.0 / stage_to_client.determinant().abs().sqrt().max(1e-9)
}

/// Image size (stored orientation) worth decoding for display at `scale`,
/// so DCT-scaled JPEG decoding produces at least one texel per pixel.
pub fn needed_decode_size(stored: (u32, u32), scale: f64) -> (u32, u32) {
    let s = scale.min(1.0);
    // The epsilon keeps 6000 × 0.26 from rounding up to 1561.
    let dim = |d: u32| ((d as f64 * s - 1e-6).ceil() as u32).clamp(1, d.max(1));
    (dim(stored.0), dim(stored.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn fit_image_to_window() {
        let opts = ViewOptions {
            mode: ViewMode::FitImageToWindow,
            no_upscale: true,
        };
        let l = compute((4000, 3000), (800.0, 800.0), &opts, &ViewState::default());
        assert!(close(l.scale, 0.2));
        assert_eq!(l.origin, (0.0, 100.0));
        assert!(!l.overflows);
        // Small images stay at 100 % unless upscaling is allowed.
        let l = compute((200, 100), (800.0, 800.0), &opts, &ViewState::default());
        assert!(close(l.scale, 1.0));
        assert_eq!(l.origin, (300.0, 350.0));
        let up = ViewOptions {
            no_upscale: false,
            ..opts
        };
        assert!(close(
            compute((200, 100), (800.0, 800.0), &up, &ViewState::default()).scale,
            4.0
        ));
    }

    #[test]
    fn actual_size_overflows_and_clamps_pan() {
        let opts = ViewOptions {
            mode: ViewMode::ActualSize,
            no_upscale: true,
        };
        let l = compute((4000, 3000), (1000.0, 500.0), &opts, &ViewState::default());
        assert!(close(l.scale, 1.0) && l.overflows);
        assert_eq!(l.origin, (-1500.0, -1250.0));
        // Panning beyond the edge stops at the edge.
        let st = ViewState {
            center: Some((0.0, 0.0)),
            ..Default::default()
        };
        let l = compute((4000, 3000), (1000.0, 500.0), &opts, &st);
        assert_eq!(l.origin, (0.0, 0.0));
        let mut st = ViewState::default();
        let l0 = compute((4000, 3000), (1000.0, 500.0), &opts, &st);
        pan(&mut st, &l0, (100.0, -50.0));
        let l1 = compute((4000, 3000), (1000.0, 500.0), &opts, &st);
        assert_eq!(
            (l1.origin.0 - l0.origin.0, l1.origin.1 - l0.origin.1),
            (100.0, -50.0)
        );
    }

    #[test]
    fn zoom_keeps_point_under_cursor() {
        let opts = ViewOptions::default();
        let mut st = ViewState::default();
        let l = compute((4000, 3000), (800.0, 600.0), &opts, &st);
        let cursor = (600.0, 150.0);
        let before = l.client_to_image(cursor);
        zoom_at(&mut st, &l, cursor, l.scale * 4.0);
        let l2 = compute((4000, 3000), (800.0, 600.0), &opts, &st);
        let after = l2.client_to_image(cursor);
        // Exact up to the snapping of the origin: half a client pixel.
        let tol = 0.5 / l2.scale + 1e-9;
        assert!(
            (before.0 - after.0).abs() <= tol && (before.1 - after.1).abs() <= tol,
            "{before:?} {after:?}"
        );
        assert!(close(l2.scale, 0.8));
    }

    #[test]
    fn origin_snaps_to_whole_pixels_without_drift() {
        let opts = ViewOptions {
            mode: ViewMode::ActualSize,
            no_upscale: true,
        };
        // Centring 133 × 77 in 140 × 90 would put it at (3.5, 6.5).
        let l = compute((133, 77), (140.0, 90.0), &opts, &ViewState::default());
        assert_eq!(l.origin, (4.0, 7.0));
        // Sub-pixel pans add up instead of being rounded away.
        let mut st = ViewState::default();
        let l0 = compute((4000, 3000), (1000.0, 500.0), &opts, &st);
        for _ in 0..4 {
            let l = compute((4000, 3000), (1000.0, 500.0), &opts, &st);
            pan(&mut st, &l, (0.25, 0.0));
        }
        let l1 = compute((4000, 3000), (1000.0, 500.0), &opts, &st);
        assert_eq!(l1.origin.0 - l0.origin.0, 1.0);
    }

    #[test]
    fn crop_restricts_content() {
        let opts = ViewOptions {
            mode: ViewMode::ActualSize,
            no_upscale: true,
        };
        let st = ViewState {
            crop: Some(PixelRect::new(100, 50, 200, 100)),
            ..Default::default()
        };
        let l = compute((1000, 1000), (400.0, 400.0), &opts, &st);
        assert_eq!(l.content, (200.0, 100.0));
        assert_eq!(l.origin, (100.0, 150.0));
        assert_eq!(l.image_to_client().apply((100.0, 50.0)), (100.0, 150.0));
        assert_eq!(
            l.visible_image_rect(),
            Some(Rect::new(100.0, 50.0, 200.0, 100.0))
        );
        // An invalid crop is ignored.
        let bad = ViewState {
            crop: Some(PixelRect::new(900, 900, 200, 200)),
            ..Default::default()
        };
        assert_eq!(
            compute((1000, 1000), (400.0, 400.0), &opts, &bad).content,
            (1000.0, 1000.0)
        );
    }

    #[test]
    fn window_sizing_and_decode_size() {
        assert_eq!(
            window_client_size((800, 600), (1920, 1040), (320, 200)),
            (800, 600)
        );
        assert_eq!(
            window_client_size((6000, 4000), (1920, 1040), (320, 200)),
            (1560, 1040)
        );
        assert_eq!(
            window_client_size((50, 20), (1920, 1040), (320, 200)),
            (320, 200)
        );
        assert_eq!(needed_decode_size((6000, 4000), 0.26), (1560, 1040));
        assert_eq!(needed_decode_size((6000, 4000), 2.0), (6000, 4000));
    }

    #[test]
    fn zoom_steps_and_modes() {
        assert_eq!(zoom_step(1.0, 1), 1.5);
        assert_eq!(zoom_step(1.0, -1), 0.75);
        assert_eq!(zoom_step(0.3, 1), 0.333);
        assert_eq!(zoom_step(64.0, 1), MAX_ZOOM);
        for m in [
            ViewMode::FitWindowToImage,
            ViewMode::FitImageToWindow,
            ViewMode::ActualSize,
        ] {
            assert_eq!(ViewMode::from_key(m.key()), Some(m));
        }
    }
}
