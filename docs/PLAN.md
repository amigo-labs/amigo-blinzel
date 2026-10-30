# amigo-blinzel: Plan

This document is the binding plan. At the end, every item is checked against the implementation.

**Status legend:**
- ✅ implemented and verified
- 🟡 implemented, verification (CI or manually on Windows) pending
- ⬜ open

As of: 2026-10-25. The current state of work for the next session is in [section 16](#16-handover-next-steps).

---

## 1. Goal

IrfanView and ImageGlass are too slow. amigo-blinzel is meant to be measurably faster than any alternative at **opening**, **flipping through images** and **thumbnails**, and to prove it on a website with reproducible benchmarks.

**Guiding principle: get the most out of it.** No library is adopted unchecked. Every hot path is measured, and where an in-house build is faster, we build it ourselves.

## 2. Decisions

| Topic | Decision |
|---|---|
| Language | Rust |
| Platform | **Windows 11 24H2 or later** (build 26100+, x64 and ARM64; only versions with Microsoft support), **macOS 14 or later** (one universal app for Apple Silicon and Intel) and **Linux** (x64 and ARM64, Wayland and X11, glibc 2.35 or later). Per platform, one native shell around the same interface (`blinzel-ui`): Win32 + Direct3D 11, AppKit + Metal, winit + wgpu (Vulkan, GL ES). |
| Formats | all incl. RAW; native decoders via benchmark gate, otherwise the system decoder (WIC on Windows, ImageIO on macOS; Linux has none, RAW there via embedded previews) |
| Thumbnails | filmstrip + grid browser, both off by default |
| Resident mode | optional (tray, kept warm) |
| UI | frameless, no toolbars; ☰ at the top left, min/max/close at the top right, everything else in the menu. macOS: the system window buttons on the left, ☰ on the right, all commands also in the menu bar |
| View modes | fit window to image (original size), fit image to window, 100 % |
| Languages | English, German, Spanish, French, Italian, Japanese, Portuguese (Brazil), Russian, Simplified Chinese |
| Selection | rectangle; Ctrl+C copies; Ctrl+X = copy + crop view |
| Saving | image files are never modified; **optionally save a cutout** (Ctrl+S, "Save as") |
| Batch processing | **resize many images**, in the app only (no CLI): JPEG, PNG, WebP, AVIF or same as source; keep metadata, rename by pattern, crop to an aspect ratio, sharpen |
| Mouse | context-dependent: image fits → left drag selects; image larger → left drag pans, Shift+drag selects |
| Installer | no installer framework; Windows: the exe installs itself; Linux: `blinzel --install` per user; macOS: drag the app into "Applications" |
| Signature | **no code signing** (no Authenticode, no Apple notarization); proof of origin is our own Ed25519 signature of the checksums |
| Updates | **silently in the background**: check at most 1× a day, download the signed update, verify it, active on the next start |
| Website | **Cloudflare Workers** (static assets), with competitive comparison and benchmarks |

## 3. Methodology: benchmark gate "library vs. in-house build"

`crates/blinzel-bench` generates a deterministic corpus and measures every candidate. The winner gets built in; the library remains the yardstick and pixel reference.

| Hot path | Yardstick | In-house lever | Status |
|---|---|---|---|
| JPEG decode | libjpeg-turbo, zune-jpeg | own parallel decoder (M3) | ✅ `blinzel-jpeg` for the visible image: sequential 4.1×, progressive 1.7× turbo (16 threads), byte-identical; single-thread and regions stay with turbo |
| Path to the GPU | RGBA from the decoder | YCbCr planes, color conversion in the shader | ✅ measured: 24 MP 118 instead of 147 ms, 45 MP 190 instead of 410 ms |
| PNG | zune-png, png | `png` + own fused RGBA expansion | ✅ 1.9× (photo) and 3.2× (screenshot) faster than zune-png |
| Memory | system allocator | committed buffer pool | ✅ 96 MB: 0.6 ms instead of 40 ms |
| Thumbnail scaling | Windows thumbnail provider (`IShellItemImageFactory`) | DCT 1/8 + area averaging per YCbCr plane | ✅ 200 12-MP JPEGs, 16 threads: 287/s instead of 18/s |
| Thumbnail storage | Windows thumbnail cache (`thumbcache_*.db`) | own pack file | ✅ 200 from the cache: 23.6 ms instead of 432 ms (CPU side, without atlas upload) |
| Directory | FindFirstFileExW (`LARGE_FETCH`, `InfoBasic`) | `GetFileInformationByHandleEx(FileFullDirectoryInfo)` (what `NtQueryDirectoryFile` returns) | ✅ measured, stays with FindFirstFileExW: 10k entries 7.7–8.0 ms vs. 6.8–7.3 ms (64 KB buffer) or −8 to +18 % (256 KB), on average under 10 %; sorting 2.5 ms. Target ≤ 25 ms met. |
| WebP | WIC (Store extension) | `image-webp` (pure Rust, MIT/Apache) | ✅ measured (12/24 MP, same pixels): lossless 210 instead of 303 ms → native. For lossy, WIC with the codec loaded is about 20 % faster (12 MP 411–454 instead of 485–541 ms, 24 MP 604–649 instead of 751–812 ms), but its first call in the process costs 200–400 ms. Hence: the first lossy WebP native, the WIC codec loads in the background meanwhile, the following ones via WIC. Animations native. |
| JPEG XL | WIC (Store extension "JPEG XL Image Extension") | `jxl-oxide` (pure Rust, MIT/Apache) with our own row-by-row conversion to RGBA8 instead of the generic pixel stream (which cost 225–800 ms, as much as decoding) | ✅ native, jxl crates with `opt-level = "s"`: exe 2.9 → 4.6 MB (with `opt-level = 3` 5.1 MB, `"z"` 4.4 MB and considerably slower). Measured in the same run (12/24 MP, distance 1 and lossless): 12 MP first image 270–353 instead of 514–559 ms, afterwards 259–265 instead of 319–364 ms; 24 MP 523–535 instead of 600–662 ms; lossless on par (581–707 vs. 671–692 ms). In addition: no Store codec needed, color profile and orientation from the header. Channels with an offset region and CMYK stay with WIC. |
| TIFF | WIC | `tiff` (pure Rust, MIT) with strips in parallel on the P-cores, each thread with its own decoder over the same bytes | ✅ measured (12/24 MP, 8-bit RGB; files with 600 strips as written by libtiff): LZW 12 MP 66–115 instead of 226–512 ms, 24 MP 156–217 instead of 513–1091 ms, Deflate 41–47 instead of 128–156 ms, uncompressed on par. With only one strip (e.g. from ffmpeg) or one thread, WIC is equally fast or faster: then WIC first. Native only grayscale/RGB(A) with 8/16 bits, interleaved, without JPEG compression; TIFF-based raws (preview subfile, SubIFDs, DNG, CR2) go to WIC. Exe +190 KB. The probe now reads orientation and color profile for every simple TIFF. |
| RAW | WIC (Microsoft Raw Image Decoder) | own probe (`raw.rs`): orientation, size (EXIF, RAF records, RW2 sensor borders) and the embedded JPEGs from TIFF raws (NEF, CR2, ARW, DNG, PEF, RW2), RAF and CR3, under 1 ms per file | ✅ measured (CC0 samples from raw.pixls.us, open to image): CR2 2793 → 96 ms, CR3 3571 → 138 ms, NEF 2001 → 59 ms, RW2 5855 → 69 ms, RAF 1628 → 91 ms, iPhone DNG 745 → 91 ms. A preview that covers what is needed (the screen; for 100 % the whole image) is the stage; otherwise it appears first and WIC develops afterwards (ARW: preview 1616 × 1080, 1435 ms). Without a preview (ORF), WIC as before. After development, the probe takes over the size from WIC. |
| HEIC | WIC (Store extension "HEVC Video Extensions", often hardware decode) | `zune-heic` (pure Rust, own HEVC decoder, MIT/Apache/Zlib): 8-bit 4:2:0 and monochrome, grid tiles in parallel, `clap`/`irot`/`imir` applied, alpha (premultiplied is divided out) | 🟡 Linux, 4 vCPU, `blinzel-bench run`: 12 MP (one tile, heif-enc) 340–700 ms, 24 MP 700–730 ms. Not yet measured against WIC (`BLINZEL_DECODER=native` or `=wic` with `blinzel.exe --bench`). Because of the HEVC patents only with the Cargo feature `heic` (off by default, not included in releases): then WIC goes first where the HEVC codec is installed, and without it `zune-heic` decodes. Without the feature, HEIC stays with WIC. Probe and decode take the same path, because WIC delivers unrotated with orientation, `zune-heic` rotated. The color profile is read by our own HEIF container reader (also without the feature). 10-bit, 4:2:2/4:4:4 and sequences stay with WIC. Note: HEVC is patent-encumbered ([section 14](#14-open-items-on-your-side)). |
| AVIF | WIC (Store extension "AV1 Video Extension") | `rav1d` (Rust port of dav1d, BSD-2; assembly only on x86-64, ARM64 with the Rust fallbacks, because MSVC does not build dav1d's ARM assembly) and our own HEIF container reader (`decode::avif`): items, grids, alpha (also premultiplied), `clap` as crop, `irot`/`imir` as orientation for the GPU, `colr` (ICC; nclx with P3 yields a generated profile). 8–12 bits, 4:0:0 to 4:4:4, all common CICP matrices and both value ranges, bilinear chroma interpolation | 🟡 Linux, 4 vCPU, `blinzel-bench run` (avifenc, quality 60): 12 MP 383 ms (1 thread) or 205 ms (4), 24 MP 730 or 267 ms. Pixels against libavif (`avifdec`): at most 4 levels of deviation, mostly 0–2; orientation and crop as in libheif. Not yet measured against WIC; until then like HEIC: WIC first where the AV1 codec is installed, otherwise native. HDR (PQ, HLG) and unknown essential properties stay with WIC. |
| Batch scaling (M9) | `fast_image_resize` (MIT/Apache, SIMD) with the same math: sRGB → linear U16, Lanczos3, alpha premultiplied | own separable Lanczos3 in `f32`, linear and premultiplied, in bands of 64 rows (small intermediate buffer), source rows read directly from the YCbCr planes | ✅ `blinzel-bench resize`, one thread: 24 MP → 1600 px 309 instead of 506 ms, → 400 px 201 instead of 438 ms, 12 MP with alpha → 2048 px 228 instead of 308 ms; only when enlarging (2 → 4 MP) is the library faster (148 vs. 104 ms). Differences only at hard edges in dark areas (mean 0.04–0.2 levels) → in-house build. |
| SIMD levels | baseline x86-64-v2 | runtime dispatch per kernel (`kernels.rs`: SSE4.2, AVX, AVX2+FMA) | ✅ measured: no level gains ≥ 10 % on a hot path, hence no dispatch ([section 15](#simd-levels)) |

**Insight from the measurements:** DCT scaling brings less than expected (24 MP: 1/8 = 74 ms, full = 118 ms). Huffman decoding dominates. The **parallel Huffman decoder (M3)** is therefore the biggest remaining lever; progressive JPEGs (approx. 320 ms) need it in particular.

### Own JPEG decoder `blinzel-jpeg` (M3)

1. **Parallelism within one image:**
   - Restart segments in parallel.
   - Speculative parallel Huffman decoding via self-synchronization (Weißenberger & Schmidt), DC correction via prefix sum.
   - Pipeline of entropy decode and IDCT.
2. **Less work:**
   - DCT scaling.
   - DC-only mode for 1/8 and thumbnails (AC only skipped).
   - YCbCr output.
   - AVX2/NEON IDCT.
3. Write **directly into the target memory**.

Order of the stages:
- sequential baseline
- restart-parallel
- parallel Huffman
- DC-only
- progressive

**Design (2026-10-23):**
- **Reference:** The output is byte-identical with libjpeg-turbo (islow IDCT, for 1/2, 1/4, 1/8 the reduced IDCTs from `jidctred.c`). Tests compare every plane 1:1 with turbo.
- **Output:** YCbCr planes without upsampling, directly into buffers provided by the caller (buffer pool). Every plane is padded to whole MCUs so that blocks are written without edge cases.
- **Entropy data:** A pre-pass removes the byte stuffing (`FF 00`) and records the restart markers. After that, every bit position is a plain number, and the bit reader needs no marker check in the hot path.
- **Huffman:** 10-bit lookup; for AC additionally a combined table (code + extra bits → run, value, length).
- **Parallel without restart markers** (Weißenberger & Schmidt):
  1. Speculative: Each thread decodes its section from a guessed position and records the bit position and DC sums at every MCU start.
  2. Synchronize: Each thread decodes past its end until it hits a recorded state of its successor. From there on, both are identical.
  3. Prefix sums yield the absolute MCU number and the DC predictors at every recorded MCU start.
  4. Final: The threads decode MCU row bands from the known state and write directly into their own band of the planes (safe Rust, `split_at_mut`).
- **With restart markers**, step 1 is exact and step 2 is dropped.
- **Fallback:** Arithmetic coding, 12-bit, lossless and CMYK stay with libjpeg-turbo.

## 4. Architecture

| Crate | Contents | Status |
|---|---|---|
| `blinzel-core` | pixel buffers, buffer pool, format detection, decoders (JPEG/turbojpeg, PNG, GIF, own BMP), EXIF/MPF, layout, selection, natsort, file list, scheduler, LRU, CPU reference renderer, timing, settings, export | 🟡 (tested on Linux) |
| `blinzel-ui` | Platform-independent interface: state machine `App`, loader, thumbnail pool, grid/filmstrip, menu model, color management cache, settings file. Talks only to the traits in `backend.rs` (`Device`, `Canvas`, `Host`, `Services`), which each platform shell in `blinzel-app` implements natively (Windows: Direct3D 11/Win32). | 🟡 (tested on Linux; Windows unchanged) |
| `blinzel-jpeg` | strict marker parser; parallel decoder (Huffman, 8-bit, sequential and progressive) into YCbCr planes, byte-identical with libjpeg-turbo | 🟡 |
| `blinzel-i18n` | `i18n/*.toml` → static tables (DE/EN) | 🟡 |
| `blinzel-bench` | corpus + library-vs-in-house benches (JSON) | 🟡 |
| `blinzel-app` | The three shells around `blinzel-ui`. Windows (`blinzel.exe`): Win32 window, menu, D3D11 renderer, WIC, clipboard, dialogs, setup, updater, `--bench`. Linux (`src/linux/`): winit, wgpu renderer (`blinzel.wgsl`), cosmic-text, rfd, arboard, notify, Little CMS, X11 display profile, `--install`. macOS (`src/macos/`): AppKit, Metal renderer (`blinzel.metal`), Core Text, ImageIO, menu bar, NSPasteboard. Shared by Linux and macOS: updater flow (`selfupdate.rs`), downloads via `curl`, Little CMS (`lcms.rs`). | 🟡 |
| `blinzel-release` | Release tool: Ed25519 keys and signature of the checksums, license notices per platform (`THIRD-PARTY-NOTICES*.txt`), the macOS app bundle (`macos-bundle`) | 🟡 |
| `blinzel-race` | Competitive benchmark (M8): test images with color signature, Desktop Duplication on all monitors, open/next/rapid fire | 🟡 |
| `website/` + `blinzel-site` | Website for Cloudflare Workers (M8): static, DE/EN, charts as inline SVG from `website/data/race.json` | 🟡 |

## 5. Core principles: startup and browsing

- 🟡 **Parallel cold start:**
  - The single-instance handoff (`WM_COPYDATA` + `AllowSetForegroundWindow`) comes first.
  - Then the decode job is queued, and `d3d11.dll` is loaded via `LoadLibrary` on a worker.
  - The window is created cloaked, with `DWMWA_TRANSITIONS_FORCEDISABLED` and `WS_EX_NOREDIRECTIONBITMAP`.
  - The folder listing is built in the background.
  - The window is uncloaked with the first image, after 400 ms at the latest.
  - *Deviation from the original plan:* Instead of reading the header before `CreateWindow`, the still invisible window is resized to the image size before uncloaking. The result is the same, it just does not block.
- 🟡 **Shaders:** precompiled with fxc. COM, WIC, Direct2D and DirectWrite start only when needed; DirectWrite is warmed up after the first present.
- 🟡 **Embedded camera preview** first, if it is at least half the size of the screen. For raws it replaces the stage if it covers it (see gate table).
- 🟡 **Prefetch scheduler:**
  - Two stages (I/O, CPU), priorities, generation cancel.
  - Direction-dependent: 4 ahead, 2 back.
  - Every file is read only once; one job delivers preview and stage.
- 🟡 **GPU:**
  - Tile textures (4096, with border), upload directly from workers.
  - VRAM budget via `QueryVideoMemoryInfo`.
  - EXIF orientation as a transform, never rotate pixels.
- 🟡 **I/O:**
  - `ReadFile` with buffer pool, no mmap.
  - Cloud placeholders are skipped during prefetch.
  - HDD detection (seek penalty) → 1 I/O thread.
- 🟡 **Hybrid CPUs:** workers only on P-cores (EfficiencyClass), ABOVE_NORMAL, without power throttling.
- 🟡 **Robust:** no `panic=abort`, every decode runs in `catch_unwind`.
- ✅ **Delay-load** for shell32/ole32/oleaut32/shlwapi measured: startup until `main` stays at 20–23 ms (that is the process creation itself). No delay-load.
- 🟡 **Resident mode (M6, setting, off by default):**
  - Closing hides the window instead of ending the process. Image, folder listing, textures and buffers are released; device, shaders and decoder threads stay warm.
  - A second start only hands over the file (`WM_COPYDATA`). The hidden window comes back cloaked, is uncloaked with the first frame and then takes the foreground.
  - Without "Open images in the same window", the handoff only goes to a hidden instance, and closing only hides if there is no hidden one yet.
  - Tray icon (Show, Open…, Exit), re-registered after an Explorer restart (`TaskbarCreated`).
  - Start at sign-in via `HKCU\…\Run` (`blinzel.exe --resident`) as long as the setting is on. Uninstalling removes the entry and ends running instances beforehand.
  - The `--resident` start warms up decode, upload and present once (generated JPEG at screen size; for this the window is cloaked once and shown as a tool window, i.e. without a taskbar button).
  - If an update has been installed in the meantime, the hidden instance starts the new version the next time it hides.
  - A woken instance reloads the settings; a hidden one does not save on exit (its copy may be stale).
- 🟡 **ICC color management (M6, setting, on by default):**
  - Embedded profiles from JPEG (APP2 `ICC_PROFILE`) and PNG (`iCCP`) end up in the `Probe`; images without a profile are treated as sRGB.
  - The target is the profile of the monitor the window is on (`GetICMProfileW`). If "Advanced color" is active there (Auto Color Management or HDR), DWM converts from sRGB itself; the target is then sRGB.
  - Our own check recognizes sRGB profiles by their primaries and tone curve. If source and target are sRGB (the normal case), nothing happens: no DLL loading, no shader work.
  - Otherwise the Windows Color System (`mscms`, all profile types) builds a 3D LUT with 33³ points (16-bit), cached per profile pair. The image shader applies it trilinearly after filtering, before compositing with the background; the CPU reference and the first GDI image do the same.
  - When moving to another monitor (or on `WM_DISPLAYCHANGE`), the target is determined anew.
  - `GetICMProfileW` loads the ICM subsystem on the first call (approx. 7 ms, which at startup competed with decode and device: first GDI image and Direct3D frame each approx. 7 ms later). Therefore a start uses the target of the last run (`%LOCALAPPDATA%\amigo-blinzel\display-color.txt`) and asks Windows only after the first frame; if the answer differs, it redraws. After that, no measurable difference in A/B anymore.
  - A needed LUT is built in parallel with decoding (4–7 ms, cold with DLL loading). Display P3 image (24 MP) cold: first GDI image as without a profile; resident handoff with profile 24–26 ms. The `--resident` start preloads `mscms`.
  - WebP profiles (and the EXIF orientation) are read by the native probe; for JPEG XL the profile describes the color space `jxl-oxide` renders into (sRGB yields no profile). TIFF profiles are also read by the native probe, HEIC profiles (`colr`/`prof`) by our own HEIF container reader, AVIF profiles by our own container reader (`colr`, nclx with P3 primaries as a generated profile), both also when WIC decodes. Profiles from WIC formats (HEIC, AVIF) are not read yet (comes with the native decoders in M5); grayscale and CMYK profiles yield no transform.
- 🟡 **App icon:** `build.rs` draws the favicon motif and embeds it as ICO (DIB up to 64 px, PNG for 256 px). Window, taskbar, tray and Start menu shortcut use it.
- 🟡 **First image via GDI:** Device creation takes longer than reading and decoding. The loader therefore renders the first decoded image itself on the CPU (`blinzel_core::preview`, the same geometry as the GPU); the UI thread pumps messages instead of waiting for the device and shows it with `SetDIBitsToDevice`. Direct3D takes over as soon as the device is ready. The window is shown without activation (`SW_SHOWNA`, saves approx. 20 ms) and takes the foreground after the first image.

## 6. UI and window behavior

- 🟡 **Frameless window:**
  - `WM_NCCALCSIZE` removes the caption; borders and shadow remain.
  - Top resize border and drag strip via `WM_NCHITTEST`.
  - Snap layouts via `HTMAXBUTTON`.
- 🟡 **Controls:** ☰ + min/max/close as SDF shapes over the image, with halo and hover areas (close in red).
- 🟡 **Hamburger menu:** native `TrackPopupMenuEx` in dark mode (uxtheme 135/133/136), localized.
- 🟡 **View modes** (1/2/3), zoom around the cursor (wheel, +/−, 0), pan (mouse, arrows on overflow), full screen (F11, Enter, double-click), rotation in the view only (R/L).
- 🟡 Mode 1 ("original size"): The window follows every image, limited to the work area and centered on its middle.
- 🟡 An empty start shows a hint; drag & drop, Ctrl+O, Del (Recycle Bin), mouse back/forward.
- 🟡 Filmstrip (T) and grid (G), see below.

### Thumbnails (M4)

- **Generation** (`blinzel_core::thumb`): edge 256 px (Explorer size; at 150 % scaling that is enough for 160 px tiles).
  - The source is the smallest one that covers the tile: an embedded preview (EXIF/MPF) if it is large enough and the aspect ratio matches (EXIF thumbnails often have bars), otherwise the image itself with DCT scaling (JPEG down to 1/8).
  - One thread per image, many images in parallel on the background pool; the view has priority.
  - Downscaling with exact area averaging (separable box filter with fractional weights), output BGRA; the EXIF orientation is applied when drawing.
- **Cache** (benchmark gate against the Windows thumbnail cache `IShellItemImageFactory`): own pack file under `%LOCALAPPDATA%\amigo-blinzel\thumbs\`.
  - `thumbs.pack`: appended entries (JPEG, quality 85, approx. 10–15 KB per thumbnail).
  - `thumbs.idx`: log of fixed-size records (path hash, size and modification time of the file, offset, length, dimensions, orientation), loaded into a hash table on first access.
  - A stale entry (size or time changed) is regenerated. Above 1 GB a new generation begins (pack and index are replaced).
  - Multiple instances: appending under a named mutex; what another instance has written is only seen on the next load (duplicates do no harm).
- **GPU:** atlas pages with 64 slots for 256 thumbnails in YCbCr 4:2:0 (2048² R8 for Y, 1024² R8G8 for Cb/Cr, 6 MB per page, at most 8 pages), LRU; the shader converts to RGB. Thumbnails from the cache (JPEG) go into the atlas as planes without a detour through RGB, 1.5 instead of 4 bytes per pixel. A grid with 200 tiles is 200 small draws.
- **Interface** (both off by default, in the "View" menu and via key):
  - Filmstrip (T, setting) at the bottom with the neighbors of the current image; the image gets the area above. Click opens, wheel flips.
  - Grid (G) over the whole window for the folder: click selects, double-click, Enter or Space opens, arrows/Page Up/Page Down/Home/End move the selection, wheel scrolls, Esc or G closes.
  - What is visible is requested first, then the next and the previous page; the pool runs at normal priority, the displayed image decodes above it.
- **Measurement** (`blinzel-bench thumbs <dir> [--count N] [--threads T]`): cold (without cache) thumbnails per second over a folder with 12-MP JPEGs with sensor noise (approx. 4 MB per file), warm 200 tiles from the cache. Targets from section 12: cold ≥ 150/s, 200 from the cache ≤ 30 ms.
- **Status (Ryzen 7 PRO 6850H, 16 threads):** cold 287/s (1 thread: 26/s), saving 1.1–1.3 ms per thumbnail (12.6 KB), warm 200 in 23.6 ms (1 thread: 0.9 ms per thumbnail). Only the per-plane conversion to target size achieved this: `Image::to_bgra` (exact, float, bilinear chroma) cost 1.8 ms per 256 thumbnail and 15 ms per 1/8 decode.
- For comparison, on the same files the Windows thumbnail provider and cache: cold 18/s, warm 432–446 ms for 200.
- **End to end in the app** (`blinzel --bench`, `thumb_grid_200`: open the cache, read and decode 200, load into the atlas, draw the grid, GPU done): **16 ms** median (target ≤ 30 ms). With a BGRA atlas it was 37–69 ms; the upload and the conversion to RGB made the difference.
- 🟡 Remember window position and size: on close (and hide) `GetWindowPlacement` into the settings, on start `SetWindowPlacement` as long as a monitor is still there; a maximized window is still maximized while cloaked so that the first image is computed for the real size. Visual check open.

## 7. Selection, copy, cut, save

Principle: image files are never overwritten. Saving happens only via "Save as", with the Windows overwrite confirmation.

- 🟡 **Selection rectangle** in full-resolution coordinates:
  - Handles, moving, size display `W × H` via DirectWrite.
  - Dimmed surroundings.
  - Ctrl+A, Esc.
- 🟡 **Ctrl+C** with delayed clipboard rendering (`CF_DIBV5` + `PNG`): only the region is decoded, at full resolution and only when pasting.
- 🟡 **Ctrl+X:** copy and crop the view to the selection (pure view state). Esc or "Show original" restores the image.
- 🟡 **Ctrl+S "Save cutout as…"** (selection, otherwise crop):
  - **Lossless JPEG** if the source is a JPEG, the cutout starts on MCU boundaries and no view rotation is active.
    - Implemented via `tj3Transform` crop, without re-encoding, very fast.
    - The EXIF data is preserved, including the orientation.
    - The stale EXIF thumbnail is detached, the MPF segment removed.
  - **Otherwise:**
    - Decode the region at full resolution and orient it.
    - Save as JPEG (quality 95, subsampling of the source, ICC profile carried over) or as PNG.
  - Runs on a background thread; errors appear as a message.

### Batch processing: resize (M9)

Principle as above: source files stay unchanged. Every output is a new file; a name that already exists gets a number (` (2)`) instead of being replaced.

- ✅ **Engine** `blinzel-core::batch` (feature `batch`), tested on Linux:
  - Size in display orientation: long edge, width, height, fit in box, fill box (center crop, i.e. fixed aspect ratio), percent; "don't enlarge" as the default.
  - JPEG is decoded smaller via DCT scaling as long as at least twice the target size remains.
  - Scale in linear light (gate above), then rotate into display orientation, optionally sharpen (unsharp mask).
  - Formats: JPEG (libjpeg-turbo; from quality 90 without chroma subsampling, transparency onto white), PNG, WebP (libwebp, lossy or lossless), AVIF (rav1e via `ravif`), "as source" (JPEG/PNG/WebP/AVIF stay, otherwise PNG with transparency, otherwise JPEG). Animations: the first frame.
  - Metadata: EXIF from JPEG, PNG, WebP, HEIF/AVIF and JPEG XL is rewritten, not patched: orientation 1, new pixel dimensions, without IFD1 preview and MakerNote, GPS removed on request. The ICC profile of the source is embedded (AVIF: `colr`/`prof` next to nclx). Checked with Pillow and `avifdec`.
  - Names by pattern: `{name}`, `{ext}`, `{n}`/`{n:3}`, `{w}`, `{h}`, `{date}` (EXIF capture time, otherwise modification time in local time; `{date:DD.MM.YYYY_hh-mm}` with `YYYY YY MM DD hh mm ss`); target folder relative to the source (default `resized`).
  - Writing: temporary file, then via hard link to the free name (fails instead of replacing; without hard links, rename).
  - Names planned in advance (`batch::plan`): from the header and EXIF of each file, in list order with numbers for identical names; the run writes exactly these paths (only if one has been taken in the meantime, the next number). Every intermediate file has its own name, even with the same target name.
  - Parallel across files (one decode thread per file), limited by a memory budget; a broken file is reported, the rest continues; cancel.
- ✅ **Measurement** (`blinzel-bench batch`, Linux container, 4 vCPU, 24 12-MP JPEGs with sensor noise, long edge 1920): JPEG 8.6/s (431 KB per file), WebP 5.1/s (213 KB), AVIF 1.1/s (73 KB, rav1e speed 6). For comparison ImageMagick on the same files (`convert -resize 1920x1920`, 4 processes): 2.1/s.
- ✅ **Dialog in the app** (menu "Resize images…", Ctrl+B): modal panel over the viewer, the same code on all platforms (`blinzel-ui/src/batch_dialog.rs`, controls in `widgets.rs`). Size mode, dimensions, "don't enlarge", format, quality, lossless (WebP), sharpen, metadata/GPS/color profile, name pattern, target folder (system dialog "…"). Runs in the background with progress, cancel, report (first errors) and "Open folder". The settings are kept for the session.
- ✅ **File list via drag & drop** (`blinzel-ui/src/batch_files.rs`): The dialog starts empty; images and folders (without subfolders) are dragged in, headers read in the background. The list shows old → new for each file with real size and real date, unreadable files in red; × removes, "Remove all" clears. Without an open dialog, a drop opens the image as before. On macOS a drop handler is still missing; local time for modification dates only on Windows (otherwise UTC).
- ✅ **Multi-selection in the grid:** Ctrl+click, Shift+click (range), Ctrl+A, Esc clears; marked tiles with a check mark.
- ✅ **"Save cutout as"** also as WebP and AVIF (the same encoders).
- ⬜ Comparison with `vipsthumbnail` and XnConvert in `blinzel-race`.

## 8. Localization

- 🟡 `i18n/<sprache>.toml`; `build.rs` generates `enum Msg` and static tables. At runtime a translation is just an array index.
- 🟡 Missing keys fall back to English (build warning). Placeholder errors abort the build.
- 🟡 Language selection via `GetUserPreferredUILanguages`; can be overridden in the menu, without a restart.
- 🟡 Plural rules (CLDR families).
- 🟡 RTL languages: `[meta] direction = "rtl"` in `i18n/<sprache>.toml` yields `Lang::is_rtl()`. The app then mirrors the controls (menu on the right, window buttons on the left), opens menus mirrored (`TPM_LAYOUTRTL`) from the right edge, sets the reading direction of DirectWrite and of the message boxes (`MB_RTLREADING`) and swaps ← and → when flipping through images. Open: filmstrip/grid and the setup dialog stay left-to-right; we do not ship an RTL language yet (translation by native speakers needed).

## 9. Platforms

### Windows 11 24H2 or later

Only versions with Microsoft support. As of October 2026:

| Version | Support |
|---|---|
| Windows 11 25H2 | Home/Pro until Oct. 2027, Enterprise until Oct. 2028 |
| Windows 11 24H2 | Enterprise/Education until Oct. 2027 |

Windows 10 is not supported: support has ended for Home/Pro, only Commercial ESU and LTSC are still running.

**Consequences:**
- 🟡 Minimum Direct3D feature level 11_0 and Shader Model 5. Every Windows 11 device has a DX12/WDDM 2 GPU. No FL 10 fallbacks.
- 🟡 Version check: build < 26100 → localized message at startup or in the installer.
- 🟡 Application manifest: supportedOS, PerMonitorV2, longPathAware, UTF-8 code page.
- 🟡 ARM64 build in CI (cross from x64); tests run on x64.
- Usable without fallbacks: EcoQoS, CPU sets, snap layouts, dark mode attributes, WIC codecs for HEIF, AVIF and WebP (as far as the Store extensions are installed).

### macOS and Linux (M10, M11)

The shells implement the same traits as Windows (`Device`, `Canvas`, `Host`, `Services` in `blinzel-ui/src/backend.rs`). The uniforms of each draw call come from `blinzel_ui::draws` for wgpu and Metal; the shaders are line-by-line ports of `blinzel.hlsl` and, like WARP, are checked against the CPU reference renderer (lavapipe in the Linux CI, the Metal device of the macOS runner).

**Linux:**
- 🟡 Frameless winit window (the borders resize, the top strip moves), menu and dialogs drawn by the app itself, file dialogs via the xdg portal (rfd), trash, clipboard, folder watching (inotify), single instance via a Unix socket.
- 🟡 Color management via Little CMS; under X11 the target is the profile that colord or KDE publish as `_ICC_PROFILE` on the root window (mapped to the window's monitor via RandR). Wayland has no protocol for this yet: sRGB there.
- 🟡 `blinzel --install` / `--uninstall`: `~/.local/share/amigo-blinzel/`, link `~/.local/bin/blinzel`, desktop entry with MIME types, icon; app id/WM_CLASS match the entry.
- 🟡 Updater as on Windows for the installed copy; asset `amigo-blinzel-linux-<arch>` (the bare file), downloads via `curl`.
- Not on Linux: resident mode and tray.

**macOS:**
- 🟡 NSWindow with content under the transparent title bar, `CAMetalLayer` as the view's layer, menu bar (application menu and "Image", filled from the menu tree when opened) and context menu as NSMenu, NSOpenPanel/NSSavePanel, Trash, clipboard (PNG), folder watching (FSEvents), files from the Finder via `application:openURLs:`, drag & drop.
- 🟡 ImageIO as the system decoder: HEIC there first (hardware HEVC, Apple's license), RAW, TIFF and the rest as fallback; downscaled requests via the thumbnail path.
- 🟡 Resident mode (`macos/resident.rs`): an icon in the menu bar (`NSStatusItem`, SF Symbol "eye") with the tray menu; closing the window hides it (`windowShouldClose:`), the app then leaves the Dock (activation policy "accessory") and keeps its GPU warm; the Dock icon, Finder or the menu wakes it. Start at sign-in via the LaunchAgent `~/Library/LaunchAgents/io.github.amigo-labs.blinzel.plist` (`blinzel --resident`, only for a copy in an app bundle). An update installed meanwhile takes over when the window is next hidden (`open -n -g … --args --resident`).
- 🟡 Color management: the target is the display's profile (`CGDisplayCopyColorSpace`); the Metal layer has no color space of its own, so macOS shows its pixels unchanged, as on Windows.
- 🟡 App bundle (`blinzel-release macos-bundle`): Info.plist with document types, icon (ICNS from the icon generator), universal binary via `lipo`, ad hoc signed, not notarized. The updater swaps the bundle; the previous one stays next to it for rollback.
- dav1d assembly only on x86-64: the published `rav1d` does not contain `src/arm/asm-offsets.h`, which its ARM assembly files include. ARM64 (Mac and Linux) uses the Rust fallbacks.

## 10. Installation, updates, signing (M7)

### Setup exe

- 🟡 **Release assets** per release on GitHub:
  - `amigo-blinzel-setup-x64.exe` and `amigo-blinzel-setup-arm64.exe`; this is the same exe as the viewer.
  - `SHA256SUMS` and `SHA256SUMS.sig`.
  - `THIRD-PARTY-NOTICES.txt` (the same license texts the exe shows in the menu).
- 🟡 **Installation dialog:** When started as setup (file name contains `setup`, or `--install`), a dialog appears in the viewer's look, frameless and multilingual.
- 🟡 **Per-user installation, without admin**, to `%LOCALAPPDATA%\Programs\amigo-blinzel\`:
  - Start menu shortcut (`IShellLinkW`, AppUserModelID).
  - Entry under `HKCU\…\Uninstall`.
  - Create ProgIDs, Capabilities and RegisteredApplications.
  - Then open the Default apps page (Windows does not allow setting it programmatically).
  - `SHChangeNotify`.
- 🟡 **Uninstallation:** via the system settings. The exe copies itself to `%TEMP%` and cleans up from there.
- 🟡 **Silent mode:** `--install --silent` and `--uninstall --silent` (for winget).
- 🟡 **Version check in setup:** build < 26100 → abort with a message.
- 🟡 **winget manifest**; a PR to winget-pkgs after every release, optionally automated.

### No code signature

- ✅ **Decided:** Releases are not signed with a code signing certificate (no SignPath, no Authenticode, no notarization on the Mac). Consequence: Windows SmartScreen warns on the first start ("More info" → "Run anyway"); the download page explains this.
- 🟡 **Releases automatically via release-please** (as in amigo-native):
  1. Every push to `main` creates or updates a release PR (`.github/workflows/release-please.yml`). It contains the next version in `Cargo.toml` and `Cargo.lock` as well as the `CHANGELOG.md` entry from the Conventional Commits since the last release. Before 1.0, `feat` bumps the second digit, `fix` the third; `docs`, `ci`, `chore` etc. do not trigger a release.
  2. When the release PR is merged, release-please sets the tag `v<version>` and creates the release as a draft with the changelog.
  3. Then `release.yml` builds all platforms, generates `SHA256SUMS`, signs them with Ed25519, attaches the files and publishes the release. Only then do the updaters see it.
  - `release.yml` can also be started by hand for an existing tag.
  - Configuration: `release-please-config.json` and `.release-please-manifest.json`. The manifest starts at 0.0.0, so the first release becomes 0.1.0. `bootstrap-sha` is the merge of PR #22.
  - Optionally the secret `RELEASE_PLEASE_TOKEN` (fine-grained PAT with write access for contents and pull-requests), so that CI also runs on the release PR. Without a token, "Allow GitHub Actions to create and approve pull requests" must be on under Settings → Actions → General.
- ✅ **Own signature of the checksums (Ed25519).** The app has the public key built in (`276b0fbd4a1f9eb68476f0b8a14c5cc0bab403cbca24bcf07d8cff333d3fa9b7`). It is the only proof of origin: the updater installs only what is signed with the release key.

**Implementation (branch `m7-install`):**
- `blinzel-app/src/setup/`: mode detection, installation via a `Layout` (real paths or a sandbox for tests), registry helpers for HKCU only, Start menu shortcut with AppUserModelID `amigo-labs.amigo-blinzel`, setup window with the viewer's renderer and DirectWrite (without Direct3D: message box).
- Exit codes of the setup: 0 installed, 1 failed, 2 Windows too old, 3 cancelled.
- `packaging/winget/`: templates of the three manifests (scope user, silent switch, ProductCode = uninstall key).
- `.github/workflows/release.yml`: builds x64 and ARM64, generates and signs `SHA256SUMS`, creates a draft release.
- Test `blinzel-app/tests/setup.rs`: installation, in-place update and uninstallation in a sandbox (temp folder, own HKCU branch).

### Self-updater (silently in the background)

- 🟡 **Check:**
  - At most 1× a day, no earlier than 10 s after startup, on a background thread (never on the critical path).
  - Source: `GET https://api.github.com/repos/amigo-labs/amigo-blinzel/releases/latest` via WinHTTP, which is loaded via `LoadLibrary` and uses the system proxy.
- 🟡 **Download** the matching asset (x64/ARM64) plus `SHA256SUMS(.sig)` to `%LOCALAPPDATA%\amigo-blinzel\updates\`.
- 🟡 **Verify in two steps:**
  1. Ed25519 signature of the checksums.
  2. SHA-256 of the exe.

  Only then is it swapped.
- 🟡 **Swap:**
  - Rename the running exe to `.old` (allowed on NTFS) and put the new one in its place.
  - The next start uses the new version at no extra cost.
  - `.old` is deleted on the next start.
- 🟡 **Rollback:** If the new version fails three times to start up to the first frame, `.old` is restored.
- 🟡 **Menu:** "Check for updates" and "About amigo-blinzel (version)". Setting `auto_update = true`.

**Implementation:**
- Decisions without Windows in `blinzel-core::update` (version, release JSON via a small JSON reader of our own, `SHA256SUMS`, Ed25519 via `ed25519-compact`, schedule, start counter) with unit tests.
- Windows part in `blinzel-app/src/update.rs`: WinHTTP and `BCryptHash` are loaded only when needed. Only an installed copy updates itself; developer and portable builds never do.
- `blinzel-release keygen | sign | verify` for our own signing key.
- The public key is stored as `RELEASE_KEY` in `crates/blinzel-core/src/update.rs`, the secret one (seed as hex) only in the repository secret `RELEASE_SIGNING_KEY`. Without `RELEASE_KEY` (`None`), the updater verifies and installs nothing. New key: `cargo run -p blinzel-release -- keygen <datei>`, store the file's content as the secret, delete the file, enter the printed line. A key change reaches installed copies only via a release that is still signed with the old key.

## 11. Website and competitive comparison (M8)

### Website on Cloudflare Workers

- 🟡 `website/`: static HTML/CSS with minimal JS, extremely fast (in keeping with the product), German and English.
  - Pages: home, features, **comparison**, **benchmarks**, methodology, download, changelog, privacy.
  - Charts are generated at build time as inline SVG from the benchmark JSONs; no chart library in the browser.
  - The download link points to `releases/latest/download/amigo-blinzel-setup-x64.exe` (stable URL).
- 🟡 **Deployment** via Cloudflare Workers Static Assets (`wrangler.toml` in the repo root directory).
  - Cloudflare Workers Builds is connected to the repo (root directory `/`, default deploy command). Before every deploy, Wrangler runs `website/build.sh`: it installs the missing Rust toolchain in the build image and generates `website/dist` with `blinzel-site`.
  - Alternatively, `.github/workflows/website.yml` deploys as soon as the secrets `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` are set; with Workers Builds this is not necessary.
  - Initially the `workers.dev` subdomain; a custom domain can be added at any time.

### Competitive benchmark `blinzel-race`

A dedicated measurement tool for Windows and macOS measures all viewers the same way, from the outside.

**Implementation (`crates/blinzel-race`, branch `m8-race`):**
- `corpus <dir>`: 24 JPEGs with 24 MP and approx. 8 MB each (sensor noise, so that the entropy decoder works as hard as with real photos), each around one of six signature colors (the colored corners of the RGB cube). Consecutive images always differ.
- Detection: Desktop Duplication on every monitor (viewers open where they last were); every presented frame delivers `LastPresentTime` (QPC) and a grid of 40 × 24 sample points.
- Keys are only sent while the viewer owns the foreground window.
- `list` shows the detected viewers with version (version resource; `blinzel.exe` now has one).
- `run <dir> [--reps N] [--cold] [--only …] [--viewer name=exe] [--json …]`. `--cold` empties the standby list and needs administrator rights.
- **macOS** (`capture_macos.rs`, `sys_macos.rs`, `viewers_macos.rs`; the flow in `race.rs` is the same):
  - Detection via ScreenCaptureKit on every display. A frame only arrives when the screen has changed, and carries its display time (`mach_absolute_time`). ScreenCaptureKit scales the grid down on the GPU.
  - Launch via Launch Services (`open -a App datei`), as in the Finder. Quit politely via `NSRunningApplication.terminate`, after 3 s with `SIGKILL`.
  - Keys via `CGEvent`, only while the frontmost window belongs to the viewer (`CGWindowListCopyWindowInfo`). `--cold` calls `purge` via `sudo` and asks for the password once at the beginning for this; the tool itself does not run as root, otherwise the viewers would also start as root.
  - The terminal needs the "Screen Recording" and "Accessibility" permissions; without them, `run` aborts with a hint.
  - Candidates: amigo-blinzel, Preview, XnView MP, qView, Pixea, ApolloOne, Phoenix Slides (Xee³ is no longer distributed). Preview does not flip through the folder for a single file; for it there is only the value for opening.
  - Result to `website/data/race-macos.json`; the website shows it as a separate macOS section (`{{race-macos:…}}`).

**First trial runs** (laptop, Ryzen 7 PRO 6850H, Radeon 680M, 3 monitors; proof of function only, no reference figures):

| Viewer | Open warm | Next | Rapid fire 6 keys |
|---|---|---|---|
| amigo-blinzel 0.1.0 | 270–295 ms | 9–14 ms | all shown |
| ImageGlass 8.8 | 1.8 s | 0.8 s | all, slowly |
| Windows Photos 2026.11 | 1.2 s | 45 ms | 3 shown, keys swallowed |

- 🟡 **Candidates:** IrfanView 64, ImageGlass, XnView MP, FastStone Image Viewer, Windows Photos; optionally JPEGView and qimgv. Each with default settings; versions are logged.
- 🟡 **Metrics:**
  1. **Open:** time from process start until the image is visible, warm and cold (standby list emptied).
     - Detected via the Desktop Duplication API (frame timestamp `LastPresentTime`).
     - The test images carry a unique color signature.
  2. **Next image:** `SendInput` (right arrow), time until the color signature changes; individually after idle (prefetch case) and as rapid fire at 30 Hz (how many images are really shown).
  3. **Thumbnails** (from M4): time until the grid of a 1000-image folder is filled, for viewers with a grid mode.
- 🟡 **Fairness and legal:**
  - Same machine, same files, 10 repetitions, median and p95.
  - Raw data, versions and methodology are published.
  - Thus objective, verifiable and relevant, as comparative advertising law requires (UWG §6).
- ⬜ **Figures:** Reference values from a defined reference machine (hardware is stated). CI values only as a trend.
- 🟡 **Feature comparison table** (`compare.html`, DE/EN): license, commercial use, ads, telemetry, ARM64, download size, color profiles, HEIC/AVIF/JPEG XL/WebP/RAW for IrfanView, ImageGlass, XnView MP, FastStone, JPEGView, qimgv and Windows Photos. As of 2026-10-23, every cell links its source; anything unconfirmed reads "not documented". Proofread once by hand before publishing. Startup time and memory come from the reference measurement.

### Animations (M5)

- 🟡 **GIF:** `decode::gif::decode_animation` composites every frame onto the canvas like browsers do: transparent pixels leave the canvas as is, disposal "background" clears the frame rectangle, "previous" restores the state before; delays under 20 ms play at 100 ms; the loop count from the NETSCAPE extension applies (otherwise once). At most 256 MB of frames; longer animations play what fits.
- For this, the loader has the stage `Anim`, only for the current image and only once its probe says GIF: the first image appears immediately as before, the animation starts as soon as all frames are on the GPU (480 × 270: 1.2 ms decode per frame). A timer advances the frames; when the image changes, the animation is discarded.
- 🟡 **WebP:** `decode::webp::decode_animation` (`image-webp` composites the frames itself), same rules for delays, loop counts and the 256 MB budget; the stage `Anim` now applies to GIF and WebP. A still WebP reports the stage as a still image.
- 🟡 **JPEG XL:** `decode::jxl::decode_animation` renders every keyframe (duration from ticks and `tps`, loop count from the header), same rules and budget. Still files are already detected by the header.
- 🟡 **Playback while decoding:** The decoders deliver every frame individually (`stream_animation` with a frame callback); the loader uploads it to the GPU immediately and, from the second frame on, sends the current state every 200 ms. Playback thus starts after two frames; if it reaches the last frame decoded so far, it holds there and continues with the next delivery. A decode error midway ends the animation with the frames up to that point. Visual check open (large GIFs).

## 12. Milestones

| | Contents | Target | Status |
|---|---|---|---|
| **M0** | Workspace, CI (Linux + Windows), corpus, bench harness | baseline JSON in CI | 🟡 |
| **M1** | Single image: startup sequence, chrome, menu, view modes, i18n, D3D11, JPEG/PNG/BMP/GIF, WIC fallback, EXIF/MPF | t0 → present ≤ 50 ms warm, ≤ 120 ms cold; no white frame | 🟡 |
| **M2** | Browsing, prefetch, budgets, full-res when zoomed, folder watch, P/E pools, selection, Ctrl+C/X/S | next ≤ 8 ms p99 prefetched; 10k folder ≤ 25 ms; Ctrl+C < 1 ms | 🟡 |
| **M3** | `blinzel-jpeg`: parallel decoder | full-res 24 MP ≥ 3× turbo (8 cores); DC-only ≥ 2× | 🟡 targets reached (4.1× and 3.6×), progressive 1.7×; CI confirmation pending |
| **M4** | Thumbnails: fast paths, pack cache, atlas, filmstrip + grid | 200 tiles from the cache ≤ 30 ms; cold ≥ 150/s | 🟡 done: 287/s cold, 200 from the cache to on screen 16 ms; open: visual check |
| **M5** | WebP (animated), GIF animation, AVIF, HEIC, JXL, TIFF, RAW native (each with bench gate) | native faster than WIC, otherwise WIC stays | 🟡 GIF and WebP animation, WebP native (gate: lossless and animations native, lossy via WIC once loaded), JPEG XL native (gate passed, +1.7 MB), TIFF native with parallel strips (otherwise WIC), RAW via embedded previews (development still WIC), license notices of the bundled libraries (`THIRD-PARTY-NOTICES.txt`, menu "Third-party notices"), AVIF (`rav1d`) native as long as WIC has no codec, HEIC (`zune-heic`) likewise, but only with the feature `heic` (off by default, HEVC patents); open: gate HEIC/AVIF against WIC on Windows |
| **M6** | Tray/resident, settings, ICC color management | resident handoff → visible ≤ 25 ms | 🟡 resident, tray, settings menu, app icon, ICC (JPEG/PNG) done; handoff 23–25 ms (median, idle machine); open: visual check |
| **M7** | Setup exe, release workflow, self-updater, winget | checksums signed, update without user action | 🟡 implemented; open: release key, first release |
| **M8** | `blinzel-race` + website on Cloudflare Workers | public, reproducible comparison figures | 🟡 tool, website and feature table (with sources) done; Windows measured (amigo-blinzel vs. Windows Photos, without cold); open: macOS measurement, proofreading the table, Cloudflare secrets |
| **M9** | Batch processing: engine, dialog, multi-selection | faster than ImageMagick; sources never overwritten | 🟡 engine and dialog done, 8.6 instead of 2.1 images/s; open: visual check on Windows and macOS, comparison with vipsthumbnail/XnConvert |
| **M10** | Linux shell (winit + wgpu), installation, updater | golden test on lavapipe, startup to first image in CI | 🟡 done, checked in CI (Xvfb); open: visual check under Wayland, measure startup times |
| **M11** | macOS shell (AppKit + Metal), app bundle, updater | Metal golden test, startup to first image in CI | 🟡 done, built in macOS CI; open: visual check on a Mac (Finder, HEIC, color profile, menu bar), measure startup times |
| **M12** | Screenshots (separate PR after #22) | Print key to overlay without noticeable delay | 🟡 Windows done: Print key (and Ctrl+Shift+Print) in resident mode, GDI capture of the virtual desktop, overlay from `blinzel-ui/src/snip.rs` (window under the mouse highlighted, drag = region, click = window, Enter = screen), always to the clipboard, then as set by three switches (`blinzel-core/src/screenshot.rs`): PNG to `Bilder\Screenshots` (Pictures\Screenshots, configurable), open in the viewer, open in the editor (a shot that is opened but not saved goes to the cache and is deleted after a day); "Open with…" and "Edit with…" (E) on all platforms; CI: `--capture-test`. Open: visual check on Windows 11, measure time Print key → overlay, macOS (ScreenCaptureKit, Carbon hotkey, NSStatusItem), Linux (X11 GetImage, Wayland portal), HDR |

**Order:**
1. PR #1: M0–M2 including the Windows 11 baseline and "Save cutout".
2. M3.
3. M7, so that releases can be distributed early.
4. M8 (website with first comparison figures for opening and browsing).
5. M4, M5, M6. The website figures are extended with every milestone.

## 13. Verification

- **Linux (container/CI):**
  - `cargo test --workspace`: core, jpeg, i18n, scheduler, layout, selection, export.
  - `cargo clippy -D warnings`, `cargo fmt --check`.
  - `cargo xwin clippy --target x86_64-pc-windows-msvc`.
- **Windows CI:**
  - Build, tests, WARP golden tests (shader against CPU reference: orientations, tile seams, zoom, crop, gray, alpha, text).
  - `blinzel-bench`, `blinzel.exe --bench`, startup smoke test.
  - `blinzel-bench` additionally as an x86-64-v3 build; `blinzel-bench compare` writes both runs side by side into the step summary.
  - ARM64 build.
- **Locally on Windows 11** (developer machine):
  - The rustup host there is `windows-gnu`; builds use `cargo +1.99.1-x86_64-pc-windows-msvc …`.
  - NASM is under `%LOCALAPPDATA%\bin\NASM`, CMake comes from Visual Studio (`Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin`); both must be in the `PATH`.
  - Laptop measurements vary by up to 30 % between consecutive runs (throttling). Therefore measure builds alternately and run libjpeg-turbo alongside as a control.
- **Manually on Windows 11:**
  - Double-click start and browsing (including rapid fire).
  - View modes, snap layouts, menu via mouse and keyboard, language switch.
  - Selection → paste into Paint/Word/GIMP; Ctrl+X/Esc; Ctrl+S with both paths (lossless and re-encoded).
  - Recycle Bin, drag & drop, single instance.
- **Linux CI:** golden test of the wgpu shaders on lavapipe, startup to first image under Xvfb, display profile via `_ICC_PROFILE` on Xvfb, license notices up to date.
- **macOS CI (macos-14):** Clippy and tests, Metal golden test, app bundle with signature check, startup to first image.
- **Manually on a Mac:** opening from the Finder and via drag & drop, HEIC via ImageIO, color profile of a P3 display, menu bar and context menu, batch processing, Gatekeeper notice on first start.
- **Manually on Linux:** GNOME (Wayland) and KDE (X11): startup, moving the window, file dialog, clipboard, `--install`.
- **Release (M7):** SmartScreen notice on first start as described on the download page; update path from v0.x.y to v0.x.y+1 on a clean VM.

## 14. Open items on your side

- ✅ License: MIT.
- 🟡 Release signing key: generated, the public part is in `crates/blinzel-core/src/update.rs`. Open: store the secret part as the secret `RELEASE_SIGNING_KEY` and keep it safe (if it is lost, installed copies no longer accept updates).
- 🟡 Cloudflare: Workers Builds is connected; optionally a custom domain. The secrets for the GitHub workflow are therefore not needed.
- 🟡 Windows measurement in `website/data/race.json` (2026-10-26, laptop Ryzen 7 PRO 6850H, 3 monitors): amigo-blinzel 1.1.0 with default settings vs. Windows Photos, without `--cold`. No other viewers are installed there. `settle` in the raw data counts from the end of the pause after the last key press and is therefore negative for fast viewers.
- ⬜ The same on a Mac (Apple Silicon): `blinzel-race corpus testbilder`, then `blinzel-race run testbilder --cold --json website/data/race-macos.json` (asks once for the `sudo` password). Install the viewers beforehand and grant the terminal "Screen Recording" and "Accessibility".
- ⬜ Legal notice (Impressum): details of the website operator (name, address, contact), in case the site is operated commercially. The website has no page for this yet; only the operator can provide the details.
- ⬜ First manual test on Windows 11 (see Verification).
- ⬜ First manual test on a Mac and on Linux (see Verification).
- ✅ HEVC patents: decided, our own HEVC decoder (`zune-heic`) sits behind the Cargo feature `heic` and is off by default; releases do not include it, HEIC runs via WIC there. Anyone building it themselves enables it with `cargo build --release -p blinzel-app --features heic` (its license texts are then missing from the `THIRD-PARTY-NOTICES*.txt`, which only cover the default build).
- ⬜ Measure the HEIC and AVIF gate on Windows: `set BLINZEL_DECODER=native` or `=wic`, then `blinzel.exe --bench <ordner>` with 12/24 MP files, alternately.

## 15. Measurements (current state)

Linux container, 4 vCPU, one core per decode, bench in quick mode:

| Case | Result |
|---|---|
| JPEG 24 MP, full | turbojpeg YCbCr 118 ms; BGRA 147 ms; zune-jpeg 202 ms |
| JPEG 24 MP, screen (1440p) | 92 ms |
| JPEG 24 MP, 1/8 | 74 ms |
| JPEG 24 MP, progressive | approx. 320 ms, independent of the scaling |
| JPEG 45 MP | YCbCr 190 ms; BGRA 410 ms |
| PNG 12 MP RGB | own path 117 ms; png 132 ms; zune-png 216 ms |
| PNG 8 MP screenshot | own path 8.4 ms; both libraries with RGBA expansion 24–27 ms |
| natural sort, 10k names | 2.5 ms |
| buffer 96 MB | pool 0.6 ms; new 40 ms |
| `blinzel.exe` (x64, release, static CRT) | 1.49 MB (cross build with clang-cl/lld); 2.5 MB with MSVC in CI |

Windows CI (`windows-2025`, without GPU, artifact `windows-bench-and-exe`), before and after the libjpeg-turbo fix (PR #2):

| Case | before | after |
|---|---|---|
| JPEG 24 MP 4:2:0, turbojpeg YCbCr full | 228 ms | 93 ms |
| JPEG 24 MP 4:2:0, 1/8 | 195 ms | 70 ms |
| JPEG 24 MP 4:4:4 / grayscale, full | 284 / 217 ms | 118 / 86 ms |
| JPEG 45 MP, full | 423 ms | 175 ms |
| JPEG 24 MP progressive, full | 658 ms | approx. 300 ms |
| zune-jpeg 24 MP (comparison) | 160 ms | 129 ms; turbojpeg now 1.2–1.4× faster |
| App: open 24 MP (decode to 2250 × 1500) | 240 ms | 87 ms (upload 1 ms) |
| Browsing at 30 keys/s, p99 | 604 ms | 240 ms (19 of 22 from prefetch, median 0 ms) |
| Folder with 10k files | 9.9 ms | 8.8 ms |
| Startup to first present | 617 ms | 150 ms, of which 134 ms decode |

The runner has no GPU: the startup measurement reports `"adapter": "Microsoft Basic Render Driver"`, i.e. software rendering by Windows. Real GPU startup times must be measured on a machine with a graphics card (`blinzel.exe bild.jpg --exit-after-first-frame --json start.json`).

The C part of libjpeg-turbo was unoptimized here: under MSVC with the Visual Studio generator, cmake-rs removes all `/O` flags, and MSBuild then compiles with `/Od`. Fixed by a patched copy of cmake-rs (`third_party/cmake-rs`, via `[patch.crates-io]`) that keeps CMake's default flags per configuration. libjpeg-turbo also builds as "Release" (`/O2 /Ob2`). `blinzel-bench run --check` fails if zune-jpeg ever beats the shipped JPEG path again. The startup measurement records the adapter name so that software rendering is recognizable.

### SIMD levels

Measured locally on AMD Ryzen 7 PRO 6850H (8 cores, AVX2, no AVX-512), Windows 11, on AC power, power plan "Balanced"; medians, one core per decode.

**libjpeg-turbo** on `jpeg-24mp-420.jpg`, each level as a separate child process (`JSIMD_FORCE*`):

| Case | without SIMD | SSE2 | AVX2 | AVX2 vs. SSE2 |
|---|---|---|---|---|
| YCbCr full | 120 ms | 81 ms | 76 ms | 5 % |
| BGRA full (incl. color conversion, upsampling) | 211 ms | 99 ms | 91 ms | 9 % |
| 1/8 | 59 ms | 55 ms | 54 ms | ≈ 0 |

libjpeg-turbo selects AVX2 by itself anyway. The rest is the scalar Huffman decoder, i.e. the lever for M3.

**Own kernels** (`blinzel_core::kernels`) on 12 MP, two runs:

| Kernel | Baseline (v2) | AVX2+FMA |
|---|---|---|
| RGB → RGBA | 6.2 / 5.5 ms | 4.6 / 5.1 ms |
| Gray+alpha → RGBA | 5.0 / 4.7 ms | 4.4 / 4.6 ms |
| RGBA → BGRA | 5.5 / 5.5 ms | 5.5 / 4.8 ms |
| YCbCr 4:2:0 → BGRA (CPU reference, not the display path) | 583 / 530 ms | 500 / 557 ms |

The packed kernels are memory-bound. If AVX2 gains 0.5–1.5 ms there, that is at most 2 % for a 12 MP PNG (≈ 90 ms).

**Upper bound "everything with AVX2"** (x86-64-v3 build, so the crates `png` and `fdeflate` as well): three alternating runs yield 2–5 % for PNG (minima); JPEG stays the same because the C part is unaffected by `RUSTFLAGS`. A single back-to-back comparison had shown 24 %, but the baseline run was throttled.

**Result by the criterion (≥ 10 % on a hot path):** No level reaches it. Therefore there is no runtime dispatch; the kernels run with `Level::Baseline`, the baseline remains x86-64-v2. The kernel module and the bench group `simd` remain as tools for M3 (AVX2 IDCT, color conversion in our own decoder).

**Windows CI** (PR #4, `windows-2025`, AMD EPYC 7763, AVX2; the runs are quiet, the minimum is at the median):

| Measurement | Result |
|---|---|
| libjpeg-turbo YCbCr full: without SIMD / SSE2 / AVX2 | 159 / 100 / 95 ms |
| libjpeg-turbo BGRA full: without SIMD / SSE2 / AVX2 | 253 / 122 / 111 ms |
| libjpeg-turbo 1/8: without SIMD / SSE2 / AVX2 | 72 ms each |
| Kernel RGB → RGBA: baseline / AVX2 | 4.4 / 3.4 ms (1.28×, ≈ 1 % of the 12 MP PNG decode of 113 ms) |
| Kernel gray+alpha → RGBA, RGBA → BGRA: baseline / AVX2 | 3.0 / 2.6 ms; 3.8 / 3.5 ms |
| Kernel YCbCr → BGRA, all levels | 604–636 ms, no difference |
| x86-64-v3 vs. v2: PNG (RGB, RGBA, screenshot) | 1.02× / 1.02× / 1.06× |
| x86-64-v3 vs. v2: JPEG (C code unchanged) | 1.01–1.03× |

This confirms the local measurement. At 1/8, SIMD no longer brings libjpeg-turbo anything at all on the runner.

## 16. Handover: next steps

**Status (2026-10-25), PR #22:** Code signing removed; `blinzel-ui` as a platform-independent interface; batch processing (M9); Linux shell (M10) and macOS shell (M11) with installation, updater, CI and release jobs; license notices per platform; website with downloads for three platforms. Open are the visual checks from section 13 and the items in section 14.

**Status (2026-10-23):**
- PR #1–#3 are merged.
- PR #4 (`simd-levels`): SIMD levels measured, no dispatch ([SIMD levels](#simd-levels)). Waiting for merge.
- PR #5 (`m3-jpeg`, stacked on #4): own JPEG decoder.
  - Sequential (baseline and extended, 8-bit, any subsampling, restart markers, multiple scans), scales 8/8, 4/8, 2/8, 1/8.
  - Parallel: exact with restart markers, otherwise speculative with self-synchronization. A guess that runs into an invalid code restarts one bit later.
  - Byte-identical with libjpeg-turbo; checked in `blinzel-core/tests/own_jpeg.rs` and with `blinzel-bench run --check` over the whole corpus.
  - Integrated via `decode::decode_with(data, req, threads)`: the visible image runs on all P-cores via `blinzel-jpeg`; prefetch, regions and error cases stay with libjpeg-turbo.
  - `blinzel-bench jpeg-ab <datei> [skala] [threads]` measures turbo and `blinzel-jpeg` alternately in the same process.

**M3 measurements** (Ryzen 7 PRO 6850H, 8 cores / 16 threads, A/B alternating, medians):

| File | turbo | blinzel-jpeg, 1 thread | 16 threads | Factor |
|---|---|---|---|---|
| 24 MP 4:2:0 full | 77 ms | 90 ms | 20 ms | 3.9× |
| 24 MP 4:2:0 1/2 | 66 ms | 78 ms | 19 ms | 3.5× |
| 24 MP 4:2:0 1/8 | 57 ms | 57 ms | 16 ms | 3.6× |
| 24 MP 4:4:4 full | 102 ms | 109 ms | 26 ms | 3.9× |
| 45 MP 4:2:0 full | 151 ms | 206 ms | 38 ms | 4.0× |
| 12 MP 4:2:0 full | 37 ms | 45 ms | 12 ms | 3.1× |

App benchmark with a real GPU (Radeon 680M): open a 24 MP JPEG in 28.7 ms total (decode 19.9 ms to 3000 × 2000).

**Progressive** (since this state): All scans are read up front; with multiple threads each scan runs in its own thread, row by row behind the scans it depends on (same component, overlapping band). Coefficients sit in block rows behind mutexes, are allocated only on first access and released during the parallel IDCT. Refinement scans keep a mask of the non-zero coefficients per block.

| 24 MP 4:2:0 progressive | turbo | blinzel-jpeg, 1 thread | 16 threads |
|---|---|---|---|
| full | 244 ms | 330 ms | 142 ms (1.7×) |
| 1/8 | 238 ms | 312 ms | 138 ms (1.7×) |

Limit: The last Y refinement scan (3.7 MB) alone takes approx. 100 ms. Without knowing the block number it cannot be split speculatively; it only gets faster through a faster sequential decoder.

For sequential (baseline) JPEGs, `blinzel-jpeg` has been on par with turbo (1.01×) since the accumulator bit reader, and 4.1× with 16 threads.

**M7 (branch `m7-install`, PR stacked on `m3-jpeg`):** setup exe, uninstallation, silent mode, self-updater with checks and rollback, release workflow, winget templates, menu entries. Details in [section 10](#10-installation-updates-signing-m7). Only items from section 14 (release key) and the first real release are open.

**Startup time (real GPU, Radeon 680M, 24 MP JPEG, process start → first image):**

| State | First image | Direct3D frame |
|---|---|---|
| before | 230–330 ms (waited for `D3D11CreateDevice`, 115–150 ms) | same |
| + message loop instead of blocking, first image via GDI | 65–71 ms | 128–136 ms |
| + `SW_SHOWNA`, rendering on the loader thread | **54–61 ms** | 127–137 ms |

The time until `main` (20–25 ms) is process creation; delay-load changes nothing about that. After that: reading and decoding approx. 25 ms, CPU frame approx. 5 ms, drawing under 1 ms. Side effect: since the UI thread processes messages instead of blocking, `D3D11CreateDevice` now also takes only approx. 95 instead of 150 ms.

**Still open:** measurement from the outside with `blinzel-race` and a visual check of the GDI → Direct3D transition. Both only work with an unlocked screen; the machine was locked during the measurement. Only resident mode (M6) gets below 50 ms.

**Resident handoff (M6, branch `m6-resident`, same machine, 24 MP JPEG):** Measured with `blinzel --resident --json <datei>`: the handoff carries the creation time of the second process, so the marks count from its process start, as with a cold start.

| State | Handoff received → visible (median) | first handoff after startup |
|---|---|---|
| without warm-up | 24.6 ms | 43 ms |
| + decode/upload warmed up at screen size | 24.7 ms | 33 ms |
| + window shown cloaked once | **23–25 ms** (range 22–29 ms) | 23–24 ms |

Of this, reading approx. 2 ms, decoding approx. 19 ms (3000 × 2000, all P-cores), upload approx. 1.5 ms, present under 1 ms. Target ≤ 25 ms reached. The second process needs as long until the handoff as a cold start until `main` (normally 20–25 ms; during this measurement the machine was locked and needed 120–150 ms, the old state too). From the outside, therefore, normally approx. 45–50 ms from double-click to image.

Memory of the hidden instance: 50 MB private after startup, approx. 70 MB after use and stable (60 handoffs, no growth).

**Next, in this order:**
1. With an unlocked screen: visual check of the GDI transition, look at the tray menu and waking, measure `blinzel-race` with cold start and resident.
2. Finish M6: visual check of tray, waking and color management on a monitor with a wide-gamut profile.
3. M4: visual check of grid and filmstrip; then file names under the tiles and a grid scenario in `blinzel-race`.
4. M5 as in section 12.
