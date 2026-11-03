//! Screenshots on macOS: ⌘⇧1 (in resident mode) or the menu bar icon
//! freezes every display through ScreenCaptureKit and covers each with a
//! borderless panel showing its picture, where [`blinzel_ui::snip`] picks
//! a region, a window or the display. That part goes to the clipboard,
//! and as the settings say into the screenshot folder, the viewer and the
//! editor ([`blinzel_core::screenshot`]).
//!
//! A selection stays on one display, as with macOS' own screenshots: the
//! displays may differ in scale, so their pictures do not share pixels.

use std::cell::{Cell, RefCell};
use std::ffi::{OsString, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use blinzel_core::geom::{Orientation, Rect};
use blinzel_core::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};
use blinzel_core::screenshot as core_shot;
use blinzel_core::settings::Settings;
use blinzel_i18n::{Lang, Msg, fill, tr, tr_in, trf};
use blinzel_ui::backend::{Canvas, Device};
use blinzel_ui::gfx::Rgba;
use blinzel_ui::snip::{self, Outcome, Snip};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{AllocAnyThread, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSBackingStoreType, NSCursor, NSEvent,
    NSEventModifierFlags, NSScreen, NSScreenSaverWindowLevel, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowLevel,
    NSWindowStyleMask,
};
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGPreflightScreenCaptureAccess, CGRectMakeWithDictionaryRepresentation,
    CGRequestScreenCaptureAccess, CGWindowListCopyWindowInfo, CGWindowListOption,
    kCGColorSpaceSRGB, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDate, NSDateFormatter, NSError, NSObjectProtocol, NSPoint, NSRect,
    NSSize, NSString, ns_string,
};
use objc2_quartz_core::CAMetalLayer;
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
};

use super::gpu::{Gpu, MetalCanvas, MetalDevice, MetalImage, Target};
use super::shell::{UserEvent, key_of, log, post, screen_id};

/// System Settings, Privacy & Security, Screen Recording.
const PRIVACY_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

/// A display as AppKit sees it, taken on the main thread.
#[derive(Clone, Copy, Debug)]
pub struct Display {
    id: u32,
    /// Global Quartz coordinates in points: origin at the top left of the
    /// main display, y down.
    frame: Rect,
    /// AppKit's frame (origin at the bottom left of the main display).
    cocoa: Rect,
    /// Pixels per point.
    scale: f64,
}

impl Display {
    fn pixels(&self) -> (u32, u32) {
        (
            (self.frame.w * self.scale).round().max(1.0) as u32,
            (self.frame.h * self.scale).round().max(1.0) as u32,
        )
    }
}

/// One display's frozen picture.
#[derive(Debug)]
pub struct Frozen {
    display: Display,
    bgra: Vec<u8>,
    size: (u32, u32),
    /// Visible windows front to back, in the picture's pixels.
    windows: Vec<Rect>,
}

impl Frozen {
    /// Part `r` (whole pixels inside the picture) as BGRA.
    fn crop(&self, r: Rect) -> (Vec<u8>, u32, u32) {
        let (x, y) = (r.x.max(0.0) as usize, r.y.max(0.0) as usize);
        let w = (r.w as usize).min(self.size.0 as usize - x);
        let h = (r.h as usize).min(self.size.1 as usize - y);
        let stride = self.size.0 as usize * 4;
        let mut out = Vec::with_capacity(w * h * 4);
        for row in y..y + h {
            out.extend_from_slice(&self.bgra[row * stride + x * 4..row * stride + (x + w) * 4]);
        }
        (out, w as u32, h as u32)
    }
}

/// A finished screenshot: the PNG for the clipboard and the file it went
/// into (none when the settings ask for none).
#[derive(Debug)]
pub struct Taken {
    pub png: Vec<u8>,
    pub file: Result<Option<PathBuf>, String>,
}

/// A Quartz rect (global points, y down) in the pixels of a display whose
/// Quartz frame is `display` at `scale`.
pub fn to_display_pixels(r: Rect, display: Rect, scale: f64) -> Rect {
    Rect::new(
        (r.x - display.x) * scale,
        (r.y - display.y) * scale,
        r.w * scale,
        r.h * scale,
    )
}

/// An AppKit frame (y up from the bottom of the main display, which is
/// `main_height` points tall) as a Quartz rect (y down from its top).
pub fn cocoa_to_quartz(r: Rect, main_height: f64) -> Rect {
    Rect::new(r.x, main_height - r.y - r.h, r.w, r.h)
}

fn rect(r: NSRect) -> Rect {
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

/// Every display, in AppKit's order (the main display first).
fn displays(mtm: MainThreadMarker) -> Vec<Display> {
    let screens = NSScreen::screens(mtm);
    let main_height = screens.firstObject().map_or(0.0, |s| s.frame().size.height);
    screens
        .iter()
        .filter_map(|s| {
            let cocoa = rect(s.frame());
            Some(Display {
                id: screen_id(&s)? as u32,
                frame: cocoa_to_quartz(cocoa, main_height),
                cocoa,
                scale: s.backingScaleFactor(),
            })
        })
        .collect()
}

/// The pointer in global Quartz points.
fn pointer(mtm: MainThreadMarker) -> (f64, f64) {
    let main_height = NSScreen::screens(mtm)
        .firstObject()
        .map_or(0.0, |s| s.frame().size.height);
    let p = NSEvent::mouseLocation();
    (p.x, main_height - p.y)
}

fn describe(err: *mut NSError) -> String {
    // SAFETY: ScreenCaptureKit passes a valid error or null.
    match unsafe { err.as_ref() } {
        Some(e) => e.localizedDescription().to_string(),
        None => "unknown error".into(),
    }
}

/// The displays this process may capture.
fn shareable_content() -> Result<Retained<SCShareableContent>, String> {
    let (tx, rx) = mpsc::channel();
    let done = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        // SAFETY: a valid object or null.
        let r = unsafe { Retained::retain(content) }.ok_or_else(|| describe(err));
        let _ = tx.send(r);
    });
    // SAFETY: ScreenCaptureKit copies the block.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&done) };
    rx.recv_timeout(Duration::from_secs(10))
        .map_err(|_| "ScreenCaptureKit did not answer".to_string())?
}

/// `image` as opaque BGRA in sRGB.
fn to_bgra(image: &CGImage, (w, h): (u32, u32)) -> Result<Vec<u8>, String> {
    // SAFETY: Core Graphics' constant name.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .ok_or("no sRGB colour space")?;
    let mut plane = Plane::new(w, h, 4);
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    {
        // SAFETY: `plane` holds `h` rows of `stride` bytes and outlives the
        // context.
        let ctx: CFRetained<CGContext> = unsafe {
            CGBitmapContextCreate(
                plane.data.as_mut_ptr().cast::<c_void>(),
                w as usize,
                h as usize,
                8,
                plane.stride,
                Some(&space),
                info,
            )
        }
        .ok_or("no bitmap context")?;
        let r = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64));
        CGContext::draw_image(Some(&ctx), r, Some(image));
    }
    // Rows without padding, alpha set (the screen is opaque).
    let row = w as usize * 4;
    let mut out = Vec::with_capacity(row * h as usize);
    for y in 0..h as usize {
        out.extend_from_slice(&plane.data[y * plane.stride..y * plane.stride + row]);
    }
    for p in out.as_chunks_mut::<4>().0 {
        p[3] = 255;
    }
    Ok(out)
}

/// The visible windows front to back, in global Quartz points.
fn windows() -> Vec<Rect> {
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else {
        return Vec::new();
    };
    // SAFETY: the window list holds dictionaries.
    let list: &CFArray<CFDictionary<CFString, CFType>> = unsafe { list.cast_unchecked() };
    list.iter()
        .filter_map(|d| {
            // SAFETY: Core Graphics' constant names.
            let (layer_key, bounds_key) = unsafe { (kCGWindowLayer, kCGWindowBounds) };
            let layer = d.get(layer_key)?.downcast::<CFNumber>().ok()?.as_i64()?;
            // Normal windows only: no menu bar, Dock or overlays.
            if layer != 0 {
                return None;
            }
            let bounds = d.get(bounds_key)?.downcast::<CFDictionary>().ok()?;
            let mut r = CGRect::default();
            // SAFETY: `r` is a valid out pointer.
            unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &mut r) }
                .then_some(())?;
            (r.size.width >= 1.0 && r.size.height >= 1.0).then(|| rect(r))
        })
        .collect()
}

/// Freezes each of `displays`. Runs off the main thread: ScreenCaptureKit
/// answers on its own queue.
fn grab(displays: &[Display]) -> Result<Vec<Frozen>, String> {
    let content = shareable_content()?;
    let windows = windows();
    // SAFETY: plain property reads.
    let sc_displays = unsafe { content.displays() };
    let mut out = Vec::new();
    for d in displays {
        let Some(sc) = sc_displays
            .iter()
            .find(|s| unsafe { s.displayID() } == d.id)
        else {
            continue;
        };
        let size = d.pixels();
        let (tx, rx) = mpsc::channel();
        let done = RcBlock::new(move |image: *mut CGImage, err: *mut NSError| {
            let r = std::ptr::NonNull::new(image)
                // SAFETY: a valid image; retained for the receiver.
                .map(|p| unsafe { CFRetained::retain(p) })
                .ok_or_else(|| describe(err))
                .and_then(|img| to_bgra(&img, size));
            let _ = tx.send(r);
        });
        // SAFETY: ScreenCaptureKit's documented calls; it copies the block.
        unsafe {
            let filter = SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &sc,
                &NSArray::new(),
            );
            let config = SCStreamConfiguration::new();
            config.setWidth(size.0 as usize);
            config.setHeight(size.1 as usize);
            config.setShowsCursor(false);
            SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                &filter,
                &config,
                Some(&done),
            );
        }
        let bgra = rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "ScreenCaptureKit did not answer".to_string())??;
        out.push(Frozen {
            display: *d,
            bgra,
            size,
            windows: windows
                .iter()
                .map(|w| to_display_pixels(*w, d.frame, d.scale))
                .collect(),
        });
    }
    if out.is_empty() {
        return Err("no display".into());
    }
    Ok(out)
}

// -------------------------------------------------------------- permission

thread_local! {
    /// macOS asked once in this process; later attempts explain instead.
    static ASKED: Cell<bool> = const { Cell::new(false) };
}

/// Whether screen recording is allowed. Asks macOS the first time; after
/// that explains where to allow it (a change needs a restart of the app).
fn allowed(mtm: MainThreadMarker) -> bool {
    if CGPreflightScreenCaptureAccess() {
        return true;
    }
    if !ASKED.with(|a| a.replace(true)) {
        // macOS shows its own prompt.
        CGRequestScreenCaptureAccess();
        return false;
    }
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(tr(Msg::ScreenRecordingTitle)));
    alert.setInformativeText(&NSString::from_str(tr(Msg::ScreenRecordingBody)));
    alert.addButtonWithTitle(&NSString::from_str(tr(Msg::OpenSystemSettings)));
    alert.addButtonWithTitle(&NSString::from_str(tr(Msg::SetupCancel)));
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    if alert.runModal() == NSAlertFirstButtonReturn {
        let _ = std::process::Command::new("/usr/bin/open")
            .arg(PRIVACY_PANE)
            .spawn();
    }
    false
}

// ------------------------------------------------------------------- start

/// Takes a screenshot: the display under the pointer at once, or the
/// overlay to pick a part. The pictures arrive as [`UserEvent::Frozen`].
pub fn start(mtm: MainThreadMarker, whole_screen: bool) {
    if PANELS.with(|p| !p.borrow().is_empty()) || !allowed(mtm) {
        return;
    }
    let displays = displays(mtm);
    let _ = std::thread::Builder::new()
        .name("blinzel-screenshot".into())
        .spawn(move || post(UserEvent::Frozen(grab(&displays), whole_screen)));
}

/// The frozen displays are here: saves the one under the pointer, or opens
/// the overlay. Returns an error for the notice.
pub fn frozen(
    mtm: MainThreadMarker,
    gpu: &Arc<Gpu>,
    device: &MetalDevice,
    shots: Vec<Frozen>,
    whole_screen: bool,
    settings: &Settings,
    hidden: bool,
) -> Result<(), String> {
    let p = pointer(mtm);
    if whole_screen {
        let frames: Vec<Rect> = shots.iter().map(|f| f.display.frame).collect();
        let f = &shots[snip::monitor_under(&frames, p).unwrap_or(0)];
        let (bgra, w, h) = f.crop(Rect::new(0.0, 0.0, f.size.0 as f64, f.size.1 as f64));
        save(bgra, w, h, settings);
        return Ok(());
    }
    close();
    let mut key = None;
    let mut panels = Vec::with_capacity(shots.len());
    for (index, f) in shots.into_iter().enumerate() {
        let cursor = match cursor_on(&f.display, p) {
            Some(c) => {
                key = Some(index);
                c
            }
            None => (-1.0, -1.0),
        };
        // Not shown yet: a failure drops the panels made so far.
        panels.push(open_panel(mtm, gpu, device, f, index, cursor)?);
    }
    GIVE_BACK.with(|g| g.set(hidden));
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    for (i, p) in panels.iter().enumerate() {
        if Some(i) == key {
            p.window.makeKeyAndOrderFront(None);
        } else {
            p.window.orderFrontRegardless();
        }
    }
    PANELS.with(|ps| *ps.borrow_mut() = panels);
    NSCursor::crosshairCursor().set();
    Ok(())
}

/// The pointer `p` (global Quartz points) in the pixels of `d`, if it is
/// on that display.
fn cursor_on(d: &Display, p: (f64, f64)) -> Option<(f64, f64)> {
    d.frame.contains(p).then(|| {
        let local = to_display_pixels(Rect::new(p.0, p.1, 0.0, 0.0), d.frame, d.scale);
        (local.x, local.y)
    })
}

// ----------------------------------------------------------------- overlay

struct Panel {
    window: Retained<SnipWindow>,
    canvas: MetalCanvas,
    picture: MetalImage,
    snip: Snip,
    frozen: Frozen,
}

thread_local! {
    static PANELS: RefCell<Vec<Panel>> = const { RefCell::new(Vec::new()) };
    /// The overlay opened with the viewer hidden.
    static GIVE_BACK: Cell<bool> = const { Cell::new(false) };
}

fn open_panel(
    mtm: MainThreadMarker,
    gpu: &Arc<Gpu>,
    device: &MetalDevice,
    frozen: Frozen,
    index: usize,
    cursor: (f64, f64),
) -> Result<Panel, String> {
    let (w, h) = frozen.size;
    let mut plane = Plane::new(w, h, 4);
    plane.data.copy_from_slice(&frozen.bgra);
    let image = Image {
        pixels: Pixels::Packed {
            plane,
            format: PackedFormat::Bgra8,
            alpha: Alpha::Opaque,
        },
        width: w,
        height: h,
        full_width: w,
        full_height: h,
        region: None,
    };
    let picture = device.upload(&image, Orientation::NORMAL)?;
    drop(image);
    let c = frozen.display.cocoa;
    let frame = NSRect::new(NSPoint::new(c.x, c.y), NSSize::new(c.w, c.h));
    let (style, level, behavior) = panel_style();
    let window = SnipWindow::new(mtm, frame, style);
    window.setLevel(level);
    window.setOpaque(true);
    window.setHasShadow(false);
    window.setCollectionBehavior(behavior);
    // SAFETY: the panel list keeps the window.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setAcceptsMouseMovedEvents(true);
    let bounds = NSRect::new(NSPoint::new(0.0, 0.0), frame.size);
    let view = SnipView::new(mtm, bounds, index);
    let layer = CAMetalLayer::new();
    layer.setContentsScale(frozen.display.scale);
    view.setLayer(Some(&layer));
    view.setWantsLayer(true);
    // Pointer moves reach every panel, not only the key one.
    // SAFETY: the view owns the area; no user info.
    let area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            bounds,
            NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::InVisibleRect,
            Some(&view),
            None,
        )
    };
    view.addTrackingArea(&area);
    window.setContentView(Some(&view));
    window.makeFirstResponder(Some(&view));
    let canvas = MetalCanvas::new(gpu.clone(), Target::Layer(layer), (w, h));
    let snip = snip_of(&frozen, cursor);
    let mut panel = Panel {
        window,
        canvas,
        picture,
        snip,
        frozen,
    };
    render(&mut panel);
    Ok(panel)
}

/// How each display's panel sits: borderless at screen-saver level (above
/// the menu bar and the Dock), on every Space and over full-screen apps,
/// outside the window cycle.
fn panel_style() -> (NSWindowStyleMask, NSWindowLevel, NSWindowCollectionBehavior) {
    (
        NSWindowStyleMask::Borderless,
        NSScreenSaverWindowLevel,
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    )
}

/// The overlay of one frozen display: its picture is the only monitor, so
/// a selection stays on that display.
fn snip_of(f: &Frozen, cursor: (f64, f64)) -> Snip {
    let (w, h) = f.size;
    Snip::new(
        (w, h),
        vec![Rect::new(0.0, 0.0, w as f64, h as f64)],
        f.windows.clone(),
        cursor,
    )
}

fn render(p: &mut Panel) {
    if p.canvas.begin(Rgba(0.0, 0.0, 0.0, 1.0)) {
        p.snip
            .draw(&mut p.canvas, &p.picture, p.frozen.display.scale);
        p.canvas.present();
    }
}

/// Closes the overlay, if open.
pub fn close() {
    let panels = PANELS
        .with(|p| p.try_borrow_mut().map(|mut p| std::mem::take(&mut *p)))
        .unwrap_or_default();
    if panels.is_empty() {
        return;
    }
    for p in &panels {
        p.window.orderOut(None);
    }
    NSCursor::arrowCursor().set();
    // Without a viewer window the app that was in front gets the focus
    // back.
    if GIVE_BACK.with(|g| g.replace(false))
        && let Some(mtm) = MainThreadMarker::new()
    {
        NSApplication::sharedApplication(mtm).deactivate();
    }
}

/// Feeds panel `index`, then acts on the outcome once the panel list is
/// free again.
fn feed(index: usize, f: impl FnOnce(&mut Snip) -> Outcome) {
    let outcome = PANELS.with(|ps| {
        let mut ps = ps.try_borrow_mut().ok()?;
        let p = ps.get_mut(index)?;
        let o = f(&mut p.snip);
        if o == Outcome::Redraw {
            render(p);
        }
        Some(o)
    });
    match outcome {
        Some(Outcome::Cancel) => close(),
        Some(Outcome::Done(r)) => {
            let part = PANELS.with(|ps| ps.borrow().get(index).map(|p| p.frozen.crop(r)));
            close();
            if let Some((bgra, w, h)) = part {
                post(UserEvent::Picked(bgra, w, h));
            }
        }
        _ => {}
    }
}

/// A point of `event` in the pixels of panel `index`'s picture.
fn point(view: &NSView, event: &NSEvent, scale: f64) -> (f64, f64) {
    let p = view.convertPoint_fromView(event.locationInWindow(), None);
    (p.x * scale, p.y * scale)
}

fn scale_of(index: usize) -> f64 {
    PANELS.with(|ps| {
        ps.try_borrow()
            .ok()
            .and_then(|ps| ps.get(index).map(|p| p.frozen.display.scale))
            .unwrap_or(1.0)
    })
}

define_class!(
    // SAFETY: NSWindow has no subclassing requirements; no Drop.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "BlinzelSnipWindow"]
    struct SnipWindow;

    // SAFETY: no requirements.
    unsafe impl NSObjectProtocol for SnipWindow {}

    impl SnipWindow {
        /// Borderless windows take no keys by default.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }
    }
);

impl SnipWindow {
    fn new(mtm: MainThreadMarker, frame: NSRect, style: NSWindowStyleMask) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSWindow's designated initializer.
        unsafe {
            msg_send![super(this), initWithContentRect: frame, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false]
        }
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; no Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "BlinzelSnipView"]
    #[ivars = usize]
    struct SnipView;

    // SAFETY: no requirements.
    unsafe impl NSObjectProtocol for SnipView {}

    // SAFETY: the signatures match AppKit's.
    impl SnipView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _e: &NSEvent) {
            // Keys go to the display the pointer is on.
            if let Some(w) = self.window() {
                w.makeKeyWindow();
            }
            NSCursor::crosshairCursor().set();
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, e: &NSEvent) {
            self.moved(e);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, e: &NSEvent) {
            self.moved(e);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            let i = *self.ivars();
            let p = point(self, e, scale_of(i));
            feed(i, |s| s.mouse_down(p));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, e: &NSEvent) {
            let i = *self.ivars();
            let p = point(self, e, scale_of(i));
            feed(i, |s| s.mouse_up(p));
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, _e: &NSEvent) {
            close();
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, e: &NSEvent) {
            let chars = e
                .charactersIgnoringModifiers()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let shift = e.modifierFlags().contains(NSEventModifierFlags::Shift);
            if let Some(k) = key_of(e.keyCode(), &chars) {
                feed(*self.ivars(), |s| s.key(k, shift));
            }
        }
    }
);

impl SnipView {
    fn new(mtm: MainThreadMarker, frame: NSRect, index: usize) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(index);
        // SAFETY: NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn moved(&self, e: &NSEvent) {
        let i = *self.ivars();
        let p = point(self, e, scale_of(i));
        feed(i, |s| s.mouse_move(p));
    }
}

// ------------------------------------------------------------------ result

/// Text for a failed screenshot.
pub fn failure(reason: &str) -> String {
    trf(Msg::ScreenshotFailed, &[("reason", &reason)])
}

fn folder(settings: &Settings) -> PathBuf {
    folder_in(settings, std::env::var_os("HOME"))
}

/// The folder from the settings, else ~/Pictures/Screenshots in `home`.
fn folder_in(settings: &Settings, home: Option<OsString>) -> PathBuf {
    if let Some(d) = &settings.screenshot_dir {
        return PathBuf::from(d);
    }
    home.map(|h| Path::new(&h).join("Pictures/Screenshots"))
        .unwrap_or_else(|| std::env::temp_dir().join("Screenshots"))
}

fn file_stem() -> String {
    file_stem_at(blinzel_i18n::lang(), &NSDate::now())
}

/// "Screenshot 2026-10-26 071530" in `lang`, for `date` in local time.
fn file_stem_at(lang: Lang, date: &NSDate) -> String {
    let f = NSDateFormatter::new();
    f.setDateFormat(Some(ns_string!("yyyy-MM-dd HHmmss")));
    let date = f.stringFromDate(date).to_string();
    fill(tr_in(lang, Msg::ScreenshotName), &[("date", &date)])
}

/// Encodes the picked part and writes the file the settings ask for in
/// the background; the result arrives as [`UserEvent::Shot`].
pub fn save(bgra: Vec<u8>, w: u32, h: u32, settings: &Settings) {
    let cache =
        core_shot::cache_folder(&blinzel_ui::dirs::cache().unwrap_or_else(std::env::temp_dir));
    let (settings, folder, stem) = (settings.clone(), folder(settings), file_stem());
    let _ = std::thread::Builder::new()
        .name("blinzel-screenshot".into())
        .spawn(move || {
            let result = blinzel_core::export::encode_png_bgra(&bgra, w, h).map(|png| {
                let file = core_shot::store(&settings, &folder, &cache, &stem, &png);
                Taken { png, file }
            });
            post(UserEvent::Shot(result));
        });
}

/// `Ok` where this process may record the screen (`preflight`), else why
/// not.
fn may_record(preflight: bool) -> Result<(), String> {
    if preflight {
        Ok(())
    } else {
        Err("screen recording is not allowed for this process".into())
    }
}

/// The main display into `out` as PNG, at once (CI): its size.
pub fn capture_test(mtm: MainThreadMarker, out: &Path) -> Result<(u32, u32), String> {
    may_record(CGPreflightScreenCaptureAccess())?;
    let displays = displays(mtm);
    let main = displays.first().copied().ok_or("no display")?;
    let shot = grab(&[main])?.pop().ok_or("no display")?;
    let (w, h) = shot.size;
    let png = blinzel_core::export::encode_png_bgra(&shot.bgra, w, h)?;
    std::fs::write(out, png).map_err(|e| format!("{}: {e}", out.display()))?;
    log(&format!(
        "capture test: {w} x {h}, {} windows",
        shot.windows.len()
    ));
    Ok((w, h))
}

/// The exit code of `--capture-test` and its line: the PNG's size for
/// stdout and 0, or the reason for stderr and 1.
pub fn capture_test_exit(out: &Path, r: &Result<(u32, u32), String>) -> (i32, String) {
    match r {
        Ok((w, h)) => (0, format!("{}: {w}x{h}", out.display())),
        Err(e) => (1, format!("amigo-blinzel: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macos::shell::Delegate;
    use objc2::runtime::AnyClass;
    use objc2::{ClassType, sel};
    use objc2_app_kit::{NSMainMenuWindowLevel, NSStatusWindowLevel};
    use objc2_core_graphics::{
        CGBitmapContextCreateImage, CGDisplayBounds, CGError, CGGetActiveDisplayList,
        CGMainDisplayID, kCGColorSpaceDisplayP3,
    };

    /// `docs/specs/screenshots.md` AC-08: every display gets its own
    /// borderless panel above the menu bar; the pointer is on one of them,
    /// and a drag towards the next display stops at its edge.
    #[test]
    fn each_display_gets_a_panel_above_the_menu_bar_and_keeps_its_selection() {
        let (style, level, behavior) = panel_style();
        assert_eq!(style, NSWindowStyleMask::Borderless);
        assert_eq!(level, NSScreenSaverWindowLevel);
        assert!(level > NSMainMenuWindowLevel && level > NSStatusWindowLevel);
        assert!(behavior.contains(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
        ));
        // A Retina main display and a 1x display right of it.
        let main = Display {
            id: 1,
            frame: Rect::new(0.0, 0.0, 1512.0, 982.0),
            cocoa: Rect::new(0.0, 0.0, 1512.0, 982.0),
            scale: 2.0,
        };
        let right = Display {
            id: 2,
            frame: Rect::new(1512.0, 0.0, 1920.0, 1080.0),
            cocoa: Rect::new(1512.0, -98.0, 1920.0, 1080.0),
            scale: 1.0,
        };
        assert_eq!(cursor_on(&main, (100.0, 50.0)), Some((200.0, 100.0)));
        assert_eq!(cursor_on(&main, (2000.0, 500.0)), None);
        assert_eq!(cursor_on(&right, (2000.0, 500.0)), Some((488.0, 500.0)));
        let frozen = Frozen {
            display: main,
            bgra: Vec::new(),
            size: main.pixels(),
            windows: Vec::new(),
        };
        let mut s = snip_of(&frozen, (2900.0, 1000.0));
        s.mouse_down((2900.0, 1000.0));
        // Released over the display on the right: the part stops at the
        // edge of this one (3024 pixels wide).
        assert_eq!(
            s.mouse_up((3500.0, 1200.0)),
            Outcome::Done(Rect::new(2900.0, 1000.0, 124.0, 200.0))
        );
    }

    /// A 2 × 2 picture of `rgb` in colour space `name`.
    fn picture_in(name: &CFString, rgb: [u8; 3]) -> CFRetained<CGImage> {
        let space = CGColorSpace::with_name(Some(name)).unwrap();
        let mut data = [rgb[0], rgb[1], rgb[2], 255].repeat(4);
        // SAFETY: `data` holds 2 rows of 8 bytes and outlives the context.
        let ctx = unsafe {
            CGBitmapContextCreate(
                data.as_mut_ptr().cast::<c_void>(),
                2,
                2,
                8,
                8,
                Some(&space),
                CGImageAlphaInfo::NoneSkipLast.0,
            )
        }
        .unwrap();
        CGBitmapContextCreateImage(Some(&ctx)).unwrap()
    }

    /// `docs/specs/screenshots.md` AC-08: a display's picture is converted
    /// to sRGB (a Display P3 red gets redder), an sRGB one stays as it is.
    #[test]
    fn a_frozen_picture_is_converted_to_srgb() {
        // SAFETY: Core Graphics' constant names.
        let (p3, srgb) = unsafe { (kCGColorSpaceDisplayP3, kCGColorSpaceSRGB) };
        let px = to_bgra(&picture_in(p3, [200, 60, 60]), (2, 2)).unwrap();
        assert_eq!(px.len(), 16);
        // P3 (200, 60, 60) is about sRGB (219, 42, 52).
        let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
        assert!(
            (212..=226).contains(&r) && (34..=50).contains(&g) && (44..=60).contains(&b),
            "{r} {g} {b}"
        );
        assert_eq!(a, 255);
        let px = to_bgra(&picture_in(srgb, [200, 60, 60]), (2, 2)).unwrap();
        for (got, want) in px[..4].iter().zip([60u8, 60, 200, 255]) {
            assert!(got.abs_diff(want) <= 1, "{:?}", &px[..4]);
        }
    }

    /// `docs/specs/screenshots.md` AC-08: every display is frozen, at its
    /// own size (the CI runner grants Screen Recording, see AC-23).
    #[test]
    fn every_display_is_frozen() {
        // Connects to the window server first, as the app does through
        // NSApplication: ScreenCaptureKit needs that connection.
        let main = CGMainDisplayID();
        assert!(CGPreflightScreenCaptureAccess(), "screen recording");
        let mut ids = [0u32; 16];
        let mut n = 0u32;
        // SAFETY: room for 16 ids and a valid count pointer.
        let err = unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut n) };
        assert_eq!(err, CGError::Success);
        let displays: Vec<Display> = ids[..n as usize]
            .iter()
            .map(|&id| {
                let frame = rect(CGDisplayBounds(id));
                Display {
                    id,
                    frame,
                    cocoa: frame,
                    scale: 1.0,
                }
            })
            .collect();
        assert!(displays.iter().any(|d| d.id == main));
        let shots = grab(&displays).unwrap();
        assert_eq!(shots.len(), displays.len());
        for (f, d) in shots.iter().zip(&displays) {
            assert_eq!((f.display.id, f.size), (d.id, d.pixels()));
            assert_eq!(f.bgra.len(), f.size.0 as usize * f.size.1 as usize * 4);
        }
    }

    /// `docs/specs/screenshots.md` AC-14: a right click on a panel closes
    /// the overlay (its view answers it instead of passing it on), and so
    /// does the app losing the foreground (⌘Tab, a click into another app).
    #[test]
    fn a_right_click_or_losing_the_foreground_cancels_the_overlay() {
        let imp = |class: &AnyClass, name| {
            class
                .instance_method(name)
                .map(|m| m.implementation() as usize)
        };
        let right = sel!(rightMouseDown:);
        assert!(imp(SnipView::class(), right).is_some());
        assert_ne!(imp(SnipView::class(), right), imp(NSView::class(), right));
        assert!(imp(Delegate::class(), sel!(applicationDidResignActive:)).is_some());
    }

    /// `docs/specs/screenshots.md` AC-23: `--capture-test` exits with 0
    /// and the PNG's size, without Screen Recording with 1 and the reason.
    #[test]
    fn the_capture_test_exits_0_with_its_size_else_1() {
        let out = Path::new("capture.png");
        assert!(may_record(true).is_ok());
        assert_eq!(
            capture_test_exit(out, &Ok((1920, 1080))),
            (0, "capture.png: 1920x1080".to_string())
        );
        let (code, line) = capture_test_exit(out, &may_record(false).map(|()| (0, 0)));
        assert_eq!(code, 1);
        assert!(line.contains("screen recording is not allowed"), "{line}");
        assert_eq!(capture_test_exit(out, &Err("no display".into())).0, 1);
    }

    #[test]
    fn quartz_frames_count_down_from_the_main_display() {
        // A display above the main one (1440 points tall), 1080 points tall.
        let above = cocoa_to_quartz(Rect::new(0.0, 1440.0, 1920.0, 1080.0), 1440.0);
        assert_eq!(above, Rect::new(0.0, -1080.0, 1920.0, 1080.0));
        let main = cocoa_to_quartz(Rect::new(0.0, 0.0, 2560.0, 1440.0), 1440.0);
        assert_eq!(main, Rect::new(0.0, 0.0, 2560.0, 1440.0));
    }

    #[test]
    fn windows_map_into_display_pixels() {
        // Retina main display: a window at (100, 50) points.
        let r = to_display_pixels(
            Rect::new(100.0, 50.0, 800.0, 600.0),
            Rect::new(0.0, 0.0, 1512.0, 982.0),
            2.0,
        );
        assert_eq!(r, Rect::new(200.0, 100.0, 1600.0, 1200.0));
        // A window on an external 1x display left of the main one.
        let r = to_display_pixels(
            Rect::new(-1800.0, 10.0, 400.0, 300.0),
            Rect::new(-1920.0, 0.0, 1920.0, 1080.0),
            1.0,
        );
        assert_eq!(r, Rect::new(120.0, 10.0, 400.0, 300.0));
    }

    #[test]
    fn crops_rows() {
        let display = Display {
            id: 1,
            frame: Rect::new(0.0, 0.0, 3.0, 2.0),
            cocoa: Rect::new(0.0, 0.0, 3.0, 2.0),
            scale: 1.0,
        };
        let f = Frozen {
            display,
            bgra: (0..24).collect(),
            size: (3, 2),
            windows: Vec::new(),
        };
        let (px, w, h) = f.crop(Rect::new(1.0, 1.0, 2.0, 1.0));
        assert_eq!((w, h), (2, 1));
        assert_eq!(px, (16..24).collect::<Vec<u8>>());
    }

    /// `docs/specs/screenshots.md` AC-18: the folder chosen in the menu,
    /// else ~/Pictures/Screenshots.
    #[test]
    fn the_folder_is_the_chosen_one_else_pictures_screenshots() {
        let chosen = Settings {
            screenshot_dir: Some("/Volumes/Shots".into()),
            ..Settings::default()
        };
        let home = || Some(OsString::from("/Users/ada"));
        assert_eq!(folder_in(&chosen, home()), Path::new("/Volumes/Shots"));
        let none = Settings::default();
        assert_eq!(
            folder_in(&none, home()),
            Path::new("/Users/ada/Pictures/Screenshots")
        );
    }

    /// `docs/specs/screenshots.md` AC-18: the file is named after the
    /// local time in the UI language.
    #[test]
    fn the_file_is_named_after_the_time_in_the_ui_language() {
        // 2026-07-15 12:00:00 UTC: the 14th to the 16th in any time zone.
        let date = NSDate::dateWithTimeIntervalSince1970(1_784_116_800.0);
        for (lang, name) in [(Lang::En, "Screenshot "), (Lang::De, "Bildschirmfoto ")] {
            let stem = file_stem_at(lang, &date);
            let rest = stem.strip_prefix(name).expect(&stem);
            assert!(rest.starts_with("2026-07-1"), "{stem}");
            let shape: String = rest
                .chars()
                .map(|c| if c.is_ascii_digit() { 'd' } else { c })
                .collect();
            assert_eq!(shape, "dddd-dd-dd dddddd", "{stem}");
        }
    }
}
