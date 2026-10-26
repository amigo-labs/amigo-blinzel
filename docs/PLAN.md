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
| Languages | multilingual, starting with German + English |
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

## 8. Mehrsprachigkeit

- 🟡 `i18n/<sprache>.toml`; `build.rs` erzeugt `enum Msg` und statische Tabellen. Zur Laufzeit ist eine Übersetzung nur ein Array-Index.
- 🟡 Fehlende Keys fallen auf Englisch zurück (Build-Warnung). Platzhalterfehler brechen den Build ab.
- 🟡 Sprachwahl über `GetUserPreferredUILanguages`; im Menü überschreibbar, ohne Neustart.
- 🟡 Plural-Regeln (CLDR-Familien).
- 🟡 RTL-Sprachen: `[meta] direction = "rtl"` in `i18n/<sprache>.toml` ergibt `Lang::is_rtl()`. Dann spiegelt die App die Bedienelemente (Menü rechts, Fensterknöpfe links), öffnet Menüs gespiegelt (`TPM_LAYOUTRTL`) von der rechten Kante, setzt die Leserichtung von DirectWrite und der Meldungsfenster (`MB_RTLREADING`) und tauscht ← und → beim Blättern. Offen: Filmstreifen/Raster und der Setup-Dialog bleiben links-nach-rechts; eine RTL-Sprache liefern wir noch nicht aus (Übersetzung von Muttersprachlern nötig).

## 9. Plattformen

### Windows 11 ab 24H2

Nur Versionen mit Microsoft-Support. Stand Oktober 2026:

| Version | Support |
|---|---|
| Windows 11 25H2 | Home/Pro bis Okt. 2027, Enterprise bis Okt. 2028 |
| Windows 11 24H2 | Enterprise/Education bis Okt. 2027 |

Windows 10 wird nicht unterstützt: Für Home/Pro ist der Support beendet, nur Commercial ESU und LTSC laufen noch.

**Konsequenzen:**
- 🟡 Minimum Direct3D Feature Level 11_0 und Shader Model 5. Jedes Windows-11-Gerät hat eine DX12/WDDM-2-GPU. Keine FL-10-Fallbacks.
- 🟡 Versionsprüfung: Build < 26100 → lokalisierte Meldung beim Start bzw. im Installer.
- 🟡 Anwendungsmanifest: supportedOS, PerMonitorV2, longPathAware, UTF-8-Codepage.
- 🟡 ARM64-Build im CI (cross von x64); Tests laufen auf x64.
- Ohne Fallbacks nutzbar: EcoQoS, CPU-Sets, Snap-Layouts, Dark-Mode-Attribute, WIC-Codecs für HEIF, AVIF und WebP (soweit die Store-Erweiterungen installiert sind).

### macOS und Linux (M10, M11)

Die Hüllen setzen dieselben Traits um wie Windows (`Device`, `Canvas`, `Host`, `Services` in `blinzel-ui/src/backend.rs`). Die Uniforms jedes Zeichenaufrufs kommen für wgpu und Metal aus `blinzel_ui::draws`; die Shader sind Zeile-für-Zeile-Ports von `blinzel.hlsl` und werden wie WARP gegen den CPU-Referenz-Renderer geprüft (lavapipe im Linux-CI, das Metal-Gerät des macOS-Runners).

**Linux:**
- 🟡 Rahmenloses winit-Fenster (Ränder ziehen die Größe, der obere Streifen verschiebt), Menü und Dialoge von der App selbst gezeichnet, Dateidialoge über das xdg-Portal (rfd), Papierkorb, Zwischenablage, Ordner beobachten (inotify), Einzelinstanz über einen Unix-Socket.
- 🟡 Farbmanagement über Little CMS; Ziel ist unter X11 das Profil, das colord oder KDE als `_ICC_PROFILE` am Root-Fenster veröffentlichen (über RandR dem Monitor des Fensters zugeordnet). Wayland hat dafür noch kein Protokoll: dort sRGB.
- 🟡 `blinzel --install` / `--uninstall`: `~/.local/share/amigo-blinzel/`, Link `~/.local/bin/blinzel`, Desktop-Eintrag mit MIME-Typen, Icon; app id/WM_CLASS passen zum Eintrag.
- 🟡 Updater wie unter Windows für die installierte Kopie; Asset `amigo-blinzel-linux-<arch>` (die nackte Datei), Downloads über `curl`.
- Nicht unter Linux: Resident-Modus und Tray.

**macOS:**
- 🟡 NSWindow mit Inhalt unter der transparenten Titelleiste, `CAMetalLayer` als Ebene der Ansicht, Menüleiste (Programm-Menü und „Bild“, beim Öffnen aus dem Menübaum gefüllt) und Kontextmenü als NSMenu, NSOpenPanel/NSSavePanel, Papierkorb, Zwischenablage (PNG), Ordner beobachten (FSEvents), Dateien aus dem Finder über `application:openURLs:`, Drag & Drop.
- 🟡 ImageIO als System-Decoder: HEIC zuerst dort (Hardware-HEVC, Apples Lizenz), RAW, TIFF und Übriges als Rückfall; verkleinerte Anfragen über den Thumbnail-Pfad.
- 🟡 Farbmanagement: Ziel ist das Profil des Bildschirms (`CGDisplayCopyColorSpace`); die Metal-Ebene hat keinen eigenen Farbraum, macOS zeigt ihre Pixel also unverändert, wie unter Windows.
- 🟡 App-Bundle (`blinzel-release macos-bundle`): Info.plist mit Dokumenttypen, Icon (ICNS aus dem Icon-Generator), Universal-Binary per `lipo`, ad hoc signiert, nicht notarisiert. Updater tauscht das Bundle, das vorige bleibt für den Rollback daneben.
- dav1d-Assembler nur auf x86-64: Das veröffentlichte `rav1d` enthält `src/arm/asm-offsets.h` nicht, das seine ARM-Assembler-Dateien einbinden. ARM64 (Mac und Linux) nutzt die Rust-Fallbacks.

## 10. Installation, Updates, Signierung (M7)

### Setup-Exe

- 🟡 **Release-Assets** pro Release auf GitHub:
  - `amigo-blinzel-setup-x64.exe` und `amigo-blinzel-setup-arm64.exe`, das ist dieselbe Exe wie der Viewer.
  - `SHA256SUMS` und `SHA256SUMS.sig`.
  - `THIRD-PARTY-NOTICES.txt` (dieselben Lizenztexte, die die Exe im Menü zeigt).
- 🟡 **Installationsdialog:** Als Setup gestartet (Dateiname enthält `setup` oder `--install`), erscheint ein Dialog in Viewer-Optik, rahmenlos und mehrsprachig.
- 🟡 **Installation pro Benutzer, ohne Admin**, nach `%LOCALAPPDATA%\Programs\amigo-blinzel\`:
  - Startmenü-Verknüpfung (`IShellLinkW`, AppUserModelID).
  - Eintrag unter `HKCU\…\Uninstall`.
  - ProgIDs, Capabilities und RegisteredApplications anlegen.
  - Danach die Standard-Apps-Seite öffnen (programmatisch setzen erlaubt Windows nicht).
  - `SHChangeNotify`.
- 🟡 **Deinstallation:** über die Systemeinstellungen. Die Exe kopiert sich nach `%TEMP%` und räumt von dort auf.
- 🟡 **Silent-Modus:** `--install --silent` und `--uninstall --silent` (für winget).
- 🟡 **Versionsprüfung im Setup:** Build < 26100 → abbrechen mit Meldung.
- 🟡 **winget-Manifest**; ein PR an winget-pkgs nach jedem Release, optional automatisiert.

### Keine Code-Signatur

- ✅ **Entschieden:** Releases werden nicht mit einem Code-Signing-Zertifikat signiert (kein SignPath, kein Authenticode, auf dem Mac keine Notarisierung). Folge: Windows SmartScreen warnt beim ersten Start („Weitere Informationen“ → „Trotzdem ausführen“); die Download-Seite erklärt das.
- 🟡 **Releases automatisch über release-please** (wie bei amigo-native):
  1. Jeder Push auf `main` legt einen Release-PR an oder aktualisiert ihn (`.github/workflows/release-please.yml`). Er enthält die nächste Version in `Cargo.toml` und `Cargo.lock` sowie den `CHANGELOG.md`-Eintrag aus den Conventional Commits seit dem letzten Release. `feat` erhöht vor 1.0 die zweite Stelle, `fix` die dritte; `docs`, `ci`, `chore` usw. lösen kein Release aus.
  2. Wird der Release-PR gemergt, setzt release-please den Tag `v<version>` und legt das Release als Entwurf mit dem Changelog an.
  3. Danach baut `release.yml` alle Plattformen, erzeugt `SHA256SUMS`, signiert sie mit Ed25519, hängt die Dateien an und veröffentlicht das Release. Erst dann sehen es die Updater.
  - `release.yml` lässt sich für einen bestehenden Tag auch von Hand starten.
  - Konfiguration: `release-please-config.json` und `.release-please-manifest.json`. Das Manifest startet bei 0.0.0, das erste Release wird so 0.1.0. `bootstrap-sha` ist der Merge von PR #22.
  - Optional das Secret `RELEASE_PLEASE_TOKEN` (fein granulierter PAT mit Schreibrechten für contents und pull-requests), damit auch auf dem Release-PR die CI läuft. Ohne Token muss unter Settings → Actions → General „Allow GitHub Actions to create and approve pull requests“ an sein.
- ✅ **Eigene Signatur der Prüfsummen (Ed25519).** Den Public Key kennt die App fest (`276b0fbd4a1f9eb68476f0b8a14c5cc0bab403cbca24bcf07d8cff333d3fa9b7`). Sie ist der einzige Herkunftsnachweis: Der Updater installiert nur, was mit dem Release-Schlüssel signiert ist.

**Umsetzung (Branch `m7-install`):**
- `blinzel-app/src/setup/`: Moduserkennung, Installation über ein `Layout` (echte Pfade oder Sandbox für Tests), Registry-Hilfe nur für HKCU, Startmenü-Verknüpfung mit AppUserModelID `amigo-labs.amigo-blinzel`, Setup-Fenster mit Renderer und DirectWrite des Viewers (ohne Direct3D: Meldungsfenster).
- Exit-Codes des Setups: 0 installiert, 1 fehlgeschlagen, 2 Windows zu alt, 3 abgebrochen.
- `packaging/winget/`: Vorlagen der drei Manifeste (Scope user, Silent-Schalter, ProductCode = Uninstall-Schlüssel).
- `.github/workflows/release.yml`: baut x64 und ARM64, erzeugt und signiert `SHA256SUMS`, legt einen Release-Entwurf an.
- Test `blinzel-app/tests/setup.rs`: Installation, Update an Ort und Stelle und Deinstallation in einer Sandbox (Temp-Ordner, eigener HKCU-Zweig).

### Self-Updater (still im Hintergrund)

- 🟡 **Prüfen:**
  - Höchstens 1× pro Tag, frühestens 10 s nach dem Start, auf einem Hintergrund-Thread (nie auf dem kritischen Pfad).
  - Quelle: `GET https://api.github.com/repos/amigo-labs/amigo-blinzel/releases/latest` über WinHTTP, das per `LoadLibrary` geladen wird und den Proxy des Systems nutzt.
- 🟡 **Laden** des passenden Assets (x64/ARM64) plus `SHA256SUMS(.sig)` nach `%LOCALAPPDATA%\amigo-blinzel\updates\`.
- 🟡 **Prüfen in zwei Schritten:**
  1. Ed25519-Signatur der Prüfsummen.
  2. SHA-256 der Exe.

  Erst dann wird getauscht.
- 🟡 **Tauschen:**
  - Die laufende Exe in `.old` umbenennen (unter NTFS erlaubt) und die neue an ihren Platz legen.
  - Der nächste Start nutzt die neue Version ohne Zusatzkosten.
  - `.old` wird beim nächsten Start gelöscht.
- 🟡 **Rollback:** Startet die neue Version dreimal nicht bis zum ersten Frame, wird `.old` zurückgeholt.
- 🟡 **Menü:** „Nach Updates suchen“ und „Über amigo-blinzel (Version)“. Einstellung `auto_update = true`.

**Umsetzung:**
- Entscheidungen ohne Windows in `blinzel-core::update` (Version, Release-JSON über einen kleinen eigenen JSON-Leser, `SHA256SUMS`, Ed25519 über `ed25519-compact`, Zeitplan, Startzähler) mit Unit-Tests.
- Windows-Teil in `blinzel-app/src/update.rs`: WinHTTP und `BCryptHash` werden erst bei Bedarf geladen. Nur eine installierte Kopie aktualisiert sich; Entwickler- und portable Builds nie.
- `blinzel-release keygen | sign | verify` für den eigenen Signaturschlüssel.
- Der öffentliche Schlüssel steht als `RELEASE_KEY` in `crates/blinzel-core/src/update.rs`, der geheime (Seed als Hex) nur im Repository-Secret `RELEASE_SIGNING_KEY`. Ohne `RELEASE_KEY` (`None`) prüft und installiert der Updater nichts. Neuer Schlüssel: `cargo run -p blinzel-release -- keygen <datei>`, den Inhalt der Datei als Secret hinterlegen, die Datei löschen, die ausgegebene Zeile eintragen. Ein Schlüsselwechsel erreicht installierte Kopien nur über ein Release, das noch mit dem alten Schlüssel signiert ist.

## 11. Website und Wettbewerbsvergleich (M8)

### Website auf Cloudflare Workers

- 🟡 `website/`: statisches HTML/CSS mit minimalem JS, extrem schnell (passend zum Produkt), Deutsch und Englisch.
  - Seiten: Start, Funktionen, **Vergleich**, **Benchmarks**, Methodik, Download, Changelog, Datenschutz.
  - Charts werden beim Build als Inline-SVG aus den Benchmark-JSONs erzeugt; keine Chart-Library im Browser.
  - Der Download-Link zeigt auf `releases/latest/download/amigo-blinzel-setup-x64.exe` (stabile URL).
- 🟡 **Deployment** über Cloudflare Workers Static Assets (`wrangler.toml` im Repo-Wurzelverzeichnis).
  - Cloudflare Workers Builds ist mit dem Repo verbunden (Root-Verzeichnis `/`, Standard-Deploy-Befehl). Wrangler führt vor jedem Deploy `website/build.sh` aus: Das installiert die fehlende Rust-Toolchain im Build-Image und erzeugt `website/dist` mit `blinzel-site`.
  - Alternativ deployt `.github/workflows/website.yml`, sobald die Secrets `CLOUDFLARE_API_TOKEN` und `CLOUDFLARE_ACCOUNT_ID` gesetzt sind; mit Workers Builds ist das nicht nötig.
  - Zunächst die `workers.dev`-Subdomain; eine eigene Domain kann jederzeit dazukommen.

### Wettbewerbs-Benchmark `blinzel-race`

Ein eigenes Messwerkzeug für Windows und macOS misst alle Viewer gleich, von außen.

**Umsetzung (`crates/blinzel-race`, Branch `m8-race`):**
- `corpus <dir>`: 24 JPEGs mit 24 MP und je ca. 8 MB (Sensorrauschen, damit der Entropie-Decoder so viel arbeitet wie bei echten Fotos), jedes um eine von sechs Signaturfarben (die farbigen Ecken des RGB-Würfels). Aufeinanderfolgende Bilder unterscheiden sich immer.
- Erkennung: Desktop Duplication auf jedem Monitor (Viewer öffnen dort, wo sie zuletzt waren); jeder präsentierte Frame liefert `LastPresentTime` (QPC) und ein Raster aus 40 × 24 Abtastpunkten.
- Tasten gehen nur raus, solange der Viewer das Vordergrundfenster besitzt.
- `list` zeigt die erkannten Viewer mit Version (Versionsressource; `blinzel.exe` hat jetzt eine).
- `run <dir> [--reps N] [--cold] [--only …] [--viewer name=exe] [--json …]`. `--cold` leert die Standby-Liste und braucht Administratorrechte.
- **macOS** (`capture_macos.rs`, `sys_macos.rs`, `viewers_macos.rs`; der Ablauf in `race.rs` ist derselbe):
  - Erkennung über ScreenCaptureKit auf jedem Display. Ein Frame kommt nur, wenn sich der Bildschirm geändert hat, und trägt seine Anzeigezeit (`mach_absolute_time`). Das Raster skaliert ScreenCaptureKit auf der GPU herunter.
  - Start über die Startdienste (`open -a App datei`), wie im Finder. Beenden höflich über `NSRunningApplication.terminate`, nach 3 s mit `SIGKILL`.
  - Tasten über `CGEvent`, nur solange das vorderste Fenster dem Viewer gehört (`CGWindowListCopyWindowInfo`). `--cold` ruft `purge` über `sudo` auf und fragt dafür einmal am Anfang nach dem Passwort; das Werkzeug selbst läuft nicht als root, sonst starteten auch die Viewer als root.
  - Das Terminal braucht die Freigaben „Bildschirmaufnahme“ und „Bedienungshilfen“; ohne sie bricht `run` mit einem Hinweis ab.
  - Kandidaten: amigo-blinzel, Vorschau, XnView MP, qView, Pixea, ApolloOne, Phoenix Slides (Xee³ wird nicht mehr vertrieben). Vorschau blättert bei einer einzelnen Datei nicht durch den Ordner; für sie gibt es nur den Wert fürs Öffnen.
  - Ergebnis nach `website/data/race-macos.json`; die Website zeigt es als eigenen macOS-Abschnitt (`{{race-macos:…}}`).

**Erste Probeläufe** (Laptop, Ryzen 7 PRO 6850H, Radeon 680M, 3 Monitore; nur Funktionsnachweis, keine Referenzzahlen):

| Viewer | Öffnen warm | Weiter | Dauerfeuer 6 Tasten |
|---|---|---|---|
| amigo-blinzel 0.1.0 | 270–295 ms | 9–14 ms | alle gezeigt |
| ImageGlass 8.8 | 1,8 s | 0,8 s | alle, langsam |
| Windows Photos 2026.11 | 1,2 s | 45 ms | 3 gezeigt, Tasten verschluckt |

- 🟡 **Kandidaten:** IrfanView 64, ImageGlass, XnView MP, FastStone Image Viewer, Windows Fotos; optional JPEGView und qimgv. Jeweils mit Standardeinstellungen; Versionen werden protokolliert.
- 🟡 **Messgrößen:**
  1. **Öffnen:** Zeit vom Prozessstart bis das Bild sichtbar ist, warm und kalt (Standby-Liste geleert).
     - Erkannt über die Desktop Duplication API (Frame-Zeitstempel `LastPresentTime`).
     - Die Testbilder tragen eine eindeutige Farbsignatur.
  2. **Weiterblättern:** `SendInput` (Pfeil rechts), Zeit bis die Farbsignatur wechselt; einzeln nach Leerlauf (Prefetch-Fall) und als Dauerfeuer mit 30 Hz (wie viele Bilder werden wirklich gezeigt).
  3. **Thumbnails** (ab M4): Zeit bis das Raster eines 1000er-Ordners gefüllt ist, für Viewer mit Raster-Modus.
- 🟡 **Fairness und Recht:**
  - Gleiche Maschine, gleiche Dateien, 10 Wiederholungen, Median und p95.
  - Rohdaten, Versionen und Methodik werden veröffentlicht.
  - Damit sachlich, nachprüfbar und relevant, wie es vergleichende Werbung (UWG §6) verlangt.
- ⬜ **Zahlen:** Referenzwerte von einer definierten Referenzmaschine (Hardware wird angegeben). CI-Werte nur als Trend.
- 🟡 **Vergleichstabelle der Funktionen** (`compare.html`, DE/EN): Lizenz, Firmennutzung, Werbung, Telemetrie, ARM64, Downloadgröße, Farbprofile, HEIC/AVIF/JPEG XL/WebP/RAW für IrfanView, ImageGlass, XnView MP, FastStone, JPEGView, qimgv und Windows Fotos. Stand 23.10.2026, jede Zelle verlinkt ihre Quelle; Unbelegtes steht als „nicht belegt“. Vor der Veröffentlichung einmal von Hand gegenlesen. Startzeit und Speicher kommen aus der Referenzmessung.

### Animationen (M5)

- 🟡 **GIF:** `decode::gif::decode_animation` komponiert jeden Frame auf die Leinwand wie Browser: transparente Pixel lassen die Leinwand stehen, Disposal „Hintergrund“ leert das Frame-Rechteck, „Vorheriges“ stellt den Stand davor wieder her; Verzögerungen unter 20 ms spielen mit 100 ms; die Wiederholungszahl aus der NETSCAPE-Erweiterung gilt (sonst einmal). Höchstens 256 MB an Frames, längere Animationen spielen, was hineinpasst.
- Der Loader hat dafür die Stufe `Anim`, nur für das aktuelle Bild und erst, wenn dessen Probe GIF sagt: Das erste Bild erscheint wie bisher sofort, die Animation läuft an, sobald alle Frames auf der GPU sind (480 × 270: 1,2 ms je Frame decodieren). Ein Timer schaltet die Frames weiter; beim Wechsel des Bildes wird die Animation verworfen.
- 🟡 **WebP:** `decode::webp::decode_animation` (`image-webp` komponiert die Frames selbst), gleiche Regeln für Verzögerungen, Wiederholungen und das 256-MB-Budget; die Stufe `Anim` gilt jetzt für GIF und WebP. Ein stilles WebP meldet die Stufe als Standbild.
- 🟡 **JPEG XL:** `decode::jxl::decode_animation` rendert jedes Keyframe (Dauer aus Ticks und `tps`, Wiederholungen aus dem Header), gleiche Regeln und Budget. Stille Dateien prüft schon der Header.
- 🟡 **Abspielen während des Decodierens:** Die Decoder liefern jeden Frame einzeln (`stream_animation` mit Frame-Callback), der Loader lädt ihn sofort auf die GPU und schickt ab dem zweiten Frame alle 200 ms den Stand. Die Wiedergabe startet damit nach zwei Frames; erreicht sie den letzten bisher decodierten, hält sie dort und läuft mit der nächsten Lieferung weiter. Ein Decode-Fehler mittendrin beendet die Animation mit den Frames bis dahin. Sichtprüfung offen (große GIFs).

## 12. Meilensteine

| | Inhalt | Ziel | Status |
|---|---|---|---|
| **M0** | Workspace, CI (Linux + Windows), Korpus, Bench-Harness | Baseline-JSON im CI | 🟡 |
| **M1** | Einzelbild: Startsequenz, Chrome, Menü, Ansichtsmodi, i18n, D3D11, JPEG/PNG/BMP/GIF, WIC-Fallback, EXIF/MPF | t0 → Present ≤ 50 ms warm, ≤ 120 ms kalt; kein weißer Frame | 🟡 |
| **M2** | Blättern, Prefetch, Budgets, Full-Res bei Zoom, Ordner-Watch, P/E-Pools, Auswahl, Strg+C/X/S | Next ≤ 8 ms p99 vorgeladen; 10k-Ordner ≤ 25 ms; Strg+C < 1 ms | 🟡 |
| **M3** | `blinzel-jpeg`: paralleler Decoder | Full-Res 24 MP ≥ 3× turbo (8 Kerne); DC-only ≥ 2× | 🟡 Ziele erreicht (4,1× bzw. 3,6×), progressive 1,7×; CI-Bestätigung steht aus |
| **M4** | Thumbnails: Fast-Paths, Pack-Cache, Atlas, Filmstreifen + Raster | 200 Kacheln aus dem Cache ≤ 30 ms; kalt ≥ 150/s | 🟡 fertig: 287/s kalt, 200 aus dem Cache bis auf dem Bildschirm 16 ms; offen: Sichtprüfung |
| **M5** | WebP (animiert), GIF-Animation, AVIF, HEIC, JXL, TIFF, RAW nativ (jeweils Bench-Gate) | nativ schneller als WIC, sonst bleibt WIC | 🟡 GIF- und WebP-Animation, WebP nativ (Gate: verlustfrei und Animationen nativ, verlustbehaftet WIC sobald geladen), JPEG XL nativ (Gate bestanden, +1,7 MB), TIFF nativ mit parallelen Streifen (sonst WIC), RAW über eingebettete Vorschauen (Entwicklung weiter WIC), Lizenzhinweise der eingebundenen Bibliotheken (`THIRD-PARTY-NOTICES.txt`, Menü „Lizenzhinweise“), AVIF (`rav1d`) nativ, solange WIC ohne Codec ist, HEIC (`zune-heic`) ebenso, aber nur mit dem Feature `heic` (Standard aus, HEVC-Patente); offen: Gate HEIC/AVIF gegen WIC unter Windows |
| **M6** | Tray/Resident, Einstellungen, ICC-Farbmanagement | Resident-Handoff → sichtbar ≤ 25 ms | 🟡 Resident, Tray, Einstellungsmenü, App-Icon, ICC (JPEG/PNG) fertig; Handoff 23–25 ms (Median, ruhiger Rechner); offen: Sichtprüfung |
| **M7** | Setup-Exe, Release-Workflow, Self-Updater, winget | Prüfsummen signiert, Update ohne Benutzeraktion | 🟡 umgesetzt; offen: Release-Schlüssel, erster Release |
| **M8** | `blinzel-race` + Website auf Cloudflare Workers | öffentliche, reproduzierbare Vergleichszahlen | 🟡 Werkzeug, Website und Funktionstabelle (mit Quellen) fertig; Windows gemessen (amigo-blinzel gegen Windows Fotos, ohne kalt); offen: macOS-Messung, Gegenlesen der Tabelle, Cloudflare-Secrets |
| **M9** | Stapelverarbeitung: Engine, Dialog, Mehrfachauswahl | schneller als ImageMagick; Quellen nie überschrieben | 🟡 Engine und Dialog fertig, 8,6 statt 2,1 Bilder/s; offen: Sichtprüfung unter Windows und macOS, Vergleich mit vipsthumbnail/XnConvert |
| **M10** | Linux-Hülle (winit + wgpu), Installation, Updater | Golden-Test auf lavapipe, Start bis zum ersten Bild im CI | 🟡 fertig, im CI geprüft (Xvfb); offen: Sichtprüfung unter Wayland, Startzeiten messen |
| **M11** | macOS-Hülle (AppKit + Metal), App-Bundle, Updater | Metal-Golden-Test, Start bis zum ersten Bild im CI | 🟡 fertig, im macOS-CI gebaut; offen: Sichtprüfung auf einem Mac (Finder, HEIC, Farbprofil, Menüleiste), Startzeiten messen |
| **M12** | Bildschirmfotos (eigener PR nach #22) | Druck-Taste bis Overlay ohne spürbare Verzögerung | 🟡 Windows fertig: Druck-Taste (und Strg+Umschalt+Druck) im Resident-Modus, GDI-Aufnahme des virtuellen Desktops, Overlay aus `blinzel-ui/src/snip.rs` (Fenster unter der Maus hervorgehoben, Ziehen = Bereich, Klick = Fenster, Enter = Bildschirm), PNG nach `Bilder\Screenshots` (einstellbar), optional in die Zwischenablage, öffnet im Viewer; „Öffnen mit…“ und „Bearbeiten mit…“ (E) auf allen Plattformen; CI: `--capture-test`. Offen: Sichtprüfung unter Windows 11, Zeit Druck-Taste → Overlay messen, macOS (ScreenCaptureKit, Carbon-Hotkey, NSStatusItem), Linux (X11 GetImage, Wayland-Portal), HDR |

**Reihenfolge:**
1. PR #1: M0–M2 inklusive Windows-11-Baseline und „Ausschnitt speichern“.
2. M3.
3. M7, damit Releases früh verteilt werden können.
4. M8 (Website mit ersten Vergleichszahlen für Öffnen und Blättern).
5. M4, M5, M6. Die Website-Zahlen werden mit jedem Meilenstein ergänzt.

## 13. Verifikation

- **Linux (Container/CI):**
  - `cargo test --workspace`: core, jpeg, i18n, Scheduler, Layout, Auswahl, Export.
  - `cargo clippy -D warnings`, `cargo fmt --check`.
  - `cargo xwin clippy --target x86_64-pc-windows-msvc`.
- **Windows-CI:**
  - Build, Tests, WARP-Golden-Tests (Shader gegen CPU-Referenz: Orientierungen, Kachelnähte, Zoom, Zuschnitt, Grau, Alpha, Text).
  - `blinzel-bench`, `blinzel.exe --bench`, Startup-Smoke-Test.
  - `blinzel-bench` zusätzlich als x86-64-v3-Build; `blinzel-bench compare` schreibt beide Läufe nebeneinander in die Step-Summary.
  - ARM64-Build.
- **Lokal unter Windows 11** (Entwicklerrechner):
  - Der rustup-Host ist dort `windows-gnu`; gebaut wird mit `cargo +1.99.1-x86_64-pc-windows-msvc …`.
  - NASM liegt unter `%LOCALAPPDATA%\bin\NASM`, CMake kommt aus Visual Studio (`Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin`); beides muss im `PATH` stehen.
  - Laptop-Messungen schwanken zwischen aufeinanderfolgenden Läufen um bis zu 30 % (Drosselung). Builds deshalb abwechselnd messen und libjpeg-turbo als Kontrollgröße mitlaufen lassen.
- **Manuell unter Windows 11:**
  - Doppelklick-Start und Blättern (auch Dauerfeuer).
  - Ansichtsmodi, Snap-Layouts, Menü per Maus und Tastatur, Sprachwechsel.
  - Auswahl → Einfügen in Paint/Word/GIMP; Strg+X/Esc; Strg+S mit beiden Pfaden (verlustfrei und neu kodiert).
  - Papierkorb, Drag & Drop, Einzelinstanz.
- **Linux-CI:** Golden-Test der wgpu-Shader auf lavapipe, Start bis zum ersten Bild unter Xvfb, Bildschirmprofil über `_ICC_PROFILE` auf Xvfb, Lizenzhinweise aktuell.
- **macOS-CI (macos-14):** Clippy und Tests, Metal-Golden-Test, App-Bundle mit Signaturprüfung, Start bis zum ersten Bild.
- **Manuell auf einem Mac:** Öffnen aus dem Finder und per Drag & Drop, HEIC über ImageIO, Farbprofil eines P3-Bildschirms, Menüleiste und Kontextmenü, Stapelverarbeitung, Gatekeeper-Hinweis beim ersten Start.
- **Manuell unter Linux:** GNOME (Wayland) und KDE (X11): Start, Fensterbewegung, Dateidialog, Zwischenablage, `--install`.
- **Release (M7):** SmartScreen-Hinweis beim ersten Start wie auf der Download-Seite beschrieben; Update-Pfad von v0.x.y nach v0.x.y+1 auf einer sauberen VM.

## 14. Offene Punkte, die bei dir liegen

- ✅ Lizenz: MIT.
- 🟡 Release-Signaturschlüssel: erzeugt, der öffentliche Teil steht in `crates/blinzel-core/src/update.rs`. Offen: den geheimen Teil als Secret `RELEASE_SIGNING_KEY` hinterlegen und sicher aufbewahren (geht er verloren, nehmen installierte Kopien keine Updates mehr an).
- 🟡 Cloudflare: Workers Builds ist verbunden; optional eine eigene Domain. Die Secrets für den GitHub-Workflow braucht es damit nicht.
- 🟡 Windows-Messung in `website/data/race.json` (26.10.2026, Laptop Ryzen 7 PRO 6850H, 3 Monitore): amigo-blinzel 1.1.0 mit Standardeinstellungen gegen Windows Fotos, ohne `--cold`. Weitere Viewer sind dort nicht installiert. `settle` in den Rohdaten zählt ab dem Ende der Pause nach dem letzten Tastendruck und ist deshalb bei schnellen Viewern negativ.
- ⬜ Dasselbe auf einem Mac (Apple Silicon): `blinzel-race corpus testbilder`, dann `blinzel-race run testbilder --cold --json website/data/race-macos.json` (fragt einmal nach dem `sudo`-Passwort). Vorher die Viewer installieren und dem Terminal „Bildschirmaufnahme“ und „Bedienungshilfen“ erlauben.
- ⬜ Impressum: Angaben zum Betreiber der Website (Name, Anschrift, Kontakt), falls die Seite geschäftsmäßig betrieben wird. Die Website hat dafür noch keine Seite; die Angaben kann nur der Betreiber liefern.
- ⬜ Erster manueller Test unter Windows 11 (siehe Verifikation).
- ⬜ Erster manueller Test auf einem Mac und unter Linux (siehe Verifikation).
- ✅ HEVC-Patente: entschieden, der eigene HEVC-Decoder (`zune-heic`) steckt hinter dem Cargo-Feature `heic` und ist standardmäßig aus; Releases enthalten ihn nicht, HEIC läuft dort über WIC. Wer selbst baut, schaltet ihn mit `cargo build --release -p blinzel-app --features heic` ein (dann fehlen seine Lizenztexte in den `THIRD-PARTY-NOTICES*.txt`, die nur den Standard-Build abdecken).
- ⬜ Gate HEIC und AVIF unter Windows messen: `set BLINZEL_DECODER=native` bzw. `=wic`, dann `blinzel.exe --bench <ordner>` mit 12/24-MP-Dateien, abwechselnd.

## 15. Messwerte (Stand)

Linux-Container, 4 vCPU, ein Kern je Decode, Bench im Schnellmodus:

| Fall | Ergebnis |
|---|---|
| JPEG 24 MP, voll | turbojpeg YCbCr 118 ms; BGRA 147 ms; zune-jpeg 202 ms |
| JPEG 24 MP, Bildschirm (1440p) | 92 ms |
| JPEG 24 MP, 1/8 | 74 ms |
| JPEG 24 MP, progressiv | ca. 320 ms, unabhängig von der Skalierung |
| JPEG 45 MP | YCbCr 190 ms; BGRA 410 ms |
| PNG 12 MP RGB | eigener Pfad 117 ms; png 132 ms; zune-png 216 ms |
| PNG 8 MP Screenshot | eigener Pfad 8,4 ms; beide Libraries mit RGBA-Expansion 24–27 ms |
| natürliche Sortierung, 10k Namen | 2,5 ms |
| Puffer 96 MB | Pool 0,6 ms; neu 40 ms |
| `blinzel.exe` (x64, Release, statische CRT) | 1,49 MB (Cross-Build mit clang-cl/lld); 2,5 MB mit MSVC im CI |

Windows-CI (`windows-2025`, ohne GPU, Artefakt `windows-bench-and-exe`), vor und nach dem libjpeg-turbo-Fix (PR #2):

| Fall | vorher | nachher |
|---|---|---|
| JPEG 24 MP 4:2:0, turbojpeg YCbCr voll | 228 ms | 93 ms |
| JPEG 24 MP 4:2:0, 1/8 | 195 ms | 70 ms |
| JPEG 24 MP 4:4:4 / Graustufen, voll | 284 / 217 ms | 118 / 86 ms |
| JPEG 45 MP, voll | 423 ms | 175 ms |
| JPEG 24 MP progressiv, voll | 658 ms | ca. 300 ms |
| zune-jpeg 24 MP (Vergleich) | 160 ms | 129 ms; turbojpeg jetzt 1,2–1,4× schneller |
| App: 24 MP öffnen (Decode auf 2250 × 1500) | 240 ms | 87 ms (Upload 1 ms) |
| Blättern mit 30 Tasten/s, p99 | 604 ms | 240 ms (19 von 22 aus dem Prefetch, Median 0 ms) |
| Ordner mit 10k Dateien | 9,9 ms | 8,8 ms |
| Start bis erstes Present | 617 ms | 150 ms, davon 134 ms Decode |

Der Runner hat keine GPU: Die Startmessung meldet `"adapter": "Microsoft Basic Render Driver"`, also Software-Rendering durch Windows. Echte GPU-Startzeiten müssen auf einem Rechner mit Grafikkarte gemessen werden (`blinzel.exe bild.jpg --exit-after-first-frame --json start.json`).

Der C-Teil von libjpeg-turbo war dabei unoptimiert: cmake-rs entfernt unter MSVC mit Visual-Studio-Generator alle `/O`-Flags, MSBuild kompiliert dann mit `/Od`. Behoben durch eine gepatchte Kopie von cmake-rs (`third_party/cmake-rs`, per `[patch.crates-io]`), die CMakes Standard-Flags je Konfiguration behält. libjpeg-turbo baut außerdem als „Release“ (`/O2 /Ob2`). `blinzel-bench run --check` schlägt fehl, falls zune-jpeg den ausgelieferten JPEG-Pfad je wieder schlägt. Die Startmessung schreibt den Adapternamen mit, damit Software-Rendering erkennbar ist.

### SIMD-Stufen

Lokal gemessen auf AMD Ryzen 7 PRO 6850H (8 Kerne, AVX2, kein AVX-512), Windows 11, Netzbetrieb, Energieschema „Ausbalanciert“; Mediane, ein Kern je Decode.

**libjpeg-turbo** auf `jpeg-24mp-420.jpg`, jede Stufe als eigener Kindprozess (`JSIMD_FORCE*`):

| Fall | ohne SIMD | SSE2 | AVX2 | AVX2 gegenüber SSE2 |
|---|---|---|---|---|
| YCbCr voll | 120 ms | 81 ms | 76 ms | 5 % |
| BGRA voll (inkl. Farbumrechnung, Upsampling) | 211 ms | 99 ms | 91 ms | 9 % |
| 1/8 | 59 ms | 55 ms | 54 ms | ≈ 0 |

AVX2 wählt libjpeg-turbo ohnehin selbst. Der Rest ist der skalare Huffman-Decoder, also der Hebel für M3.

**Eigene Kernels** (`blinzel_core::kernels`) auf 12 MP, zwei Läufe:

| Kernel | Baseline (v2) | AVX2+FMA |
|---|---|---|
| RGB → RGBA | 6,2 / 5,5 ms | 4,6 / 5,1 ms |
| Grau+Alpha → RGBA | 5,0 / 4,7 ms | 4,4 / 4,6 ms |
| RGBA → BGRA | 5,5 / 5,5 ms | 5,5 / 4,8 ms |
| YCbCr 4:2:0 → BGRA (CPU-Referenz, nicht der Anzeigepfad) | 583 / 530 ms | 500 / 557 ms |

Die gepackten Kernels sind speichergebunden. Gewinnt AVX2 dort 0,5–1,5 ms, sind das bei einem 12-MP-PNG (≈ 90 ms) höchstens 2 %.

**Obergrenze „alles mit AVX2“** (x86-64-v3-Build, damit auch die Crates `png` und `fdeflate`): Drei abwechselnde Läufe ergeben für PNG 2–5 % (Minima); JPEG bleibt gleich, weil der C-Teil von `RUSTFLAGS` unberührt bleibt. Ein einzelner Vergleich hintereinander hatte 24 % gezeigt, der Baseline-Lauf war aber gedrosselt.

**Ergebnis nach dem Kriterium (≥ 10 % auf einem Hot Path):** Keine Stufe erreicht es. Deshalb gibt es keinen Laufzeit-Dispatch; die Kernels laufen mit `Level::Baseline`, die Basis bleibt x86-64-v2. Kernel-Modul und Bench-Gruppe `simd` bleiben als Werkzeug für M3 (AVX2-IDCT, Farbumrechnung im eigenen Decoder).

**Windows-CI** (PR #4, `windows-2025`, AMD EPYC 7763, AVX2; die Läufe sind ruhig, das Minimum liegt beim Median):

| Messung | Ergebnis |
|---|---|
| libjpeg-turbo YCbCr voll: ohne SIMD / SSE2 / AVX2 | 159 / 100 / 95 ms |
| libjpeg-turbo BGRA voll: ohne SIMD / SSE2 / AVX2 | 253 / 122 / 111 ms |
| libjpeg-turbo 1/8: ohne SIMD / SSE2 / AVX2 | jeweils 72 ms |
| Kernel RGB → RGBA: Baseline / AVX2 | 4,4 / 3,4 ms (1,28×, ≈ 1 % des 12-MP-PNG-Decodes von 113 ms) |
| Kernel Grau+Alpha → RGBA, RGBA → BGRA: Baseline / AVX2 | 3,0 / 2,6 ms; 3,8 / 3,5 ms |
| Kernel YCbCr → BGRA, alle Stufen | 604–636 ms, kein Unterschied |
| x86-64-v3 gegenüber v2: PNG (RGB, RGBA, Screenshot) | 1,02× / 1,02× / 1,06× |
| x86-64-v3 gegenüber v2: JPEG (C-Code unverändert) | 1,01–1,03× |

Das bestätigt die lokale Messung. Bei 1/8 bringt SIMD libjpeg-turbo auf dem Runner gar nichts mehr.

## 16. Übergabe: nächste Schritte

**Stand (2026-10-25), PR #22:** Code-Signing entfernt; `blinzel-ui` als plattformunabhängige Oberfläche; Stapelverarbeitung (M9); Linux-Hülle (M10) und macOS-Hülle (M11) mit Installation, Updater, CI- und Release-Jobs; Lizenzhinweise je Plattform; Website mit Downloads für drei Plattformen. Offen sind die Sichtprüfungen aus Abschnitt 13 und die Punkte in Abschnitt 14.

**Stand (2026-10-23):**
- PR #1–#3 sind gemergt.
- PR #4 (`simd-levels`): SIMD-Stufen gemessen, kein Dispatch ([SIMD-Stufen](#simd-stufen)). Wartet auf Merge.
- PR #5 (`m3-jpeg`, auf #4 gestapelt): eigener JPEG-Decoder.
  - Sequentiell (Baseline und Extended, 8 Bit, jede Unterabtastung, Restart-Marker, mehrere Scans), Skalen 8/8, 4/8, 2/8, 1/8.
  - Parallel: exakt mit Restart-Markern, sonst spekulativ mit Selbstsynchronisation. Ein Rateversuch, der auf einen ungültigen Code läuft, startet ein Bit später neu.
  - Byte-identisch mit libjpeg-turbo; geprüft in `blinzel-core/tests/own_jpeg.rs` und mit `blinzel-bench run --check` über den ganzen Korpus.
  - Eingebunden über `decode::decode_with(data, req, threads)`: Das sichtbare Bild läuft auf allen P-Kernen über `blinzel-jpeg`, Prefetch, Regionen und Fehlerfälle bei libjpeg-turbo.
  - `blinzel-bench jpeg-ab <datei> [skala] [threads]` misst turbo und `blinzel-jpeg` abwechselnd im selben Prozess.

**Messwerte M3** (Ryzen 7 PRO 6850H, 8 Kerne / 16 Threads, A/B abwechselnd, Mediane):

| Datei | turbo | blinzel-jpeg, 1 Thread | 16 Threads | Faktor |
|---|---|---|---|---|
| 24 MP 4:2:0 voll | 77 ms | 90 ms | 20 ms | 3,9× |
| 24 MP 4:2:0 1/2 | 66 ms | 78 ms | 19 ms | 3,5× |
| 24 MP 4:2:0 1/8 | 57 ms | 57 ms | 16 ms | 3,6× |
| 24 MP 4:4:4 voll | 102 ms | 109 ms | 26 ms | 3,9× |
| 45 MP 4:2:0 voll | 151 ms | 206 ms | 38 ms | 4,0× |
| 12 MP 4:2:0 voll | 37 ms | 45 ms | 12 ms | 3,1× |

App-Benchmark mit echter GPU (Radeon 680M): 24-MP-JPEG öffnen 28,7 ms gesamt (Decode 19,9 ms auf 3000 × 2000).

**Progressive** (seit diesem Stand): Alle Scans werden vorab gelesen; mit mehreren Threads läuft jeder Scan in einem eigenen Thread, zeilenweise hinter den Scans, von denen er abhängt (gleiche Komponente, überlappendes Band). Koeffizienten liegen in Blockzeilen hinter Mutexen, werden erst beim ersten Zugriff angelegt und während der parallelen IDCT freigegeben. Verfeinerungsscans führen je Block eine Maske der Nicht-Null-Koeffizienten.

| 24 MP 4:2:0 progressive | turbo | blinzel-jpeg, 1 Thread | 16 Threads |
|---|---|---|---|
| voll | 244 ms | 330 ms | 142 ms (1,7×) |
| 1/8 | 238 ms | 312 ms | 138 ms (1,7×) |

Grenze: Der letzte Y-Verfeinerungsscan (3,7 MB) läuft allein ca. 100 ms. Ohne Kenntnis der Blocknummer lässt er sich nicht spekulativ teilen; schneller wird er nur durch einen schnelleren sequentiellen Decoder.

Sequentiell (Baseline) ist `blinzel-jpeg` seit dem Akkumulator-Bitleser gleichauf mit turbo (1,01×), mit 16 Threads 4,1×.

**M7 (Branch `m7-install`, PR auf `m3-jpeg` gestapelt):** Setup-Exe, Deinstallation, Silent-Modus, Self-Updater mit Prüfungen und Rollback, Release-Workflow, winget-Vorlagen, Menüeinträge. Details in [Abschnitt 10](#10-installation-updates-signierung-m7). Offen sind nur Punkte aus Abschnitt 14 (Release-Schlüssel) und der erste echte Release.

**Startzeit (echte GPU, Radeon 680M, 24-MP-JPEG, Prozessstart → erstes Bild):**

| Stand | Erstes Bild | Direct3D-Frame |
|---|---|---|
| vorher | 230–330 ms (wartete auf `D3D11CreateDevice`, 115–150 ms) | ebenso |
| + Nachrichtenschleife statt Blockieren, erstes Bild über GDI | 65–71 ms | 128–136 ms |
| + `SW_SHOWNA`, Rendern auf dem Loader-Thread | **54–61 ms** | 127–137 ms |

Die Zeit bis `main` (20–25 ms) ist Prozesserzeugung, Delay-Load ändert daran nichts. Danach: Lesen und Decodieren ca. 25 ms, CPU-Frame ca. 5 ms, Zeichnen unter 1 ms. Nebeneffekt: Seit der UI-Thread Nachrichten verarbeitet, statt zu blockieren, dauert auch `D3D11CreateDevice` nur noch ca. 95 statt 150 ms.

**Noch offen:** Messung von außen mit `blinzel-race` und Sichtprüfung des Übergangs GDI → Direct3D. Beides geht nur bei entsperrtem Bildschirm; der Rechner war während der Messung gesperrt. Unter 50 ms kommt nur der Resident-Modus (M6).

**Resident-Handoff (M6, Branch `m6-resident`, gleiche Maschine, 24-MP-JPEG):** Gemessen mit `blinzel --resident --json <datei>`: Die Übergabe trägt die Erzeugungszeit des zweiten Prozesses, die Marken zählen also wie beim Kaltstart ab dessen Prozessstart.

| Stand | Übergabe empfangen → sichtbar (Median) | erste Übergabe nach dem Start |
|---|---|---|
| ohne Vorwärmen | 24,6 ms | 43 ms |
| + Decode/Upload in Bildschirmgröße vorgewärmt | 24,7 ms | 33 ms |
| + Fenster einmal gecloakt gezeigt | **23–25 ms** (Spanne 22–29 ms) | 23–24 ms |

Davon Lesen ca. 2 ms, Decodieren ca. 19 ms (3000 × 2000, alle P-Kerne), Upload ca. 1,5 ms, Present unter 1 ms. Ziel ≤ 25 ms erreicht. Der zweite Prozess braucht bis zur Übergabe so lange wie ein Kaltstart bis `main` (normal 20–25 ms; während dieser Messung war der Rechner gesperrt und brauchte 120–150 ms, auch der alte Stand). Von außen also normal ca. 45–50 ms vom Doppelklick bis zum Bild.

Speicher der versteckten Instanz: 50 MB privat nach dem Start, nach Benutzung ca. 70 MB und stabil (60 Übergaben, kein Wachstum).

**Als Nächstes, in dieser Reihenfolge:**
1. Bei entsperrtem Bildschirm: Sichtprüfung des GDI-Übergangs, Tray-Menü und Aufwecken ansehen, `blinzel-race` mit Kaltstart und Resident messen.
2. M6 abschließen: Sichtprüfung von Tray, Aufwecken und Farbmanagement auf einem Monitor mit Wide-Gamut-Profil.
3. M4: Sichtprüfung von Raster und Filmstreifen; danach Dateinamen unter den Kacheln und ein Raster-Szenario in `blinzel-race`.
4. M5 wie in Abschnitt 12.
