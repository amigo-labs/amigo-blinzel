# amigo-blinzel

An image viewer for Windows with one goal: **maximum speed** when opening, when flipping through images and (soon) for thumbnails.

## Usage

The interface has no toolbars. The menu (☰) sits at the top left; minimize, maximize and close sit at the top right. Everything else lives in the menu or on a key.

| Key / mouse | Action |
|---|---|
| → / Space / Page Down, ← / Backspace / Page Up | next / previous image (when zoomed: arrows pan) |
| Home / End | first / last image |
| 1 / 2 / 3 | View: fit window to image (original size) / fit image to window / 100 % |
| Mouse wheel, + / −, 0 | zoom around the cursor, reset zoom |
| Left drag | image fits the window: select · image larger than the window: pan (Shift+drag = select) |
| Middle mouse button | pan |
| Ctrl+C / Ctrl+X / Ctrl+A | copy selection / cut selection (the view is cropped) / select all |
| Ctrl+S | save cutout as … (lossless JPEG where possible) |
| Esc | clear selection → show original → leave full screen → close |
| R / L | rotate (view only) |
| F11 / Enter / double-click | full screen |
| Del | move to the Recycle Bin |
| Ctrl+O, drag & drop | open |

Image files are never modified. Copying finishes instantly: the pixels are decoded only when pasting, and only the selected region, at full resolution.

A cutout can be saved as a new file. For JPEG sources, the DCT coefficients are carried over losslessly where possible instead of being re-encoded.

**System requirements:** Windows 11 version 24H2 or later, x64 or ARM64.

**Installation:** The exe is also the installer. Started as `amigo-blinzel-setup-*.exe` (or with `--install`), it installs itself per user without administrator rights; `--install --silent` for winget. Uninstall via Windows Settings. It checks for updates silently in the background at most once a day (`auto_update = false` in `%APPDATA%\amigo-blinzel\settings.toml` turns that off).

The complete plan with all decisions and checklists is in [docs/PLAN.md](docs/PLAN.md).

## Where the speed comes from

- **Decoding at screen resolution.** JPEGs are decoded via DCT scaling directly into YCbCr planes; color conversion and upsampling are done by the shader. Embedded camera previews appear first.
- **Custom parallel JPEG decoder.** `blinzel-jpeg` decodes the visible image on all P-cores, even without restart markers (speculative Huffman decoding with self-synchronization), byte-identical with libjpeg-turbo.
- **Prefetch in the browsing direction.** A priority scheduler with cancellation loads ahead; results are ready as GPU textures.
- **Parallel startup.**
  - The window is created invisible, without DWM animation.
  - File decode and D3D11 init run in parallel, the folder listing in the background.
  - The window becomes visible with the first image.
- **Native stack.** Win32 + Direct3D 11 + precompiled shaders, a static exe of about 2.5 MB, no runtime.
- **Benchmark gate.** Every library is only a yardstick; where the in-house build wins, it gets used (`crates/blinzel-bench`).

## Structure

| Crate | Contents |
|---|---|
| `blinzel-core` | Decoders (JPEG via libjpeg-turbo, PNG, GIF, own BMP), EXIF/MPF, layout, selection, scheduler, caches, CPU reference renderer – platform-independent, tested on Linux |
| `blinzel-jpeg` | own JPEG parser and parallel decoder (sequential and progressive), byte-identical with libjpeg-turbo |
| `blinzel-i18n` | translations from `i18n/*.toml`, compiled into static tables at compile time (DE, EN) |
| `blinzel-app` | `blinzel.exe`: window, D3D11 renderer, loading pipeline, WIC fallback (TIFF, HEIC, AVIF, WebP, RAW …) |
| `blinzel-bench` | test corpus generator and benchmarks library vs. in-house build (`run --check` also verifies that `blinzel-jpeg` is byte-identical) |
| `blinzel-race` | competitive comparison: measures viewers from the outside, on Windows via Desktop Duplication, on macOS via ScreenCaptureKit |
| `blinzel-release` | generate release keys, sign and verify `SHA256SUMS` |
| `blinzel-site` | builds the website (`website/`) with charts as inline SVG |

## Building

Windows with Rust (stable), Visual Studio Build Tools, Windows SDK (for `fxc`), CMake and NASM:

```
cargo build --release -p blinzel-app      # target/release/blinzel.exe
cargo test --workspace
```

Benchmarks:

```
cargo run --release -p blinzel-bench -- corpus testdata/generated
cargo run --release -p blinzel-bench -- run testdata/generated --json bench.json
target/release/blinzel.exe --bench testdata/generated --json bench-app.json
```

Competitive comparison and website:

```
cargo run --release -p blinzel-race -- corpus race-images
cargo run --release -p blinzel-race -- run race-images --json website/data/race.json        # Windows
cargo run --release -p blinzel-race -- run race-images --json website/data/race-macos.json  # macOS
cargo run --release -p blinzel-site                       # website/dist
```

All platform-independent tests run on Linux. `cargo xwin check --target x86_64-pc-windows-msvc` checks the Windows app.

## License

[MIT](LICENSE). The license texts of the bundled libraries (libjpeg-turbo, libwebp, Little CMS, Rust crates) are, per platform, in [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt) (Windows), [THIRD-PARTY-NOTICES-linux.txt](THIRD-PARTY-NOTICES-linux.txt) and [THIRD-PARTY-NOTICES-macos.txt](THIRD-PARTY-NOTICES-macos.txt), and in the menu under "Third-party notices". After a change to `Cargo.lock`, `cargo run -p blinzel-release -- notices` regenerates the files; CI checks this.
