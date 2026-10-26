//! Watches every display through ScreenCaptureKit (viewers open wherever
//! they last were). A frame arrives only when the screen changed and
//! carries its display time (`mach_absolute_time`, the clock of
//! `sys::now`); ScreenCaptureKit scales it down to a small grid on the GPU,
//! which is then classified by colour signature. Measuring from outside,
//! from what actually reached the screen, treats every viewer the same.

use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_foundation::{CFDictionary, CFNumber, CFString, CFType};
use objc2_core_graphics::{CGDisplayBounds, CGMainDisplayID};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCFrameStatus, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamFrameInfoDisplayTime, SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType,
};

use crate::frames::{Event, Frames};
use crate::signature;

/// Sample grid over the display.
const COLS: usize = 40;
const ROWS: usize = 24;
/// Captured pixels per grid cell and direction.
const CELL: usize = 4;

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[name = "BlinzelRaceOutput"]
    #[ivars = Frames]
    struct Output;

    // SAFETY: no requirements.
    unsafe impl NSObjectProtocol for Output {}

    // SAFETY: the signature matches ScreenCaptureKit's.
    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(
            &self,
            _stream: &SCStream,
            sample: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind == SCStreamOutputType::Screen
                && let Some(ev) = event(sample)
            {
                self.ivars().push(ev);
            }
        }
    }
);

impl Output {
    fn new(frames: Frames) -> Retained<Self> {
        let this = Self::alloc().set_ivars(frames);
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// A Foundation string as the Core Foundation string it is (toll-free
/// bridged).
fn cf(s: &NSString) -> &CFString {
    // SAFETY: NSString and CFString are the same object.
    unsafe { &*(s as *const NSString).cast::<CFString>() }
}

/// A complete frame: its display time and what it shows.
fn event(sample: &CMSampleBuffer) -> Option<Event> {
    // SAFETY: ScreenCaptureKit's sample buffers carry one attachments
    // dictionary with string keys, and a BGRA pixel buffer.
    unsafe {
        let attachments = sample.sample_attachments_array(false)?;
        let attachments = attachments.cast_unchecked::<CFDictionary>();
        let info = attachments.get(0)?;
        let info = info.cast_unchecked::<CFString, CFType>();
        let number = |key: &NSString| info.get(cf(key))?.downcast::<CFNumber>().ok()?.as_i64();
        // Idle, blank and suspended frames repeat what is on screen.
        if number(SCStreamFrameInfoStatus)? != SCFrameStatus::Complete.0 as i64 {
            return None;
        }
        let at = number(SCStreamFrameInfoDisplayTime)?;
        let buf = sample.image_buffer()?;
        if CVPixelBufferLockBaseAddress(&buf, CVPixelBufferLockFlags::ReadOnly) != 0 {
            return None;
        }
        let (w, h) = (CVPixelBufferGetWidth(&buf), CVPixelBufferGetHeight(&buf));
        let stride = CVPixelBufferGetBytesPerRow(&buf);
        let base = CVPixelBufferGetBaseAddress(&buf).cast::<u8>();
        let mut samples = Vec::with_capacity(COLS * ROWS);
        if !base.is_null() {
            for gy in 0..ROWS {
                let y = (gy * 2 + 1) * h / (ROWS * 2);
                for gx in 0..COLS {
                    let x = (gx * 2 + 1) * w / (COLS * 2);
                    let p = base.add(y * stride + x * 4);
                    samples.push([*p.add(2), *p.add(1), *p]);
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(&buf, CVPixelBufferLockFlags::ReadOnly);
        Some(Event {
            at,
            sig: signature::classify(&samples),
        })
    }
}

fn describe(err: *mut NSError) -> String {
    // SAFETY: ScreenCaptureKit passes a valid error or null.
    match unsafe { err.as_ref() } {
        Some(e) => e.localizedDescription().to_string(),
        None => "unknown error".into(),
    }
}

/// The displays and windows this process may capture (asks for Screen
/// Recording permission the first time).
fn shareable_content() -> Result<Retained<SCShareableContent>, String> {
    let (tx, rx) = mpsc::channel();
    let done = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        // SAFETY: a valid object or null.
        let r = unsafe { Retained::retain(content) }.ok_or_else(|| describe(err));
        let _ = tx.send(r);
    });
    // SAFETY: the block lives until the call returns; ScreenCaptureKit
    // copies it.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&done) };
    rx.recv_timeout(Duration::from_secs(10))
        .map_err(|_| "ScreenCaptureKit did not answer".to_string())?
        .map_err(|e| {
            format!(
                "screen capture: {e} (allow Screen Recording for this terminal in System \
                 Settings, Privacy & Security)"
            )
        })
}

pub struct Capture {
    /// Frames of every display; times are `mach_absolute_time` ticks.
    pub frames: Frames,
    streams: Vec<Retained<SCStream>>,
    _outputs: Vec<Retained<Output>>,
    _queue: DispatchRetained<DispatchQueue>,
    /// The main display (global coordinates, points).
    pub monitor: (i32, i32, i32, i32),
    pub monitors: usize,
}

impl Capture {
    /// One stream per display.
    pub fn start() -> Result<Capture, String> {
        let main = CGMainDisplayID();
        let content = shareable_content()?;
        let frames = Frames::default();
        let queue = DispatchQueue::new("blinzel-race.capture", None);
        let mut streams = Vec::new();
        let mut outputs = Vec::new();
        // SAFETY: ScreenCaptureKit's documented setup; the stream, output
        // and queue are kept alive in `Capture`.
        unsafe {
            let displays = content.displays();
            for display in &displays {
                let filter = SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::new(),
                );
                let config = SCStreamConfiguration::new();
                config.setWidth(COLS * CELL);
                config.setHeight(ROWS * CELL);
                config.setPixelFormat(kCVPixelFormatType_32BGRA);
                config.setShowsCursor(false);
                config.setQueueDepth(8);
                // As often as the display refreshes (up to 240 Hz).
                config.setMinimumFrameInterval(CMTime::new(1, 240));
                let stream = SCStream::initWithFilter_configuration_delegate(
                    SCStream::alloc(),
                    &filter,
                    &config,
                    None,
                );
                let output = Output::new(frames.clone());
                stream
                    .addStreamOutput_type_sampleHandlerQueue_error(
                        ProtocolObject::from_ref(&*output),
                        SCStreamOutputType::Screen,
                        Some(&queue),
                    )
                    .map_err(|e| format!("screen capture: {}", e.localizedDescription()))?;
                let (tx, rx) = mpsc::channel();
                let started = RcBlock::new(move |err: *mut NSError| {
                    let _ = tx.send((!err.is_null()).then(|| describe(err)));
                });
                stream.startCaptureWithCompletionHandler(Some(&started));
                match rx.recv_timeout(Duration::from_secs(10)) {
                    Ok(None) => {}
                    Ok(Some(e)) => return Err(format!("screen capture: {e}")),
                    Err(_) => return Err("screen capture did not start".into()),
                }
                streams.push(stream);
                outputs.push(output);
            }
        }
        if streams.is_empty() {
            return Err("no display to capture".into());
        }
        let b = CGDisplayBounds(main);
        let monitor = (
            b.origin.x as i32,
            b.origin.y as i32,
            (b.origin.x + b.size.width) as i32,
            (b.origin.y + b.size.height) as i32,
        );
        Ok(Capture {
            frames,
            monitors: streams.len(),
            streams,
            _outputs: outputs,
            _queue: queue,
            monitor,
        })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        for s in &self.streams {
            // SAFETY: a running stream; no completion handler.
            unsafe { s.stopCaptureWithCompletionHandler(None) };
        }
    }
}
