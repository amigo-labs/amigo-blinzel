# Changelog

## 1.0.0 (2026-10-26)


### Features

* screenshots on Windows, "Open with" and "Edit with" ([e6f13be](https://github.com/amigo-labs/amigo-blinzel/commit/e6f13be693a2a26c945e8538ece58ef85f915ee2))
* screenshots on Windows, "Open with"/"Edit with", automated releases ([52dae81](https://github.com/amigo-labs/amigo-blinzel/commit/52dae811ca54558f46e49a23fe232d683bf20a49))
* **website:** light-table redesign, SEO metadata and platform download ([98d7658](https://github.com/amigo-labs/amigo-blinzel/commit/98d7658b58d7edaa2a86b833a4c3e00755d7fec4))
* **website:** light-table redesign, SEO metadata, platform download ([37bb4b5](https://github.com/amigo-labs/amigo-blinzel/commit/37bb4b50f127dfa8f2083cf62790bd248f564e73))

## Changelog

## The first version (0.1.0)

The first version.

- Frameless viewer for Windows 11 24H2 and newer (x64, ARM64): three view modes, zoom, pan, full screen, view-only rotation.
- Native on macOS 14 and newer (one app for Apple silicon and Intel: AppKit, Metal, ImageIO for HEIC and RAW, the menu bar) and on Linux (x64, ARM64; Wayland and X11, Vulkan or OpenGL ES), with the same interface everywhere.
- Batch resizing in the app (Ctrl+B): the marked images or the whole folder to JPEG, PNG, WebP or AVIF; long edge, box, crop to an aspect ratio, percent; sharpening; EXIF and colour profile kept (GPS optional); file name patterns. Source files are never overwritten.
- Selection with copy (pixels decoded only when pasted), view cropping and “save cutout as” (lossless for JPEG where possible).
- Own parallel JPEG decoder, byte-identical with libjpeg-turbo, for the visible image on all performance cores.
- Prefetching in paging direction; embedded camera previews first.
- German and English.
- Third-party notices for every bundled library, in the menu and with each release.
- Installs per user without administrator rights (Windows: the executable itself, Linux: `blinzel --install`, macOS: drag the app into Applications); silent updates, verified against the project's own Ed25519 release key, with automatic rollback. Releases carry no code signing certificate.
