---
status: todo
created: 2026-11-02
size: M
platforms: all
design: [blinzel.md §2, blinzel.md §3, blinzel.md §5, blinzel.md §9, blinzel.md §12 (M1, M5)]
---

# Image formats and decoder choice

## Context

The viewer opens every common still-image format, camera raws included (`blinzel.md` §2, "Formats"). Each format gets a native decoder in `blinzel-core` where the benchmark gate showed it to be faster than the system decoder; everything else, and whatever a native decoder rejects, goes to the system decoder: WIC on Windows, ImageIO on macOS. Linux has none; camera raws are shown there through their embedded JPEG previews (`blinzel.md` §2, §3 gate table, §12 M5).

Before any pixels are decoded, a probe reads the size, the orientation, the embedded previews and the colour profile from the header, so the window can be sized, the decode size picked and a preview shown first (`blinzel.md` §5). This spec covers format detection, the probe, the native still-image decoders and the rules that choose between native and system decoders.

## Scope

### In scope

- Format detection by magic bytes.
- The probe: format, stored size, orientation, embedded previews (the colour profile only as far as which formats read one — see Out of scope).
- Native still-image decoders: JPEG through libjpeg-turbo, PNG, GIF (first frame), BMP, WebP, JPEG XL, TIFF, AVIF, HEIC (Cargo feature `heic`), camera-raw probing (`raw.rs`).
- Embedded camera previews and when the loader shows them.
- Which decoder runs first on Windows, macOS and Linux.

### Out of scope

- The own parallel JPEG decoder `blinzel-jpeg` and when it replaces libjpeg-turbo — because `jpeg-decoder.md` covers it.
- Animations (GIF, WebP, JPEG XL frames, timing, budget) — because they are their own feature.
- Reading and applying colour profiles in detail — because `color-management.md` covers it.
- Thumbnails, prefetch and the scheduler — because they only call the probe and decoders specified here.
- Developing raw sensor data natively — because it is not implemented; the system decoder develops it.
- Benchmark numbers behind the gate decisions — because they are measurements (`blinzel.md` §3, §15), not behavior.

## Acceptance criteria

- [ ] AC-01 (always): The viewer recognizes a file's format from its first bytes, independent of the file name: `FF D8 FF` is JPEG, `\x89PNG\r\n\x1a\n` PNG, `GIF89a` GIF, `BM` (at least 18 bytes) BMP, `RIFF….WEBP` WebP, an `ftyp` box with brand `heic` HEIF, with brand `avif` AVIF, with brand `crx ` a camera raw, `II*\0` TIFF, `FF 0A` JPEG XL; anything else, an empty file included, is unknown.
- [ ] AC-02 (error): If a file's format is unknown or has no native decoder, a decoder panics, or the image has more than 2^30 pixels or a zero side, then the probe or decode fails with an error instead of crashing — "not decoded natively" (the only error that hands the file to the system decoder), "decoder crashed", "too large" or "corrupt" respectively.
- [ ] AC-03 (always): JPEG XR, ICO/CUR, DDS, PSD and QOI files are recognized by their signatures and reported as not decoded natively, as are 12-bit JPEGs, the pixels of every camera raw (TIFF-based raws with sub-IFDs, DNG and Canon CR2 included) and the probe of a raw without an embedded preview, so only the system decoder shows them.
- [ ] AC-04 (event): When a JPEG is decoded on one thread, then a full or minimum-size request yields YCbCr planes (4:4:4, 4:2:2, 4:2:0, 4:4:0) or a gray plane at the smallest DCT scale n/8 that covers the requested size, and a region request yields that full-resolution rectangle as BGRA, matching the same rectangle of a full decode (PSNR over 40 dB); a region outside the image is an error and leaves the decoder usable.
- [ ] AC-05 (error): If a JPEG's entropy-coded data is truncated, then decoding still yields an image; if its header is truncated, then decoding fails with an error.
- [ ] AC-06 (always): PNGs of every colour type decode to the same pixels: gray to a gray plane, RGB, gray + alpha, RGBA and palette images (with `tRNS` transparency) to RGBA with straight alpha, 16-bit samples reduced to 8 bits; a file cut short is an error.
- [ ] AC-07 (always): A GIF still is its first frame placed on the logical screen; the image is transparent wherever the frame does not cover the screen or uses the transparent palette index, and opaque otherwise.
- [ ] AC-08 (always): BMPs decode with 1-, 4- and 8-bit palettes, RLE8, 24 bits, 32 bits, 16- and 32-bit bit fields (alpha only with an alpha mask), bottom-up and top-down rows; a missing signature, zero width, truncated pixel data or an unsupported compression is an error.
- [ ] AC-09 (always): A WebP still decodes from lossy (VP8) and lossless (VP8L) data, with alpha where the file has it; an animated WebP shows its first frame as the still; a lossless or animated file is never classified as a lossy still.
- [ ] AC-10 (always): JPEG XL stills decode to the same RGBA8 pixels as jxl-oxide's generic pixel stream — lossless gray, gray + alpha, RGB and RGBA on one or several threads, lossy RGB on one thread; an RGB image rendered in sRGB carries no colour profile; a truncated file is an error.
- [ ] AC-11 (always): TIFFs decode natively — 8-bit RGB with LZW to the same pixels for any number of strips (1 per row up to 1 for the image) and threads, 16-bit gray with Deflate rounded to 8 bits, uncompressed 8-bit RGB with associated (premultiplied) alpha converted to straight alpha — and the TIFF probe reports the orientation tag.
- [ ] AC-12 (error): If a TIFF's first image is a reduced-resolution subfile (as in camera raws) or the native reader cannot read the file, then the TIFF probe and decoder report it as not decoded natively and it counts as having no strips.
- [ ] AC-13 (always): AVIFs — 8-bit 4:2:0 and 10-bit limited-range 4:4:4 BT.709 — decode on one or several threads; an alpha item makes the image transparent where it says so; grid images are stitched; `clap` crops the pixels and the probed size; `irot`/`imir` become the probed orientation instead of rotating pixels; nclx with Display P3 primaries yields a Display P3 profile.
- [ ] AC-14 (error): If an AVIF uses an HDR transfer (PQ), or its bytes are truncated or corrupted anywhere, then probe and decode report an error or "not decoded natively" without panicking.
- [ ] AC-15 (option): Where the build has the `heic` feature, an 8-bit HEIC is probed (size, orientation already applied) and decoded natively on one or several threads, with straight alpha where it has an alpha plane; a truncated file is reported as not decoded natively.
- [ ] AC-16 (option, CI: macos): Where the build has no `heic` feature, probing and decoding a HEIC report it as not decoded natively.
- [ ] AC-17 (always): The probe reports a JPEG's EXIF orientation, a WebP's EXIF orientation (also behind a leading `Exif\0\0`) and a JPEG XL header orientation; PNG, GIF and BMP are always upright.
- [ ] AC-18 (always): For a camera raw the probe reports the orientation, the embedded JPEG previews largest first — from TIFF-based raws (IFD chain, EXIF IFD, sub-IFDs, JPEG strips), RAF and CR3 — and the developed size: the EXIF size where it is at least the largest preview's width and within 3 % of its aspect ratio, else the largest preview's size; lossless sensor JPEGs and previews outside the file are never listed, and malformed data yields no previews instead of a crash.
- [ ] AC-19 (always): For a JPEG the probe lists the EXIF thumbnail (IFD1) and the MPF images that lie inside the given bytes and are 8-bit lossy JPEGs, largest first, each once; a decoded preview reports the main image's size as its full size.
- [ ] AC-20 (event): When the loader opens a camera raw whose largest preview covers what the stage needs (the screen box, or the whole image for 100 %) within 1 %, then that preview is the stage and the system decoder is not asked; otherwise, for the current image and the next one in the browsing direction, a preview that covers at least half the screen box in width or height is shown first — for a JPEG only if it has under a third of the main image's pixels — and the main decode follows; after a raw is developed, its size comes from the system decoder.
- [ ] AC-21 (state, CI: windows): While running on Windows, a file goes to the native decoders first and to WIC when they report "not decoded natively"; WIC goes first for a TIFF that has fewer than two strips or is decoded on one thread, and for a lossy WebP still once WIC's WebP codec has loaded — the first lossy WebP in a process decodes natively and starts loading the codec in the background; when WIC goes first and fails, the native decoder is tried and WIC's error is reported if both fail; `BLINZEL_DECODER=native` or `=wic` puts that decoder first wherever both can decode.
- [ ] AC-22 (state, CI: windows): While running on Windows, HEIC and AVIF files are probed and decoded by WIC first, with the colour profile taken from the HEIF container; once WIC cannot open one (no codec installed), files of that format go to the native decoder for the rest of the process.
- [ ] AC-23 (state, CI: macos): While running on macOS, HEIC goes to ImageIO first and to the native decoder only if ImageIO fails; every other file goes to the native decoders first and to ImageIO when they report "not decoded natively"; a reduced-size request to ImageIO decodes through its thumbnail path at that size.
- [ ] AC-24 (state): While running on Linux, there is no system decoder: a camera raw shows only its embedded previews, and a file the native decoders do not handle fails with the error "<format> is not decoded natively".

## Technical notes

- **Entry points** (`crates/blinzel-core/src/decode/mod.rs`): `probe(&[u8]) -> Result<Probe>`, `decode(&[u8], Request) -> Result<Image>` (one thread), `decode_with(&[u8], Request, threads)`, `decode_preview(data, &EmbeddedPreview, full, Request)`, `guard` (`catch_unwind` around every probe and decode), `MAX_PIXELS = 1 << 30`. `Request::{Full, Fit { w, h }, Region(PixelRect)}`: only JPEG decodes smaller for `Fit`; other formats decode fully, and `Region` crops a full decode to BGRA (`crop_to_bgra`). `DecodeError::{Unsupported(Format), Corrupt(String), TooLarge { width, height }, Panicked(String)}`; only `Unsupported` sends a file to the system decoder.
- **`Probe`**: `format`, stored (not oriented) `width`/`height`, `orientation`, `previews: Vec<EmbeddedPreview>` (largest first), `icc: Option<Profile>`; `is_raw()` = TIFF or raw with previews.
- **Detection** (`crates/blinzel-core/src/format.rs`): also `GIF87a`; HEIF brands `heix`, `hevc`, `hevx`, `heim`, `heis`, `mif1`, `msf1`; AVIF brand `avis`; the JPEG XL container `\0\0\0\x0cJXL \r\n\x87\n`; BigTIFF `II+\0`/`MM\0+`; raws `IIRO`/`IIRS` (ORF), `IIU\0` (RW2), `FUJIFILMCCD-RAW` (RAF); JPEG XR `II\xbc\x01`/`II\xbc\x00`; ICO/CUR `00 00 01 00`/`00 00 02 00`; `DDS `, `8BPS`, `qoif`. The major brand of `ftyp` decides, except `mif1`/`msf1`: with `avif`/`avis` and no HEVC brand among the compatible brands inside the box it is AVIF.
- **Native decoders and outputs:**

  | Format | Code | Output | Declined (→ system decoder) |
  | ------ | ---- | ------ | --------------------------- |
  | JPEG | libjpeg-turbo, TurboJPEG 3 API, own FFI (`decode/jpeg.rs`), one decompressor per thread | YCbCr planes, Gray8, BGRA (CMYK/YCCK converted, Adobe inverted) | precision ≠ 8 |
  | PNG | `png` crate, own row-wise RGB/GA → RGBA expansion (`decode/png.rs`) | RGBA8 or Gray8 | — |
  | GIF | `gif` crate (`decode/gif.rs`) | RGBA8, first frame on the screen; a 0 × 0 screen falls back to the frame's extent (probe and decode) | — |
  | BMP | own (`decode/bmp.rs`), also RLE4, OS/2 core headers, V3+ alpha masks | BGRA8 | (unsupported compressions are errors) |
  | WebP | `image-webp` (`decode/webp.rs`); `is_lossy_still`, `TINY_LOSSY` for warming WIC | RGBA8 | — |
  | JPEG XL | `jxl-oxide`, own row conversion to RGBA8 (`decode/jxl.rs`) | RGBA8, in the colour space jxl-oxide renders into | CMYK, channels smaller than the image (offset regions), float samples stored as integers |
  | TIFF | `tiff` crate, strips in parallel, one decoder per thread (`decode/tiff.rs`); `strips()` for the gate | RGBA8 or Gray8 | JPEG compression, CMYK, YCbCr, palette, planar, float, raws; every read error |
  | AVIF | own HEIF container reader + `rav1d` (`decode/avif.rs`); `container_profile`, `exif` for any HEIF | RGBA8, stored orientation, cropped | HDR (PQ, HLG), unknown essential properties, > 12 bits |
  | HEIC | `zune-heic`, feature `heic` (`decode/heif.rs`) | RGBA8 or Gray8, already oriented and cropped | whatever zune-heic rejects (10-bit, 4:4:4, AVIF in HEIF, sequences, broken files) |
  | Camera raw | own probe only (`crates/blinzel-core/src/raw.rs`) | — | sensor data always |

- **TIFF accepted natively:** first image only, 8- or 16-bit gray, gray + alpha, RGB, RGBA, interleaved, uncompressed, LZW, Deflate or PackBits; the probe also reads the ICC tag. Raws are recognized by `NewSubfileType` bit 0, a `SubIfd` tag, the DNG version tag (50706) or `CR` at byte 8.
- **Raw probe extras** (`raw.rs`, untested): Panasonic RW2's JPEG inline in tag `0x2E` and its sensor borders (tags 4–7) as the developed size; RAF's record `0x111` (cropped size) before `0x100`; CR3's `THMB`, `PRVW` and the first track's full-size JPEG; at most 32 IFDs.
- **System decoders:** `crates/blinzel-app/src/wic.rs` (`decode_any_with`, `probe_any`; scaled decode through `IWICBitmapSourceTransform` where the codec offers it; flags `WEBP_CODEC`, `HEIF_WIC_MISSING`, `AVIF_WIC_MISSING`; env `BLINZEL_DECODER`), `crates/blinzel-app/src/macos/imageio.rs` (`decode_any_with`, `probe_any`; thumbnail path for `Fit` smaller than the image). The Linux shell (`crates/blinzel-app/src/linux/shell.rs`) calls `decode::probe` and `decode::decode_with` directly. Both system decoders deliver straight-alpha BGRA.
- **Loader** (`crates/blinzel-ui/src/loader.rs`, `cpu_stage`, `covering_raw_preview`): the decode size is the stored-orientation size that covers the screen box (`layout::needed_decode_size`).
- **Builds:** `heic` is off in releases (`-p blinzel-app`), but `cargo test --workspace` (Linux and Windows CI) unifies it on through `blinzel-bench`; the macOS CI job tests `-p blinzel-core …` without `blinzel-bench`, so only there the `heic` feature is off.

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| JPEG file named `.png` | Detected and decoded as JPEG |
| AVIF whose `ftyp` major brand is `mif1` (AVIF only as a compatible brand) | Detected and decoded as AVIF (`mif1_major_brand_with_avif_compatible_is_avif`) |
| GIF whose logical screen is 0 × 0 | Probe and decoder take the first frame's extent (`zero_screen_takes_the_frame_extent`) |
| Interlaced PNG | Decoded through a full-frame buffer instead of row by row (no test with an interlaced file) |
| CMYK JPEG (Adobe, inverted) | libjpeg-turbo decodes CMYK, converted to BGRA (only the conversion is tested) |
| HEIC/AVIF on Windows without the Store codec | First file: WIC fails, native decodes; later files skip WIC |
| Raw whose EXIF size has another aspect ratio than its preview (e.g. a portrait EXIF on a landscape preview) | The preview's size is the image size |
| TIFF whose pixels the crate cannot read | Probe succeeds natively, decode goes to the system decoder (none on Linux) |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `detects_formats` (`crates/blinzel-core/src/format.rs`), CI: Linux | built | |
| AC-02 | `unknown_formats_are_unsupported`, `panics_are_contained`, `size_limits` (`crates/blinzel-core/src/decode/mod.rs`), CI: Linux | built | |
| AC-03 | `other_formats_and_raw_pixels_go_to_the_platform` (`crates/blinzel-core/src/decode/mod.rs`), `tiff_raws_list_their_jpegs` (`crates/blinzel-core/src/raw.rs`), CI: Linux | built | |
| AC-04 | `planar_decode_matches_packed`, `dct_scaling_picks_smallest_sufficient_size`, `region_equals_crop_of_full_decode`, `gray_progressive_and_restart` (`crates/blinzel-core/src/decode/jpeg.rs`), CI: Linux | built | |
| AC-05 | `truncated_files_still_decode_or_fail_cleanly` (`crates/blinzel-core/src/decode/jpeg.rs`), CI: Linux | built | |
| AC-06 | `decodes_color_types`, `interlaced_and_palette`, `corrupt_input_is_an_error` (`crates/blinzel-core/src/decode/png.rs`), CI: Linux | built | |
| AC-07 | `decodes_first_frame`, `offset_frames_and_transparency` (`crates/blinzel-core/src/decode/gif.rs`), CI: Linux | built | |
| AC-08 | `bottom_up_24_bit`, `top_down_32_bit_and_bitfields`, `palettes_1_4_8_bit`, `rle8`, `rejects_bad_files` (`crates/blinzel-core/src/decode/bmp.rs`), CI: Linux | built | |
| AC-09 | `lossy_stills`, `decodes_lossless_stills`, `plays_animations` (still = first frame) (`crates/blinzel-core/src/decode/webp.rs`), CI: Linux | built | |
| AC-10 | `lossless_round_trips`, `lossy_matches_the_generic_stream`, `rejects_broken_files` (`crates/blinzel-core/src/decode/jxl.rs`), CI: Linux | built | |
| AC-11 | `strips_decode_in_parallel`, `gray16_and_premultiplied_alpha` (`crates/blinzel-core/src/decode/tiff.rs`), CI: Linux | built | |
| AC-12 | `raws_go_to_the_platform` (`crates/blinzel-core/src/decode/tiff.rs`), CI: Linux | built | |
| AC-13 | `decodes_8_and_10_bit`, `alpha_plane`, `irot_and_imir_become_the_orientation`, `clap_crops`, `grid_tiles_are_stitched`, `display_p3_gets_a_profile` (`crates/blinzel-core/src/decode/avif.rs`), CI: Linux | built | |
| AC-14 | `hdr_and_broken_files_are_refused_without_panicking` (`crates/blinzel-core/src/decode/avif.rs`), CI: Linux | built | |
| AC-15 | `probe_and_decode_rgb`, `alpha_is_straight`, `broken_files_go_to_the_platform` (`crates/blinzel-core/src/decode/heif.rs`), CI: Linux (feature unified by `cargo test --workspace`) | built | |
| AC-16 | `heic_needs_the_feature` (`crates/blinzel-core/src/decode/mod.rs`), CI: macOS only (the other jobs build with `heic`) | built | |
| AC-17 | `probe_reports_the_orientation` (`crates/blinzel-core/src/decode/mod.rs`), `exif_orientation_is_probed` (`crates/blinzel-core/src/decode/webp.rs`), CI: Linux | built | |
| AC-18 | `tiff_raws_list_their_jpegs`, `image_size_trusts_exif_only_with_the_previews_aspect`, `raf_header_and_records`, `cr3_boxes`, `garbage_yields_nothing` (`crates/blinzel-core/src/raw.rs`), CI: Linux | built | |
| AC-19 | `jpeg_previews_from_exif_and_mpf` (`crates/blinzel-core/src/decode/mod.rs`), CI: Linux | built | |
| AC-20 | `a_raw_preview_that_covers_the_stage_is_the_stage_and_the_system_decoder_is_not_asked`, `a_raw_preview_that_covers_half_the_screen_box_shows_first_and_the_developed_size_wins`, `a_jpeg_preview_shows_first_only_with_under_a_third_of_the_pixels`, `the_next_image_gets_its_preview_first_and_later_ones_do_not` (`crates/blinzel-ui/src/app_format_tests.rs`), CI: Linux | built | |
| AC-21 | — | open | |
| AC-22 | — | open | |
| AC-23 | — | open | |
| AC-24 | — | open | |

## Decisions

- 2026-11-02: Extracted from the code (`/spec-extract`). Colour-profile ACs live in `color-management.md`; this spec only lists which formats the probe reads a profile from (Technical notes).
- 2026-11-02: AC-03, AC-17 and AC-19 described behavior the code already had, so their tests were green at once; each was checked by breaking the code it covers (the 12-bit check, the CR2/sub-IFD/DNG checks, the ICO signature, the EXIF/WebP/JPEG XL orientation, the 8-bit/lossless/duplicate filters, the sort and `full_width`), which turned it red.
- 2026-11-02: The JPEG XL orientation fixture `crates/blinzel-core/testdata/orient6-4x2.jxl` (84 bytes, container with an `Exif` box) was made with `cjxl -d 0` (libjxl 0.7) from a 4 × 2 PNG with an `eXIf` orientation 6; `jxlinfo` shows orientation 6 in the codestream header.
- 2026-11-02: An AVIF with major brand `mif1`/`msf1` counts as AVIF when its compatible brands (inside the `ftyp` box) name `avif`/`avis` and no HEVC brand; a file naming both stays HEIF, since zune-heic or the system decoder can then still try it.
- 2026-11-02: AC-20 described behavior the loader already had, so its tests were green at once; each was checked by breaking the code it covers (the 1 % tolerance, the raw-only covering preview, the developed size from the decoder, the half-screen-box and one-third rules, the preview flag of the next image), which turned it red. The harness's stand-in system decoder (`Harness::set_system_decoder`) develops what the native decoders refuse to a given size; the raws are RAFs (`testkit::raf`) the native probe reads.
- 2026-11-02: The image one step against the browsing direction gets its preview first too (`wanted_jobs`, browsing AC-06); AC-20 names only the next one, so its test checks the next image and one two ahead.
