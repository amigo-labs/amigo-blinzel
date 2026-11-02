//! The race corpus: `count` camera-sized JPEGs in one folder, each with a
//! colour signature, named so every viewer sorts them the same way.

use std::path::{Path, PathBuf};

use blinzel_core::decode::jpeg::{EncodeOptions, encode_rgb};

use crate::signature;

pub struct Spec {
    pub count: usize,
    pub width: u32,
    pub height: u32,
    pub quality: u8,
}

impl Default for Spec {
    /// 24 MP, the typical camera of the last decade.
    fn default() -> Spec {
        Spec {
            count: 24,
            width: 6000,
            height: 4000,
            quality: 92,
        }
    }
}

pub fn file_name(i: usize) -> String {
    format!("race-{i:03}.jpg")
}

/// The image files of a corpus folder in order.
pub fn files(dir: &Path, count: usize) -> Vec<PathBuf> {
    (0..count).map(|i| dir.join(file_name(i))).collect()
}

/// The image files of a corpus folder, or why `run` cannot measure with
/// them: the first missing file and the command that writes it.
#[cfg(any(windows, target_os = "macos", test))]
pub fn check(dir: &Path, count: usize) -> Result<Vec<PathBuf>, String> {
    let files = files(dir, count);
    match files.iter().find(|f| !f.is_file()) {
        Some(missing) => Err(format!(
            "{} missing: run `blinzel-race corpus {}` first",
            missing.display(),
            dir.display()
        )),
        None => Ok(files),
    }
}

/// Writes the corpus; existing files are kept (generation takes a while).
pub fn write(dir: &Path, spec: &Spec) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (i, path) in files(dir, spec.count).iter().enumerate() {
        if path.exists() {
            continue;
        }
        let px = signature::image(i, spec.width, spec.height);
        let opts = EncodeOptions {
            quality: spec.quality,
            ..Default::default()
        };
        let data = encode_rgb(&px, spec.width, spec.height, opts).map_err(|e| e.to_string())?;
        std::fs::write(path, &data).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("{}: {} KiB", path.display(), data.len() / 1024);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("blinzel-race-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn writes_signature_jpegs_and_keeps_existing_files() {
        let d = Spec::default();
        assert_eq!(
            (d.count, d.width, d.height, d.quality),
            (24, 6000, 4000, 92)
        );
        let dir = temp("corpus");
        let spec = Spec {
            count: 8,
            width: 96,
            height: 64,
            ..Spec::default()
        };
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("race-003.jpg"), b"keep").unwrap();
        write(&dir, &spec).unwrap();

        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, (0..8).map(file_name).collect::<Vec<_>>());
        assert_eq!(names[..2], ["race-000.jpg", "race-001.jpg"]);
        assert_eq!(std::fs::read(dir.join("race-003.jpg")).unwrap(), b"keep");
        for i in (0..8).filter(|&i| i != 3) {
            // The JPEG of signature image `i` at the spec's size and quality 92.
            let px = signature::image(i, 96, 64);
            let opts = EncodeOptions {
                quality: 92,
                ..Default::default()
            };
            let want = encode_rgb(&px, 96, 64, opts).unwrap();
            assert_eq!(std::fs::read(dir.join(file_name(i))).unwrap(), want, "{i}");
            // Around base colour `i mod 6`, with texture rather than a flat fill.
            let samples: Vec<[u8; 3]> = px.as_chunks::<3>().0.iter().step_by(31).copied().collect();
            assert_eq!(
                signature::classify(&samples),
                Some(i % signature::COLORS.len())
            );
            let red: Vec<f64> = px.iter().step_by(3).map(|&v| f64::from(v)).collect();
            let mean = red.iter().sum::<f64>() / red.len() as f64;
            let var = red.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / red.len() as f64;
            assert!(var.sqrt() > 8.0, "image {i} is flat: sd {}", var.sqrt());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn check_names_the_first_missing_file() {
        let dir = temp("check");
        let spec = Spec {
            count: 3,
            width: 16,
            height: 16,
            ..Spec::default()
        };
        write(&dir, &spec).unwrap();
        assert_eq!(check(&dir, 3), Ok(files(&dir, 3)));
        let e = check(&dir, 4).unwrap_err();
        let missing = dir.join("race-003.jpg");
        assert_eq!(
            e,
            format!(
                "{} missing: run `blinzel-race corpus {}` first",
                missing.display(),
                dir.display()
            )
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
