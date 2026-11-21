//! The info overlay (`docs/specs/info-overlay.md`): the file name, its place
//! in the folder and the picture's facts in a box at the bottom left.
//!
//! The facts that need the file (pixel size, format, EXIF) come from a
//! worker thread, one file at a time and always the latest one asked for,
//! so neither the first frame nor browsing waits for them. A result wakes
//! the UI thread through [`Wake::Thumbs`], whose handler polls this
//! reader too: that keeps the platform shells unchanged.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

use blinzel_core::exif_info::{ExifInfo, Taken};
use blinzel_core::format::{Format, detect};
use blinzel_i18n::{Msg, tr, trf};

use crate::backend::{Services, Wake};

/// The worker thread's name.
pub const THREAD: &str = "blinzel-info";

/// How much of a file is read for its facts: a bigger file is read only
/// this far, and a fact that lies beyond is left out (pixel data never
/// needs reading for them).
#[cfg(not(test))]
const HEAD: u64 = 64 << 20;
#[cfg(test)]
pub(crate) const HEAD: u64 = 1 << 20;

/// Which file the facts are for. Size and write time are part of it, so a
/// replaced file is read again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: u64,
}

/// What the file says about the picture.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    /// Full pixel size after the EXIF orientation, where the header tells.
    pub size: Option<(u32, u32)>,
    pub format: Option<Format>,
    pub exif: ExifInfo,
}

struct Shared {
    want: Mutex<(Option<Target>, bool)>,
    cv: Condvar,
}

/// Reads the facts of the file last asked for, in the background.
pub struct Reader {
    shared: Arc<Shared>,
    tx: Sender<(Target, Facts)>,
    rx: Receiver<(Target, Facts)>,
    started: bool,
    /// What was asked for last.
    asked: Option<Target>,
    /// The facts of `asked`, once read.
    facts: Option<Facts>,
}

impl Default for Reader {
    fn default() -> Reader {
        let (tx, rx) = channel();
        Reader {
            shared: Arc::new(Shared {
                want: Mutex::new((None, false)),
                cv: Condvar::new(),
            }),
            tx,
            rx,
            started: false,
            asked: None,
            facts: None,
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.shared.want.lock().unwrap().1 = true;
        self.shared.cv.notify_all();
    }
}

impl Reader {
    /// The facts of `target`, if read; asks for them otherwise. Facts of
    /// another file are never returned.
    pub fn facts<S: Services>(&mut self, target: &Target, services: &Arc<S>) -> Option<&Facts> {
        if self.asked.as_ref() != Some(target) {
            self.asked = Some(target.clone());
            self.facts = None;
            self.start(services);
            self.shared.want.lock().unwrap().0 = Some(target.clone());
            self.shared.cv.notify_all();
        }
        self.facts.as_ref()
    }

    /// Takes the results that arrived; true if the asked-for one is new.
    pub fn poll(&mut self) -> bool {
        let mut fresh = false;
        while let Ok((target, facts)) = self.rx.try_recv() {
            if self.asked.as_ref() == Some(&target) {
                self.facts = Some(facts);
                fresh = true;
            }
        }
        fresh
    }

    fn start<S: Services>(&mut self, services: &Arc<S>) {
        if self.started {
            return;
        }
        self.started = true;
        let (shared, tx, services) = (self.shared.clone(), self.tx.clone(), services.clone());
        let _ = std::thread::Builder::new()
            .name(THREAD.into())
            .spawn(move || {
                services.tune_thread(false);
                loop {
                    let target = {
                        let mut w = shared.want.lock().unwrap();
                        loop {
                            if w.1 {
                                return;
                            }
                            if let Some(t) = w.0.take() {
                                break t;
                            }
                            w = shared.cv.wait(w).unwrap();
                        }
                    };
                    let facts = read(&*services, &target);
                    if tx.send((target, facts)).is_err() {
                        return;
                    }
                    services.wake(Wake::Thumbs);
                }
            });
    }
}

/// Reads the facts of `target`'s file without decoding its pixels; a file
/// that cannot be read gives none, a broken one what its bytes tell.
fn read<S: Services>(services: &S, target: &Target) -> Facts {
    let data: Vec<u8> = if target.size <= HEAD {
        match services.read_file(&target.path) {
            Ok(d) => d.to_vec(),
            Err(_) => return Facts::default(),
        }
    } else {
        // A big file: its start only, where the headers and (in the
        // formats that carry one there) the EXIF block are.
        use std::io::Read;
        let mut d = Vec::new();
        let read = std::fs::File::open(&target.path).and_then(|f| f.take(HEAD).read_to_end(&mut d));
        if read.is_err() {
            return Facts::default();
        }
        d
    };
    blinzel_core::decode::guard(|| {
        let probe = services.probe(&data).ok();
        let format = probe.as_ref().map_or_else(|| detect(&data), |p| p.format);
        Ok(Facts {
            size: probe.as_ref().map(|p| p.oriented_size()),
            format: (format != Format::Unknown).then_some(format),
            exif: ExifInfo::from_file(&data, format),
        })
    })
    .unwrap_or_default()
}

/// What the overlay shows of one image.
pub struct Shown<'a> {
    pub name: &'a str,
    /// Index (from 0) and count in the folder list.
    pub position: Option<(usize, usize)>,
    pub file_size: Option<u64>,
    /// `None` until read, and for an image that failed to open.
    pub facts: Option<&'a Facts>,
}

/// Camera raw extensions, shown instead of the container's name (a NEF
/// opens as TIFF).
const RAW: &[&str] = &[
    "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "orf", "rw2", "raf", "pef",
    "srw", "x3f", "erf", "kdc", "mrw", "3fr", "iiq", "rwl",
];

/// The common name of a format; camera raw by its extension.
pub fn format_name(format: Format, name: &str) -> Option<String> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    if let Some(ext) = ext.filter(|e| RAW.contains(&e.as_str())) {
        return Some(ext.to_ascii_uppercase());
    }
    Some(
        match format {
            Format::Jpeg => "JPEG",
            Format::Png => "PNG",
            Format::Gif => "GIF",
            Format::Bmp => "BMP",
            Format::Tiff => "TIFF",
            Format::WebP => "WebP",
            Format::Heif => "HEIC",
            Format::Avif => "AVIF",
            Format::Jxl => "JPEG XL",
            Format::Ico => "ICO",
            Format::Dds => "DDS",
            Format::Psd => "PSD",
            Format::Qoi => "QOI",
            Format::JpegXr => "JPEG XR",
            Format::Raw | Format::Unknown => return None,
        }
        .to_string(),
    )
}

/// The overlay's lines, in order; facts that are missing are left out.
pub fn lines(s: &Shown) -> Vec<String> {
    let sep = tr(Msg::DecimalSeparator);
    let mut out = vec![s.name.to_string()];
    if let Some((i, n)) = s.position {
        out.push(trf(Msg::Position, &[("index", &(i + 1)), ("total", &n)]));
    }
    let facts = s.facts;
    let size = facts
        .and_then(|f| f.size)
        .map(|(w, h)| format!("{w} × {h}"));
    let format = facts
        .and_then(|f| f.format)
        .and_then(|f| format_name(f, s.name));
    match (size, format) {
        (Some(a), Some(b)) => out.push(format!("{a} · {b}")),
        (Some(a), None) | (None, Some(a)) => out.push(a),
        (None, None) => {}
    }
    if let Some(n) = s.file_size {
        out.push(file_size(n, sep));
    }
    let Some(f) = facts else {
        return out;
    };
    let e = &f.exif;
    if let Some(t) = e.taken {
        out.push(date(t));
    }
    out.extend(e.camera());
    out.extend(e.lens.clone());
    let mut exposure = Vec::new();
    if let Some(f) = e.f_number {
        exposure.push(format!("f/{}", decimal(f, sep)));
    }
    if let Some((n, d)) = e.exposure {
        exposure.push(format!("{} s", exposure_time(n, d, sep)));
    }
    if let Some(iso) = e.iso {
        exposure.push(format!("ISO {iso}"));
    }
    if let Some(mm) = e.focal_mm {
        exposure.push(format!("{} mm", mm.round()));
    }
    if !exposure.is_empty() {
        out.push(exposure.join(" · "));
    }
    out
}

/// One decimal place, dropped when it is zero.
fn decimal(v: f64, sep: &str) -> String {
    let tenths = (v * 10.0).round() as u64;
    if tenths.is_multiple_of(10) {
        format!("{}", tenths / 10)
    } else {
        format!("{}{sep}{}", tenths / 10, tenths % 10)
    }
}

/// "1/250" below half a second, else seconds with at most one decimal.
fn exposure_time(n: u32, d: u32, sep: &str) -> String {
    let s = n as f64 / d as f64;
    if s < 0.5 {
        format!("1/{}", (1.0 / s).round())
    } else {
        decimal(s, sep)
    }
}

/// Decimal units: bytes below 1 000, then KB, MB, GB with one decimal.
fn file_size(n: u64, sep: &str) -> String {
    let (div, unit) = match n {
        0..1_000 => return format!("{n} B"),
        1_000..1_000_000 => (1e3, "KB"),
        1_000_000..1_000_000_000 => (1e6, "MB"),
        _ => (1e9, "GB"),
    };
    let tenths = (n as f64 / div * 10.0).round() as u64;
    // 999 950 bytes round up to 1.0 MB, not 1000.0 KB.
    let (tenths, unit) = match (tenths, unit) {
        (10_000.., "KB") => (10, "MB"),
        (10_000.., "MB") => (10, "GB"),
        t => t,
    };
    format!("{}{sep}{} {unit}", tenths / 10, tenths % 10)
}

fn date(t: Taken) -> String {
    let two = |v: u8| format!("{v:02}");
    trf(
        Msg::InfoDate,
        &[
            ("year", &t.year),
            ("month", &two(t.month)),
            ("day", &two(t.day)),
            ("hour", &two(t.hour)),
            ("minute", &two(t.minute)),
        ],
    )
}

/// `text` shortened with "…" until `width(text) <= max`: in the middle
/// (keeping the end, a file's extension) or at the end.
pub fn shorten(text: &str, max: f64, middle: bool, mut width: impl FnMut(&str) -> f64) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    // The most characters that fit, found by halving.
    let make = |keep: usize| -> String {
        if middle {
            let tail = keep / 2;
            let head = keep - tail;
            let mut s: String = chars[..head].iter().collect();
            s.push('…');
            s.extend(&chars[chars.len() - tail..]);
            s
        } else {
            let mut s: String = chars[..keep].iter().collect();
            s.push('…');
            s
        }
    };
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if width(&make(mid)) <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    make(lo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposure_times_read_as_fractions_below_half_a_second() {
        assert_eq!(exposure_time(10, 2500, "."), "1/250");
        assert_eq!(exposure_time(1, 3, "."), "1/3");
        assert_eq!(exposure_time(8, 10, "."), "0.8");
        assert_eq!(exposure_time(2, 1, "."), "2");
        assert_eq!(exposure_time(125, 100, ","), "1,3");
        assert_eq!(exposure_time(1, 2, "."), "0.5");
    }

    #[test]
    fn f_numbers_drop_a_zero_decimal_after_rounding() {
        assert_eq!(decimal(2.8, "."), "2.8");
        assert_eq!(decimal(8.0, "."), "8");
        assert_eq!(decimal(2.96, "."), "3");
        assert_eq!(decimal(5.6, ","), "5,6");
    }

    #[test]
    fn file_sizes_use_decimal_units() {
        assert_eq!(file_size(999, "."), "999 B");
        assert_eq!(file_size(1_000, "."), "1.0 KB");
        assert_eq!(file_size(8_400_000, ","), "8,4 MB");
        assert_eq!(file_size(2_000_000_000, "."), "2.0 GB");
        assert_eq!(file_size(999_950, "."), "1.0 MB");
        assert_eq!(file_size(999_949, "."), "999.9 KB");
        assert_eq!(file_size(999_960_000, "."), "1.0 GB");
    }

    #[test]
    fn raw_files_show_their_extension_and_others_their_format() {
        assert_eq!(format_name(Format::Tiff, "a.NEF").as_deref(), Some("NEF"));
        assert_eq!(format_name(Format::Raw, "a.cr3").as_deref(), Some("CR3"));
        assert_eq!(format_name(Format::Tiff, "a.tif").as_deref(), Some("TIFF"));
        assert_eq!(
            format_name(Format::Jxl, "a.jxl").as_deref(),
            Some("JPEG XL")
        );
        assert_eq!(format_name(Format::Unknown, "a.xyz"), None);
    }

    #[test]
    fn shorten_keeps_the_extension_in_the_middle_mode() {
        let w = |s: &str| s.chars().count() as f64;
        assert_eq!(shorten("short.png", 20.0, true, w), "short.png");
        let s = shorten("abcdefghijklmnop.png", 11.0, true, w);
        assert_eq!(s.chars().count(), 11);
        assert!(
            s.starts_with("abcde") && s.ends_with(".png") && s.contains('…'),
            "{s}"
        );
        assert_eq!(shorten("abcdefghij", 5.0, false, w), "abcd…");
    }
}
