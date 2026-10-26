//! Running a batch: files in parallel (one decode thread each), bounded by
//! a memory budget, with progress reports and cancellation. One broken
//! file is reported and skipped; the batch goes on.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

use super::naming::{self, Claims, Fields};
use super::rows::Rows;
use super::{Job, codec_for, encode, geometry, meta, resample, sharpen};
use crate::decode::{self, DecodeError, Probe, Request};
use crate::format::Format;
use crate::pixel::Image;
use crate::selection::orient_bgra;

/// Where pixels come from: the native decoders, plus the platform's
/// codecs in the app.
pub trait Source: Sync {
    fn probe(&self, data: &[u8]) -> Result<Probe, DecodeError>;
    fn decode(&self, data: &[u8], req: Request) -> Result<Image, DecodeError>;
}

/// Native decoders only.
pub struct Native;

impl Source for Native {
    fn probe(&self, data: &[u8]) -> Result<Probe, DecodeError> {
        decode::probe(data)
    }

    fn decode(&self, data: &[u8], req: Request) -> Result<Image, DecodeError> {
        decode::decode(data, req)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    Done {
        index: usize,
        source: PathBuf,
        output: PathBuf,
        bytes: u64,
    },
    Failed {
        index: usize,
        source: PathBuf,
        error: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub done: usize,
    pub failed: usize,
    /// Files not started because the batch was cancelled.
    pub skipped: usize,
}

pub struct Options {
    pub threads: usize,
    /// Bytes of decoded pixels in flight at once (a single larger image
    /// still runs, alone).
    pub memory_budget: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            threads: std::thread::available_parallelism().map_or(4, |n| n.get()),
            memory_budget: 2 << 30,
        }
    }
}

struct Budget {
    used: Mutex<u64>,
    freed: Condvar,
    limit: u64,
}

impl Budget {
    fn take(&self, n: u64) -> u64 {
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        while *used > 0 && *used + n > self.limit {
            used = self.freed.wait(used).unwrap_or_else(|e| e.into_inner());
        }
        *used += n;
        n
    }

    fn give(&self, n: u64) {
        *self.used.lock().unwrap_or_else(|e| e.into_inner()) -= n;
        self.freed.notify_all();
    }
}

/// Resizes `files`, reporting each result through `progress` (called from
/// worker threads). Files not started when `cancel` is set are skipped.
pub fn run(
    job: &Job,
    files: &[PathBuf],
    source: &dyn Source,
    opts: &Options,
    progress: &(dyn Fn(Progress) + Sync),
    cancel: &AtomicBool,
) -> Summary {
    let next = AtomicUsize::new(0);
    let (done, failed) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let claims = Claims::default();
    let budget = Budget {
        used: Mutex::new(0),
        freed: Condvar::new(),
        limit: opts.memory_budget.max(1),
    };
    let workers = opts.threads.clamp(1, files.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else { break };
                    let r = decode::guard(|| Ok(one(job, path, i + 1, source, &claims, &budget)))
                        .unwrap_or_else(|e| Err(e.to_string()));
                    progress(match r {
                        Ok((output, bytes)) => {
                            done.fetch_add(1, Ordering::Relaxed);
                            Progress::Done {
                                index: i,
                                source: path.clone(),
                                output,
                                bytes,
                            }
                        }
                        Err(error) => {
                            failed.fetch_add(1, Ordering::Relaxed);
                            Progress::Failed {
                                index: i,
                                source: path.clone(),
                                error,
                            }
                        }
                    });
                }
            });
        }
    });
    let (done, failed) = (done.into_inner(), failed.into_inner());
    Summary {
        done,
        failed,
        skipped: files.len() - done - failed,
    }
}

/// Resizes one file; returns the output path and its size in bytes.
fn one(
    job: &Job,
    path: &Path,
    index: usize,
    source: &dyn Source,
    claims: &Claims,
    budget: &Budget,
) -> Result<(PathBuf, u64), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let probe = source.probe(&data).map_err(|e| e.to_string())?;
    let stored = (probe.width, probe.height);
    let g = geometry(stored, probe.orientation, job.size, job.enlarge);

    // JPEG decodes at 1/2, 1/4 or 1/8 for free; keep twice the target.
    let req = if probe.format == Format::Jpeg {
        let need = |out: u32, crop: u32, full: u32| {
            ((out as u64 * 2 * full as u64).div_ceil(crop as u64) as u32).min(full)
        };
        Request::Fit {
            w: need(g.out.0, g.crop.w, stored.0),
            h: need(g.out.1, g.crop.h, stored.1),
        }
    } else {
        Request::Full
    };
    let estimate = stored.0 as u64 * stored.1 as u64 * 4;
    let held = budget.take(estimate);
    let pixels = (|| {
        let img = source.decode(&data, req).map_err(|e| e.to_string())?;
        let (iw, ih) = (img.width, img.height);
        // The crop at the decoded scale.
        let sx = iw as f64 / stored.0 as f64;
        let sy = ih as f64 / stored.1 as f64;
        let cx = ((g.crop.x as f64 * sx).round() as u32).min(iw - 1);
        let cy = ((g.crop.y as f64 * sy).round() as u32).min(ih - 1);
        let cw = ((g.crop.w as f64 * sx).round() as u32).clamp(1, iw - cx);
        let ch = ((g.crop.h as f64 * sy).round() as u32).clamp(1, ih - cy);
        let rows = Rows::new(&img);
        Ok::<_, String>(resample::resize_rows(
            cw,
            ch,
            g.out.0,
            g.out.1,
            rows.opaque(),
            &mut |y, out| rows.row(cy + y as u32, cx, out),
        ))
    })();
    budget.give(held);
    let resized = pixels?;

    let (mut px, w, h) = orient_bgra(&resized, g.out.0, g.out.1, probe.orientation);
    drop(resized);
    if let Some(s) = job.sharpen {
        sharpen::apply(&mut px, w, h, s);
    }
    let alpha = px.as_chunks::<4>().0.iter().any(|p| p[3] != 255);
    let codec = codec_for(job.output, probe.format, alpha);

    let source_exif = meta::read_exif(&data, probe.format);
    let exif = source_exif
        .as_deref()
        .filter(|_| job.keep_exif)
        .and_then(|t| meta::rewrite(t, (w, h), job.keep_gps));
    let icc = probe
        .icc
        .as_ref()
        .filter(|_| job.keep_icc)
        .map(|p| &p.bytes[..]);
    let bytes = encode::encode(&px, w, h, codec, &job.quality, exif.as_deref(), icc)?;

    let date = source_exif
        .as_deref()
        .and_then(meta::date_taken)
        .or_else(|| {
            std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .map(naming::date_of)
        })
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let base = naming::render(
        &job.pattern,
        &Fields {
            name: &stem,
            ext: &ext,
            index,
            width: w,
            height: h,
            date: &date,
        },
    )?;
    let dir = path.parent().unwrap_or(Path::new(".")).join(&job.out_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let out = write_new(&dir, &base, codec.extension(), &bytes, claims)?;
    Ok((out, bytes.len() as u64))
}

/// Writes `bytes` to a new file `dir/base.ext` (numbered if taken): a
/// temporary file first, then linked to its name, which fails instead of
/// replacing a file that appeared meanwhile.
fn write_new(
    dir: &Path,
    base: &str,
    ext: &str,
    bytes: &[u8],
    claims: &Claims,
) -> Result<PathBuf, String> {
    let tmp = dir.join(format!(".{base}.{}.blinzel-tmp", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let result = loop {
        let target = claims.claim(dir, base, ext);
        match std::fs::hard_link(&tmp, &target) {
            Ok(()) => break Ok(target),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // File systems without hard links (FAT, some network shares):
            // the claim checked the name a moment ago.
            Err(_) => match std::fs::rename(&tmp, &target) {
                Ok(()) => return Ok(target),
                Err(e) => break Err(format!("{}: {e}", target.display())),
            },
        }
    };
    let _ = std::fs::remove_file(&tmp);
    result
}

// The tests write JPEGs, which needs libjpeg-turbo.
#[cfg(all(test, feature = "turbojpeg"))]
mod tests {
    use super::*;
    use crate::batch::{Codec, Output, Size};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("blinzel-batch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A 300×200 PNG gradient with an EXIF block. The block says
    /// orientation 6, which PNG decoders do not apply: the file shows as
    /// stored.
    fn source_png() -> Vec<u8> {
        let (w, h) = (300u32, 200u32);
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                px.extend([x as u8, y as u8, if x < w / 2 { 40 } else { 220 }, 255]);
            }
        }
        let exif = crate::batch::meta::tests::sample();
        encode::encode(
            &px,
            w,
            h,
            Codec::Png,
            &Default::default(),
            Some(&exif),
            None,
        )
        .unwrap()
    }

    #[test]
    fn batch_writes_new_files_and_never_replaces() {
        let dir = tmp("run");
        let a = dir.join("a.png");
        std::fs::write(&a, source_png()).unwrap();
        let broken = dir.join("broken.jpg");
        std::fs::write(&broken, b"\xFF\xD8\xFF\xE0 not really").unwrap();
        // Something already named like the output.
        std::fs::create_dir_all(dir.join("out")).unwrap();
        std::fs::write(dir.join("out/a-150.jpg"), b"keep me").unwrap();

        let job = Job {
            size: Size::Width(150),
            output: Output::Codec(Codec::Jpeg),
            pattern: "{name}-{w}".into(),
            out_dir: "out".into(),
            sharpen: Some(Default::default()),
            ..Job::default()
        };
        let events = Mutex::new(Vec::new());
        let files = vec![a.clone(), broken.clone(), a.clone()];
        let sum = run(
            &job,
            &files,
            &Native,
            &Options {
                threads: 2,
                memory_budget: 1,
            },
            &|p| events.lock().unwrap().push(p),
            &AtomicBool::new(false),
        );
        assert_eq!(
            sum,
            Summary {
                done: 2,
                failed: 1,
                skipped: 0
            }
        );
        assert_eq!(
            std::fs::read(dir.join("out/a-150.jpg")).unwrap(),
            b"keep me"
        );
        let mut outs: Vec<PathBuf> = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Progress::Done { output, .. } => Some(output.clone()),
                _ => None,
            })
            .collect();
        outs.sort();
        assert_eq!(
            outs,
            [dir.join("out/a-150 (2).jpg"), dir.join("out/a-150 (3).jpg")]
        );
        for o in &outs {
            let file = std::fs::read(o).unwrap();
            let p = decode::probe(&file).unwrap();
            assert_eq!((p.width, p.height), (150, 100));
            let exif = meta::read_exif(&file, Format::Jpeg).unwrap();
            assert_eq!(meta::date_taken(&exif).as_deref(), Some("2024-05-17"));
        }
        // Only the outputs and the old file: no temporary files left.
        assert_eq!(std::fs::read_dir(dir.join("out")).unwrap().count(), 3);
        assert_eq!(std::fs::read(&a).unwrap(), source_png());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelled_batches_skip_the_rest() {
        let dir = tmp("cancel");
        let a = dir.join("a.png");
        std::fs::write(&a, source_png()).unwrap();
        let sum = run(
            &Job::default(),
            &[a.clone(), a.clone()],
            &Native,
            &Options::default(),
            &|_| {},
            &AtomicBool::new(true),
        );
        assert_eq!(sum.skipped, 2);
        assert!(!dir.join("resized").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn jpeg_sources_are_decoded_at_a_reduced_scale_and_rotated() {
        // 640×480 JPEG with EXIF orientation 6: shown 480×640.
        let (w, h) = (640u32, 480u32);
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                // Bright top-left quadrant, to check the rotation.
                let v = if x < w / 2 && y < h / 2 { 230 } else { 30 };
                px.extend([v, v, v, 255]);
            }
        }
        let exif = crate::batch::meta::tests::sample();
        let jpeg = encode::encode(
            &px,
            w,
            h,
            Codec::Jpeg,
            &Default::default(),
            Some(&exif),
            None,
        )
        .unwrap();
        let dir = tmp("jpeg");
        let src = dir.join("p.jpg");
        std::fs::write(&src, &jpeg).unwrap();
        let job = Job {
            size: Size::Width(120),
            keep_gps: false,
            ..Job::default()
        };
        let out = Mutex::new(None);
        run(
            &job,
            std::slice::from_ref(&src),
            &Native,
            &Options::default(),
            &|p| {
                if let Progress::Done { output, .. } = p {
                    *out.lock().unwrap() = Some(output);
                }
            },
            &AtomicBool::new(false),
        );
        let out = out.into_inner().unwrap().expect("one output");
        assert_eq!(out, dir.join("resized/p.jpg"));
        let file = std::fs::read(&out).unwrap();
        let p = decode::probe(&file).unwrap();
        assert_eq!((p.width, p.height), (120, 160), "pixels turned upright");
        assert_eq!(p.orientation, crate::geom::Orientation::NORMAL);
        let img = decode::decode(&file, Request::Full).unwrap().to_bgra();
        // Orientation 6 turns clockwise: the bright stored top-left ends
        // up top-right.
        let at = |x: u32, y: u32| img[((y * 120 + x) * 4) as usize];
        assert!(
            at(100, 20) > 200 && at(20, 20) < 60,
            "{} {}",
            at(100, 20),
            at(20, 20)
        );
        let exif = meta::read_exif(&file, Format::Jpeg).unwrap();
        let t = crate::exif::Tiff::new(&exif).unwrap();
        let (ifd0, _) = t.ifd(t.first_ifd().unwrap()).unwrap();
        assert!(!ifd0.iter().any(|e| e.tag == 0x8825), "GPS removed");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
