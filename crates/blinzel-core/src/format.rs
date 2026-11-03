//! Format detection by magic bytes, and the file extensions the viewer lists.

/// Image container formats we recognise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Jpeg,
    Png,
    Gif,
    Bmp,
    Tiff,
    WebP,
    Heif,
    Avif,
    Jxl,
    Ico,
    Dds,
    Psd,
    Qoi,
    JpegXr,
    /// Camera raw that is not plain TIFF (CR3, RAF, ORF, RW2, …).
    Raw,
    Unknown,
}

impl Format {
    /// Decoded by `blinzel-core` itself (HEIF only with the `heic`
    /// feature); everything else goes to the platform fallback (WIC on
    /// Windows, ImageIO on macOS).
    pub fn is_native(self) -> bool {
        matches!(
            self,
            Format::Jpeg
                | Format::Png
                | Format::Gif
                | Format::Bmp
                | Format::WebP
                | Format::Jxl
                | Format::Tiff
                | Format::Avif
        ) || (self == Format::Heif && cfg!(feature = "heic"))
    }
}

/// Detects the format from the first bytes of a file (16 are enough).
pub fn detect(d: &[u8]) -> Format {
    let starts = |m: &[u8]| d.len() >= m.len() && &d[..m.len()] == m;
    if starts(&[0xFF, 0xD8, 0xFF]) {
        Format::Jpeg
    } else if starts(b"\x89PNG\r\n\x1a\n") {
        Format::Png
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Format::Gif
    } else if starts(b"BM") && d.len() >= 18 {
        Format::Bmp
    } else if starts(b"RIFF") && d.len() >= 12 && &d[8..12] == b"WEBP" {
        Format::WebP
    } else if d.len() >= 12 && &d[4..8] == b"ftyp" {
        // A generic `mif1`/`msf1` major brand: the compatible brands after
        // the minor version (inside the box) say whether it is AVIF.
        let size = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) as usize;
        let compatible = d.get(16..size.min(d.len())).unwrap_or_default();
        let has = |brands: &[&[u8; 4]]| {
            compatible
                .as_chunks::<4>()
                .0
                .iter()
                .any(|b| brands.contains(&b))
        };
        let heic: &[&[u8; 4]] = &[b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis"];
        match &d[8..12] {
            b"avif" | b"avis" => Format::Avif,
            b"mif1" | b"msf1" if has(&[b"avif", b"avis"]) && !has(heic) => Format::Avif,
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1" | b"msf1" => {
                Format::Heif
            }
            b"crx " => Format::Raw,
            _ => Format::Unknown,
        }
    } else if starts(&[0xFF, 0x0A]) || starts(b"\0\0\0\x0cJXL \r\n\x87\n") {
        Format::Jxl
    } else if starts(b"II\xbc\x01") || starts(b"II\xbc\x00") {
        Format::JpegXr
    } else if starts(b"IIRO") || starts(b"IIRS") || starts(b"IIU\0") || starts(b"FUJIFILMCCD-RAW") {
        Format::Raw
    } else if starts(b"II*\0") || starts(b"MM\0*") || starts(b"II+\0") || starts(b"MM\0+") {
        // Includes TIFF-based raws (NEF, ARW, DNG, CR2, PEF, …).
        Format::Tiff
    } else if starts(&[0, 0, 1, 0]) || starts(&[0, 0, 2, 0]) {
        Format::Ico
    } else if starts(b"DDS ") {
        Format::Dds
    } else if starts(b"8BPS") {
        Format::Psd
    } else if starts(b"qoif") {
        Format::Qoi
    } else {
        Format::Unknown
    }
}

/// Extensions shown in the folder list (lower case, without dot).
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "jif", "png", "apng", "gif", "bmp", "dib", "tif", "tiff", "webp",
    "heic", "heif", "hif", "avif", "jxl", "ico", "cur", "dds", "psd", "qoi", "jxr", "wdp", "hdp",
    // Camera raw
    "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "orf", "rw2", "raf", "pef",
    "srw", "x3f", "erf", "kdc", "mrw", "3fr", "iiq", "rwl",
];

/// Whether a file name has an image extension (ASCII case-insensitive).
pub fn has_image_extension(name: &str) -> bool {
    let Some(dot) = name.rfind('.') else {
        return false;
    };
    let ext = &name[dot + 1..];
    ext.len() <= 5 && IMAGE_EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats() {
        assert_eq!(detect(&[0xFF, 0xD8, 0xFF, 0xE0]), Format::Jpeg);
        assert_eq!(detect(b"\x89PNG\r\n\x1a\n...."), Format::Png);
        assert_eq!(detect(b"GIF89a.."), Format::Gif);
        assert_eq!(
            detect(b"BM\x36\0\0\0\0\0\0\0\x36\0\0\0\x28\0\0\0"),
            Format::Bmp
        );
        // BMP needs its 14-byte file header and the 4-byte DIB header size.
        assert_eq!(
            detect(b"BM\x36\0\0\0\0\0\0\0\x36\0\0\0\x28\0\0"),
            Format::Unknown,
            "17 bytes"
        );
        assert_eq!(detect(b"RIFF\0\0\0\0WEBPVP8 "), Format::WebP);
        assert_eq!(detect(b"\0\0\0\x18ftypheic\0\0\0\0"), Format::Heif);
        assert_eq!(detect(b"\0\0\0\x18ftypavif\0\0\0\0"), Format::Avif);
        assert_eq!(detect(b"\0\0\0\x18ftypcrx \0\0\0\0"), Format::Raw);
        // A `mif1` major brand is decided by the compatible brands.
        assert_eq!(detect(b"\0\0\0\x18ftypmif1\0\0\0\0mif1avif"), Format::Avif);
        assert_eq!(detect(b"\0\0\0\x18ftypmif1\0\0\0\0mif1heic"), Format::Heif);
        // Brands after the end of the `ftyp` box do not count.
        assert_eq!(detect(b"\0\0\0\x14ftypmif1\0\0\0\0mif1avif"), Format::Heif);
        // A file naming both AVIF and HEVC stays HEIF (Decisions), whichever
        // HEVC brand it is.
        for hevc in [b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis"] {
            let d = [b"\0\0\0\x1cftypmif1\0\0\0\0mif1avif".as_slice(), hevc].concat();
            assert_eq!(detect(&d), Format::Heif, "{hevc:?}");
        }
        // `avis` counts like `avif`, `msf1` like `mif1`.
        assert_eq!(detect(b"\0\0\0\x18ftypmif1\0\0\0\0mif1avis"), Format::Avif);
        assert_eq!(detect(b"\0\0\0\x18ftypmsf1\0\0\0\0msf1avis"), Format::Avif);
        assert_eq!(detect(b"\0\0\0\x14ftypmsf1\0\0\0\0msf1"), Format::Heif);
        // The minor version is no compatible brand.
        assert_eq!(detect(b"\0\0\0\x14ftypmif1avifmif1"), Format::Heif);
        // An `ftyp` box too short for its major brand is unknown.
        assert_eq!(detect(b"\0\0\0\x18ftypavi"), Format::Unknown);
        assert_eq!(detect(b"GIF87a.."), Format::Gif);
        assert_eq!(detect(b"\0\0\0\x0cJXL \r\n\x87\n\0\0\0\0"), Format::Jxl);
        // TIFF in either byte order, and BigTIFF.
        for tiff in [
            b"II*\0\x08\0\0\0",
            b"MM\0*\0\0\0\x08",
            b"II+\0\x08\0\0\0",
            b"MM\0+\0\x08\0\0",
        ] {
            assert_eq!(detect(tiff), Format::Tiff, "{tiff:?}");
        }
        assert_eq!(detect(&[0xFF, 0x0A, 0, 0]), Format::Jxl);
        assert_eq!(detect(b"hello"), Format::Unknown);
        assert_eq!(detect(b""), Format::Unknown);
        // Near misses: each signature with one byte off, 16 bytes long.
        for near in [
            &b"\xFF\xD8\x00\xE0\0\0\0\0\0\0\0\0\0\0\0\0"[..],
            b"\x89PNG\r\n\x1a\0\0\0\0\x0dIHDR",
            b"GIF89b\0\0\0\0\0\0\0\0\0\0",
            b"BN\x36\0\0\0\0\0\0\0\x36\0\0\0\x28\0\0\0",
            b"RIFF\0\0\0\0WAVEfmt ",
            b"\0\0\0\x18ftyqheic\0\0\0\0",
            b"II*\x01\x08\0\0\0\0\0\0\0\0\0\0\0",
            b"MM\x01*\0\0\0\x08\0\0\0\0\0\0\0\0",
            b"\xFF\x0B\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        ] {
            assert_eq!(detect(near), Format::Unknown, "{near:?}");
        }
    }

    /// `is_native` names exactly the formats `decode` has a decoder for.
    #[test]
    fn native_formats_match_the_decoders() {
        use Format::*;
        for f in [Jpeg, Png, Gif, Bmp, WebP, Jxl, Tiff, Avif] {
            assert!(f.is_native(), "{f:?}");
        }
        assert_eq!(Heif.is_native(), cfg!(feature = "heic"));
        for f in [Ico, Dds, Psd, Qoi, JpegXr, Raw, Unknown] {
            assert!(!f.is_native(), "{f:?}");
        }
    }

    #[test]
    fn extensions() {
        assert!(has_image_extension("IMG_0001.JPG"));
        assert!(has_image_extension("a.b.webp"));
        assert!(has_image_extension("x.Cr3"));
        assert!(!has_image_extension("notes.txt"));
        assert!(!has_image_extension("jpg"));
        assert!(!has_image_extension("archive.jpg.zip"));
    }
}
