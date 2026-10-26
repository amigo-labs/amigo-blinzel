//! Running a batch: files in parallel (one decode thread each), bounded by
//! a memory budget, with progress reports and cancellation. One broken
//! file is reported and skipped; the batch goes on.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

use super::naming::Claims;
use super::plan::{self, Item};
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
    /// Seconds the local clock was ahead of UTC at `t` (file dates);
    /// UTC unless the platform knows better.
    fn utc_offset(&self, _t: std::time::SystemTime) -> i32 {
        0
    }
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
/// Planned targets ([`plan::targets`]) are kept free for their file.
pub fn run(
    job: &Job,
    files: &[Item],
    source: &dyn Source,
    opts: &Options,
    progress: &(dyn Fn(Progress) + Sync),
    cancel: &AtomicBool,
) -> Summary {
    let next = AtomicUsize::new(0);
    let (done, failed) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let claims = Claims::default();
    for t in files.iter().filter_map(|f| f.target.clone()) {
        claims.reserve(t);
    }
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
                    let Some(item) = files.get(i) else { break };
                    let path = &item.source;
                    let r = decode::guard(|| Ok(one(job, item, i + 1, source, &claims, &budget)))
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
    item: &Item,
    index: usize,
    source: &dyn Source,
    claims: &Claims,
    budget: &Budget,
) -> Result<(PathBuf, u64), String> {
    let path = &item.source;
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

    let date = plan::date_of(source_exif.as_deref(), path, source);
    let base = plan::base_name(job, path, index, (w, h), date)?;
    let dir = plan::out_dir(job, path);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // The planned name, unless the format turned out otherwise.
    let ext = codec.extension();
    let planned = item
        .target
        .as_deref()
        .filter(|t| t.extension().is_some_and(|e| e == ext));
    let out = write_new(&dir, &base, ext, &bytes, planned, claims)?;
    Ok((out, bytes.len() as u64))
}

/// Writes `bytes` to a new file: `planned` if given, else `dir/base.ext`
/// (numbered if taken). A temporary file first, then linked to its name,
/// which fails instead of replacing a file that appeared meanwhile.
fn write_new(
    dir: &Path,
    base: &str,
    ext: &str,
    bytes: &[u8],
    planned: Option<&Path>,
    claims: &Claims,
) -> Result<PathBuf, String> {
    // One temporary file per write: files of a batch may share a name.
    static WRITES: AtomicUsize = AtomicUsize::new(0);
    let n = WRITES.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{base}.{}-{n}.blinzel-tmp", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let mut planned = planned.map(Path::to_path_buf);
    let result = loop {
        let target = planned
            .take()
            .unwrap_or_else(|| claims.claim(dir, base, ext));
        match std::fs::hard_link(&tmp, &target) {
            Ok(()) => break Ok(target),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // File systems without hard links (FAT, some network shares):
            // the claim checked the name a moment ago.
            Err(_) if std::fs::symlink_metadata(&target).is_ok() => continue,
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
        let files: Vec<Item> = [&a, &broken, &a].map(|p| p.clone().into()).into();
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
            assert_eq!(
                meta::date_taken(&exif)
                    .map(|s| s.format("YYYY-MM-DD"))
                    .as_deref(),
                Some("2024-05-17")
            );
        }
        // Only the outputs and the old file: no temporary files left.
        assert_eq!(std::fs::read_dir(dir.join("out")).unwrap().count(), 3);
        assert_eq!(std::fs::read(&a).unwrap(), source_png());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn planned_names_are_the_names_written() {
        let dir = tmp("plan");
        let sources: Vec<PathBuf> = (0..4).map(|i| dir.join(format!("{i}.png"))).collect();
        for s in &sources {
            std::fs::write(s, source_png()).unwrap();
        }
        // Same name for all four: only the order decides the numbers.
        let job = Job {
            size: Size::Width(60),
            pattern: "run_{date:DD.MM.YYYY_hh-mm-ss}".into(),
            ..Job::default()
        };
        let infos: Vec<_> = sources
            .iter()
            .map(|s| plan::inspect(s, &Native).unwrap())
            .collect();
        let pairs: Vec<(&Path, &plan::SourceInfo)> =
            sources.iter().map(PathBuf::as_path).zip(&infos).collect();
        let targets: Vec<PathBuf> = plan::targets(&job, &pairs)
            .into_iter()
            .map(Result::unwrap)
            .collect();
        assert_eq!(targets[0], dir.join("resized/run_17.05.2024_10-11-12.png"));
        assert_eq!(
            targets[3],
            dir.join("resized/run_17.05.2024_10-11-12 (4).png")
        );
        let items: Vec<Item> = sources
            .iter()
            .zip(&targets)
            .map(|(s, t)| Item {
                source: s.clone(),
                target: Some(t.clone()),
            })
            .collect();
        let written = Mutex::new(vec![None; items.len()]);
        run(
            &job,
            &items,
            &Native,
            &Options {
                threads: 4,
                ..Options::default()
            },
            &|p| {
                if let Progress::Done { index, output, .. } = p {
                    written.lock().unwrap()[index] = Some(output);
                }
            },
            &AtomicBool::new(false),
        );
        let written: Vec<PathBuf> = written
            .into_inner()
            .unwrap()
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(written, targets);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelled_batches_skip_the_rest() {
        let dir = tmp("cancel");
        let a = dir.join("a.png");
        std::fs::write(&a, source_png()).unwrap();
        let sum = run(
            &Job::default(),
            &[a.clone().into(), a.clone().into()],
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
            &[src.clone().into()],
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
