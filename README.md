# amigo-blinzel

An image viewer with one goal: **maximum speed** when opening an image, when flipping through a folder and for thumbnails. For Windows, macOS and Linux, written in Rust.

**Download:** [latest release](https://github.com/amigo-labs/amigo-blinzel/releases/latest) · [changelog](CHANGELOG.md)

## Installation

| | Requirements | File | Installation |
|---|---|---|---|
| **Windows** | Windows 11 24H2 or later, x64 or ARM64 | `amigo-blinzel-setup-x64.exe`, `-arm64.exe` | Start it: the exe is the viewer and its own installer. It installs per user, without administrator rights (`--install --silent` for winget). Uninstall via Settings → Apps. |
| **macOS** | macOS 14 or later, Apple silicon and Intel (one universal app) | `amigo-blinzel-macos.dmg` | Open the DMG and drag `amigo-blinzel.app` onto Applications. |
| **Linux** | x64 or ARM64, Wayland or X11, glibc 2.35 or later, Vulkan or OpenGL ES 3 | `amigo-blinzel-linux-x64.tar.gz`, `-arm64.tar.gz` | `tar xzf amigo-blinzel-linux-x64.tar.gz && ./amigo-blinzel/blinzel --install` puts it in `~/.local/share/amigo-blinzel`, a `blinzel` command in `~/.local/bin` and an entry with "Open with" in the application menu. Uninstall: `blinzel --uninstall`. |

**First start:** the releases carry no code signing certificate from a CA ([why](docs/specs/blinzel.md#no-code-signature)).
- Windows SmartScreen warns once: *More info* → *Run anyway*.
- macOS blocks the first start. On macOS 14: Control-click the app in Finder → *Open*. From macOS 15: open it once, then *Open Anyway* in System Settings → Privacy & Security.

**Updates** come silently in the background: at most one check a day, then the update is downloaded, verified against the project's own Ed25519 signature and used from the next start on. `auto_update = false` in `settings.toml` turns that off.

| | Settings (`settings.toml`) | Cache (thumbnails, updates) |
|---|---|---|
| Windows | `%APPDATA%\amigo-blinzel` | `%LOCALAPPDATA%\amigo-blinzel` |
| macOS | `~/Library/Application Support/amigo-blinzel` | `~/Library/Caches/amigo-blinzel` |
| Linux | `~/.config/amigo-blinzel` | `~/.cache/amigo-blinzel` |

## Usage

The window has no toolbars. The menu (☰) sits at the top left, the window buttons at the top right (on macOS the system buttons on the left, ☰ on the right, and every command is also in the menu bar). Everything else is in the menu or on a key. On macOS, ⌘ works wherever Ctrl is listed.

| Key / mouse | Action |
|---|---|
| → / Space / Page Down, ← / Backspace / Page Up | next / previous image (when zoomed: the arrows pan) |
| Home / End | first / last image |
| 1 / 2 / 3 | view: fit window to image (original size) / fit image to window / 100 % |
| Mouse wheel, + / −, 0 | zoom around the cursor, reset zoom |
| Left drag | image fits the window: select · image larger than the window: pan (Shift+drag selects) |
| Middle mouse button | pan |
| Ctrl+C / Ctrl+X / Ctrl+A | copy selection / cut selection (the view is cropped) / select all |
| Ctrl+S | save cutout as … (JPEG losslessly where possible, PNG, WebP, AVIF) |
| T / G | filmstrip / grid of the folder |
| Ctrl+B | resize images (batch) |
| E | edit with … |
| R / L | rotate (view only) |
| F11 / Enter / double-click | full screen |
| Del | move to the Recycle Bin / Trash |
| Ctrl+O, drag & drop | open |
| Esc | clear selection → show original → leave full screen → close |

Image files are never modified; every result is a new file. Copying finishes instantly: the pixels are decoded only when pasting, and only the selected region, at full resolution.

**Further features:**
- **Batch processing** (Ctrl+B): resize many images at once to JPEG, PNG, WebP, AVIF or the source format. Optionally crop to an aspect ratio and sharpen. Metadata is kept, GPS removed on request, files renamed by a pattern such as `{name}_{w}x{h}`.
- **Formats:** JPEG, PNG, GIF, BMP, WebP, AVIF, TIFF, HEIC, RAW and more. Native decoders where they are faster, otherwise the system decoder (WIC, ImageIO). On Linux, RAW shows the embedded preview.
- **Color management:** images are converted to the display's profile.
- **Resident mode** (Windows, macOS): it stays in the tray or menu bar with a warm GPU, so the next image opens even faster.
- **Screenshots** in resident mode: Print on Windows, ⌘⇧1 on macOS. Pick a window, a region or the screen. The shot always goes to the clipboard, and optionally to a file, the viewer or the editor.
- **Languages:** English, German, Spanish, French, Italian, Japanese, Portuguese (Brazil), Russian, Simplified Chinese. The app follows the system language; the menu can override it.

## Where the speed comes from

- **Decoding at screen resolution.** JPEGs are decoded via DCT scaling directly into YCbCr planes; the shader does the color conversion and upsampling. Embedded camera previews appear first.
- **Its own parallel JPEG decoder.** `blinzel-jpeg` decodes the visible image on all P-cores, even without restart markers (speculative Huffman decoding with self-synchronization). The result is byte-identical with libjpeg-turbo.
- **Prefetch in the browsing direction.** A priority scheduler with cancellation loads ahead; the results wait as GPU textures.
- **Parallel startup.** The window is created invisible. Decoding and GPU setup run in parallel, the folder listing in the background. The window appears with the first image.
- **Native stack per platform.** Win32 + Direct3D 11, AppKit + Metal, winit + wgpu. No runtime; on Windows a static exe of a few MB.
- **Benchmark gate.** Every library is only a yardstick: where an in-house build is faster, it replaces the library (`crates/blinzel-bench`). The measurements and the comparison with other viewers are on the website.

## Structure

| Crate | Contents |
|---|---|
| `blinzel-core` | decoders (JPEG via libjpeg-turbo, PNG, GIF, BMP …), EXIF/MPF, layout, selection, scheduler, caches, thumbnails, batch engine, updater logic, CPU reference renderer. Platform-independent, tested on Linux |
| `blinzel-jpeg` | its own JPEG parser and parallel decoder (sequential and progressive), byte-identical with libjpeg-turbo |
| `blinzel-ui` | the interface, the same on every platform: menu, dialogs, filmstrip, grid, batch dialog, screenshot overlay, settings |
| `blinzel-i18n` | translations from `i18n/*.toml`, compiled into static tables |
| `blinzel-app` | the `blinzel` binary: one native shell per platform (Win32/D3D11, AppKit/Metal, winit/wgpu), loading pipeline, system decoders, installer, updater |
| `blinzel-bench` | test corpus generator and benchmarks library vs. in-house build (`run --check` also verifies that `blinzel-jpeg` is byte-identical) |
| `blinzel-race` | competitive comparison: measures viewers from the outside, on Windows via Desktop Duplication, on macOS via ScreenCaptureKit |
| `blinzel-release` | release tooling: keys, signing and verifying `SHA256SUMS`, macOS app bundle and DMG, license notices |
| `blinzel-site` | builds the website (`website/`) with the charts as inline SVG |

## Building

Rust stable (pinned in `rust-toolchain.toml`), CMake, a C compiler and, on x86-64, NASM (libjpeg-turbo's SIMD code). Plus:
- **Windows:** Visual Studio Build Tools and the Windows SDK (for `fxc`).
- **macOS:** Xcode Command Line Tools.
- **Linux:** to run the tests without a GPU, lavapipe (`mesa-vulkan-drivers`), as in `.github/workflows/ci.yml`.

```
cargo build --release -p blinzel-app      # target/release/blinzel(.exe)
cargo test --workspace
```

All platform-independent tests run on Linux. `cargo xwin check --target x86_64-pc-windows-msvc` checks the Windows app from Linux.

Benchmarks:

```
cargo run --release -p blinzel-bench -- corpus testdata/generated
cargo run --release -p blinzel-bench -- run testdata/generated --json bench.json
target/release/blinzel --bench testdata/generated --json bench-app.json
```

Competitive comparison and website:

```
cargo run --release -p blinzel-race -- corpus race-images
cargo run --release -p blinzel-race -- run race-images --json website/data/race.json        # Windows
cargo run --release -p blinzel-race -- run race-images --json website/data/race-macos.json  # macOS
cargo run --release -p blinzel-site                       # website/dist
```

## Releases

Releases are automatic: every push to `main` updates a release PR (release-please, from the Conventional Commits). Merging it tags the version, and `release.yml` builds every platform, signs `SHA256SUMS` and publishes the release. [docs/SECRETS.md](docs/SECRETS.md) describes the secrets this needs.

## Documentation

- [docs/specs/blinzel.md](docs/specs/blinzel.md): the design spec with the goal, every decision, the architecture and the measurements.
- [docs/specs/](docs/specs/README.md): one feature spec per feature, with acceptance criteria and the tests that prove them, plus the open items.
- [AGENTS.md](AGENTS.md): how to work on the code (conventions, tests, the spec workflow with Claude Code).
- [docs/SECRETS.md](docs/SECRETS.md): the secrets of the workflows and what they are for.
- [CHANGELOG.md](CHANGELOG.md): the changes per release.

## License

[MIT](LICENSE). The license texts of the bundled libraries (libjpeg-turbo, libwebp, Little CMS, Rust crates) are, per platform, in [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt) (Windows), [THIRD-PARTY-NOTICES-linux.txt](THIRD-PARTY-NOTICES-linux.txt) and [THIRD-PARTY-NOTICES-macos.txt](THIRD-PARTY-NOTICES-macos.txt), and in the menu under "Third-party notices". After a change to `Cargo.lock`, `cargo run -p blinzel-release -- notices` regenerates the files; CI checks this.
