//! What each file of a batch becomes, worked out from its header before
//! anything is resized: the dialog shows these names, and the run writes
//! exactly them (unless a file of that name appeared in between).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::naming::{self, Claims, Fields, Stamp};
use super::run::Source;
use super::{Codec, Job, Output, codec_for, geometry, meta};
use crate::decode::Request;
use crate::format::Format;
use crate::geom::Orientation;

/// Bytes read for the header; files whose header does not fit are read
/// whole.
const HEAD: usize = 256 << 10;

/// Header facts of a source file that its output name depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    pub format: Format,
    /// Stored (not oriented) size.
    pub stored: (u32, u32),
    pub orientation: Orientation,
    /// Capture time, else the modification time.
    pub date: Option<Stamp>,
    /// Transparent pixels; only looked at where the output format depends
    /// on it.
    pub alpha: bool,
}

/// A file to resize and, if planned, the output path it gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub source: PathBuf,
    pub target: Option<PathBuf>,
}

impl From<PathBuf> for Item {
    fn from(source: PathBuf) -> Item {
        Item {
            source,
            target: None,
        }
    }
}

/// The capture time from EXIF, else the file's modification time in the
/// platform's local time.
pub fn date_of(exif: Option<&[u8]>, path: &Path, source: &dyn Source) -> Option<Stamp> {
    exif.and_then(meta::date_taken).or_else(|| {
        let t: SystemTime = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
        Some(naming::stamp_of(t, source.utc_offset(t)))
    })
}

/// Reads what [`target`] needs. Decodes pixels only for formats that are
/// written as PNG or JPEG depending on transparency.
pub fn inspect(path: &Path, source: &dyn Source) -> Result<SourceInfo, String> {
    use std::io::Read;
    let mut head = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(HEAD as u64).read_to_end(&mut head))
        .map_err(|e| e.to_string())?;
    let mut data = head;
    let probe = match source.probe(&data) {
        Ok(p) if data.len() < HEAD || p.width > 0 => p,
        _ => {
            data = std::fs::read(path).map_err(|e| e.to_string())?;
            source.probe(&data).map_err(|e| e.to_string())?
        }
    };
    let exif = meta::read_exif(&data, probe.format);
    let date = date_of(exif.as_deref(), path, source);
    let alpha_matters = codec_for(Output::Same, probe.format, true)
        != codec_for(Output::Same, probe.format, false)
        && !probe.is_raw();
    let alpha = alpha_matters && {
        if data.len() == HEAD {
            data = std::fs::read(path).map_err(|e| e.to_string())?;
        }
        let img = source
            .decode(&data, Request::Full)
            .map_err(|e| e.to_string())?;
        img.has_alpha() && img.to_bgra().as_chunks::<4>().0.iter().any(|p| p[3] != 255)
    };
    Ok(SourceInfo {
        format: probe.format,
        stored: (probe.width, probe.height),
        orientation: probe.orientation,
        date,
        alpha,
    })
}

/// The output size in display orientation.
pub fn out_size(job: &Job, info: &SourceInfo) -> (u32, u32) {
    let g = geometry(info.stored, info.orientation, job.size, job.enlarge);
    info.orientation.oriented_size(g.out.0, g.out.1)
}

pub fn codec(job: &Job, info: &SourceInfo) -> Codec {
    codec_for(job.output, info.format, info.alpha)
}

/// The folder outputs of `source` go to.
pub fn out_dir(job: &Job, source: &Path) -> PathBuf {
    source.parent().unwrap_or(Path::new(".")).join(&job.out_dir)
}

/// The output name without extension and number.
pub fn base_name(
    job: &Job,
    source: &Path,
    index: usize,
    size: (u32, u32),
    date: Option<Stamp>,
) -> Result<String, String> {
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let ext = source
        .extension()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    naming::render(
        &job.pattern,
        &Fields {
            name: &stem,
            ext: &ext,
            index,
            width: size.0,
            height: size.1,
            date,
        },
    )
}

/// The output path of every file, in order (`index` is its 1-based
/// position): free on disk and not taken by an earlier file.
pub fn targets(job: &Job, files: &[(&Path, &SourceInfo)]) -> Vec<Result<PathBuf, String>> {
    let claims = Claims::default();
    files
        .iter()
        .enumerate()
        .map(|(i, (path, info))| {
            let base = base_name(job, path, i + 1, out_size(job, info), info.date)?;
            Ok(claims.claim(&out_dir(job, path), &base, codec(job, info).extension()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::Size;

    fn info(stored: (u32, u32), exif: u16) -> SourceInfo {
        SourceInfo {
            format: Format::Jpeg,
            stored,
            orientation: Orientation::from_exif(exif),
            date: Some(Stamp {
                year: 2025,
                month: 6,
                day: 1,
                hour: 18,
                minute: 5,
                second: 0,
            }),
            alpha: false,
        }
    }

    #[test]
    fn names_in_order_with_numbers_for_collisions() {
        let dir = std::env::temp_dir().join(format!("blinzel-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("out")).unwrap();
        std::fs::write(dir.join("out/lauf_2025-06-01 18-05.jpg"), b"x").unwrap();
        let job = Job {
            size: Size::Width(600),
            pattern: "lauf_{date:YYYY-MM-DD hh-mm}".into(),
            out_dir: PathBuf::from("out"),
            ..Job::default()
        };
        let (a, b) = (dir.join("a.jpg"), dir.join("b.JPG"));
        let (ia, ib) = (info((6000, 4000), 1), info((6000, 4000), 6));
        let t = targets(&job, &[(&a, &ia), (&b, &ib)]);
        assert_eq!(
            t,
            [
                Ok(dir.join("out/lauf_2025-06-01 18-05 (2).jpg")),
                Ok(dir.join("out/lauf_2025-06-01 18-05 (3).jpg")),
            ]
        );
        let sized = Job {
            pattern: "{name}_{w}x{h}_{n:2}".into(),
            ..job
        };
        let t = targets(&sized, &[(&a, &ia), (&b, &ib)]);
        assert_eq!(
            t,
            [
                Ok(dir.join("out/a_600x400_01.jpg")),
                Ok(dir.join("out/b_600x900_02.jpg")),
            ],
            "portrait by EXIF"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
