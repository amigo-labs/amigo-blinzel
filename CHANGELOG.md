# Changelog

## [1.2.1](https://github.com/amigo-labs/amigo-blinzel/compare/v1.2.0...v1.2.1) (2026-10-28)


### Bug Fixes

* **screenshot:** retry the Print key at sign-in and say when Windows keeps it ([bf453fa](https://github.com/amigo-labs/amigo-blinzel/commit/bf453fa9a5a0fbcf72609d340a177eae9e091bba))
* **screenshot:** retry the Print key at sign-in and say when Windows keeps it ([03dc373](https://github.com/amigo-labs/amigo-blinzel/commit/03dc373bbf44bfbf2487427a9e4a8ea0062d25c6))
* **screenshot:** tell in the tray when Windows keeps the Print key ([3c5610a](https://github.com/amigo-labs/amigo-blinzel/commit/3c5610ab2d2b015583a5fe4871b5482474572605))

## [1.2.0](https://github.com/amigo-labs/amigo-blinzel/compare/v1.1.0...v1.2.0) (2026-10-26)


### Features

* **batch:** drop zone, date formats and a name preview for resizing ([922816c](https://github.com/amigo-labs/amigo-blinzel/commit/922816c2797acf33706ac2041759e26b4569d05f))
* **batch:** drop zone, date formats and name preview; factual website with Windows measurement ([e82aa18](https://github.com/amigo-labs/amigo-blinzel/commit/e82aa18357e293819e9f877dcec6cd5230525dde))
* **website:** Windows measurement from blinzel-race ([294fc10](https://github.com/amigo-labs/amigo-blinzel/commit/294fc10d7804bba03112d8341c3952f76e69beff))


### Bug Fixes

* **setup:** don't abort when the setup window is re-entered ([266d0de](https://github.com/amigo-labs/amigo-blinzel/commit/266d0de62d60bcdebe4920ae3937944606d747f7))

## [1.1.0](https://github.com/amigo-labs/amigo-blinzel/compare/v1.0.0...v1.1.0) (2026-10-26)


### Features

* screenshots on Windows, "Open with" and "Edit with" ([e6f13be](https://github.com/amigo-labs/amigo-blinzel/commit/e6f13be693a2a26c945e8538ece58ef85f915ee2))
* screenshots on Windows, "Open with"/"Edit with", automated releases ([52dae81](https://github.com/amigo-labs/amigo-blinzel/commit/52dae811ca54558f46e49a23fe232d683bf20a49))
* **website:** light-table redesign, SEO metadata and platform download ([98d7658](https://github.com/amigo-labs/amigo-blinzel/commit/98d7658b58d7edaa2a86b833a4c3e00755d7fec4))
* **website:** light-table redesign, SEO metadata, platform download ([37bb4b5](https://github.com/amigo-labs/amigo-blinzel/commit/37bb4b50f127dfa8f2083cf62790bd248f564e73))


### Bug Fixes

* **ci:** release tags v&lt;version&gt; and no failing env without a release PR ([f958528](https://github.com/amigo-labs/amigo-blinzel/commit/f958528f2f0dc95bd1b73472bcb54046d366e433))
* **ci:** release tags v&lt;version&gt;, no failing env without a release PR ([876de52](https://github.com/amigo-labs/amigo-blinzel/commit/876de52c140c897a43c391737fdaab2c67179c17))

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
