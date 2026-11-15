//! The interface's colours: the dark palette of the shared amigo theme
//! ([`crate::theme`], generated) as [`Rgba`], under the names the widgets
//! use. Every interface colour comes from here; the only other colours in
//! this crate are black and white with alpha, drawn over the picture
//! (`docs/specs/amigo-theme.md`).

use crate::gfx::Rgba;
use crate::theme::DARK;

/// `0xRRGGBB` as an opaque colour.
pub const fn rgb(hex: u32) -> Rgba {
    rgba(hex, 1.0)
}

/// `0xRRGGBB` with the alpha `a`.
pub const fn rgba(hex: u32, a: f32) -> Rgba {
    Rgba(
        ((hex >> 16) & 0xFF) as f32 / 255.0,
        ((hex >> 8) & 0xFF) as f32 / 255.0,
        (hex & 0xFF) as f32 / 255.0,
        a,
    )
}

/// Dims the viewer behind a modal panel.
pub const SCRIM: Rgba = Rgba(0.0, 0.0, 0.0, 0.45);
/// The ground of dialogs and panels (`surface`).
pub const PANEL: Rgba = rgb(DARK.surface);
/// The ground of the popup menu (`surface_raised`).
pub const MENU: Rgba = rgb(DARK.surface_raised);
/// The outline of panels, menus and controls at rest (`border`).
pub const BORDER: Rgba = rgb(DARK.border);
/// The outline of a field and of an unchecked check box (`border_strong`):
/// the boundary a person has to find, 3 : 1 against the surface.
pub const BORDER_STRONG: Rgba = rgb(DARK.border_strong);
/// Primary text (`text`).
pub const FG: Rgba = rgb(DARK.text);
/// Secondary text (`text_secondary`).
pub const DIM: Rgba = rgb(DARK.text_secondary);
/// Disabled and hint text (`text_muted`).
pub const FAINT: Rgba = rgb(DARK.text_muted);
/// A control at rest (`surface_raised`).
pub const CONTROL: Rgba = rgb(DARK.surface_raised);
/// A control under the pointer, and the track of a slider or progress
/// bar (`hover`).
pub const CONTROL_HOVER: Rgba = rgb(DARK.hover);
/// A quiet tile on the window's background: a thumbnail not loaded yet
/// (`surface`).
pub const PLACEHOLDER: Rgba = rgb(DARK.surface);
/// Accent lines and marks (`accent`).
pub const ACCENT: Rgba = rgb(DARK.accent);
/// The tint of a marked thumbnail (`accent`, mostly transparent).
pub const ACCENT_TINT: Rgba = rgba(DARK.accent, 0.22);
/// Accent text, e.g. a link (`accent_ink`).
pub const ACCENT_INK: Rgba = rgb(DARK.accent_ink);
/// The fill of an accent control (`accent_solid`).
pub const ACCENT_SOLID: Rgba = rgb(DARK.accent_solid);
/// Text and glyphs on an accent fill (`on_accent`).
pub const ON_ACCENT: Rgba = rgb(DARK.on_accent);
/// Laid over a primary button under the pointer: lighter.
pub const ACCENT_HOVER: Rgba = Rgba(1.0, 1.0, 1.0, 0.12);
/// The outline of a focused field (`focus`).
pub const FOCUS: Rgba = rgb(DARK.focus);
/// The outline of an invalid field (`danger`).
pub const DANGER: Rgba = rgb(DARK.danger);
/// Error text (`danger_ink`).
pub const DANGER_INK: Rgba = rgb(DARK.danger_ink);

/// The close button under the pointer and pressed. Not a theme colour:
/// window buttons follow the platforms' convention.
pub const CLOSE_HOVER: Rgba = Rgba(0.769, 0.169, 0.110, 1.0);
pub const CLOSE_PRESSED: Rgba = Rgba(0.698, 0.157, 0.102, 1.0);
