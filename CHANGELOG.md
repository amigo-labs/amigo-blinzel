# Changelog

## 0.1.0 (not released yet)

The first version.

- Frameless viewer for Windows 11 24H2 and newer (x64, ARM64): three view modes, zoom, pan, full screen, view-only rotation.
- Selection with copy (pixels decoded only when pasted), view cropping and “save cutout as” (lossless for JPEG where possible).
- Own parallel JPEG decoder, byte-identical with libjpeg-turbo, for the visible image on all performance cores.
- Prefetching in paging direction; embedded camera previews first.
- German and English.
- Third-party notices for every bundled library, in the menu and with each release.
- Installs itself per user without administrator rights; silent updates with three checks and automatic rollback.
