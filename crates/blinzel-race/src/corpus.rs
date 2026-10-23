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
