# Changelog

## [1.6.0](https://github.com/amigo-labs/amigo-blinzel/compare/v1.5.0...v1.6.0) (2026-11-09)


### Features

* the app is called Blinzel ([0f333a1](https://github.com/amigo-labs/amigo-blinzel/commit/0f333a13b3d3c33759c55f434b18495f9b2d83a6))


### Bug Fixes

* **app:** tray icon fills its slot at 125 % and 150 % scaling ([362bb05](https://github.com/amigo-labs/amigo-blinzel/commit/362bb057247f6eeb4bd697f4e338a2e23d676f8d))

## [1.5.0](https://github.com/amigo-labs/amigo-blinzel/compare/v1.4.0...v1.5.0) (2026-11-08)


### Features

* Bl pixel monogram as app icon and site colour ([bed526b](https://github.com/amigo-labs/amigo-blinzel/commit/bed526b015448e5ab03baae245735e5cc5f4f25b))
* **race:** benchmark file from blinzel-race, retire the website ([1aee6a3](https://github.com/amigo-labs/amigo-blinzel/commit/1aee6a30eac3ae1c58fbcfea78ef06858da5a912))
* **race:** blinzel-race site writes the amigo-labs.dev benchmark file ([967eb0a](https://github.com/amigo-labs/amigo-blinzel/commit/967eb0ab304fb76e1847674c57c9e3d201d78cdf)), closes [#50](https://github.com/amigo-labs/amigo-blinzel/issues/50)
* the Bl pixel monogram as app icon ([7229c7a](https://github.com/amigo-labs/amigo-blinzel/commit/7229c7a3e5e4987a486b9c7643949b2e5f5ae448))


### Bug Fixes

* **race:** check out the generated benchmark files with LF ([bfc1c39](https://github.com/amigo-labs/amigo-blinzel/commit/bfc1c39af3c1bf2e5897e91a3daba9da3a9abdf3)), closes [#50](https://github.com/amigo-labs/amigo-blinzel/issues/50)

## [1.4.0](https://github.com/amigo-labs/amigo-blinzel/compare/v1.3.0...v1.4.0) (2026-11-03)


### Features

* macOS disk image download and self-signed app signature ([e138f08](https://github.com/amigo-labs/amigo-blinzel/commit/e138f08be18a33e1455a20c618f81b029628a7ad))
* macOS disk image download and self-signed app signature ([5f6cd2f](https://github.com/amigo-labs/amigo-blinzel/commit/5f6cd2f5bd2285485bb0b67cfeef8021a5532de2))
* one signing certificate for amigo-labs, created by a script ([661e436](https://github.com/amigo-labs/amigo-blinzel/commit/661e4368e320006c26b8594812cac2dbf8b87018))
* resident mode on macOS with a menu bar icon and login item ([ceaa5fe](https://github.com/amigo-labs/amigo-blinzel/commit/ceaa5fe11623d325013abdb0eb028837e6d94aee))
* resident mode on macOS with a menu bar icon and login item ([5f3c5f6](https://github.com/amigo-labs/amigo-blinzel/commit/5f3c5f6a97bfae0a75679151da325e0c3b6ba78b))
* screenshots always go to the clipboard, then save/open/edit as set ([57203f9](https://github.com/amigo-labs/amigo-blinzel/commit/57203f99fdf36680cf648fad718efe202a22a97c))
* screenshots always go to the clipboard, then save/open/edit as set ([46b29f3](https://github.com/amigo-labs/amigo-blinzel/commit/46b29f3ca241bf6d33c6aab05cf087b1d3a4e437))
* screenshots on macOS with ScreenCaptureKit and a Cmd+Shift+1 hot key ([3b023f0](https://github.com/amigo-labs/amigo-blinzel/commit/3b023f09b08c0655232751ad613a5e761480e036))
* screenshots on macOS with ScreenCaptureKit and a Cmd+Shift+1 hot key ([dedfd7c](https://github.com/amigo-labs/amigo-blinzel/commit/dedfd7c632b22e7e3ca9b0c711fe7ad6e7d69d2a))
* **ui:** decode a copied region in the shared UI crate (AC-08) ([164def7](https://github.com/amigo-labs/amigo-blinzel/commit/164def7c8df6c31e8ab59678abd41a57bd6ba70b))
* **ui:** decode a copy on its own thread with copy_in_background (AC-10, AC-11) ([546c242](https://github.com/amigo-labs/amigo-blinzel/commit/546c24247cd8c38af71e3ea12c5751f8dd7ab92b))
* **ui:** save a cutout on a shared background thread (AC-20) ([b7855a3](https://github.com/amigo-labs/amigo-blinzel/commit/b7855a39e1c93dd8ba4540fca6f2415b8efbb9e4))
* **ui:** the batch dialog starts with the files marked in the grid (AC-23) ([860e7cf](https://github.com/amigo-labs/amigo-blinzel/commit/860e7cf05c524da5d3797b407d024d9f6462a3ed))


### Bug Fixes

* **app:** the shells exit with the App's exit code (AC-09) ([c2afca3](https://github.com/amigo-labs/amigo-blinzel/commit/c2afca34821970bb6db219f21f9ea63b76f833cf))
* **bench:** a tie with zune-jpeg no longer fails the JPEG gate ([80d77b1](https://github.com/amigo-labs/amigo-blinzel/commit/80d77b1ca0e16439ddd102fb6e28d4f3fa742478))
* benchmark gate ignores a noisy median when the minimum disagrees ([258b798](https://github.com/amigo-labs/amigo-blinzel/commit/258b798cdba7b73e3f694d335f202ff0ae7111b7))
* benchmark gate ignores a noisy median when the minimum disagrees ([fbadcec](https://github.com/amigo-labs/amigo-blinzel/commit/fbadcec399092795e62a471a38b1ab789e14692f))
* **bench:** the CLI test points TMP and TEMP at its temp folder too, as Windows reads those ([58a0e29](https://github.com/amigo-labs/amigo-blinzel/commit/58a0e29110709c08a59e316afa1b9980ad27e446))
* **core:** a file's cache id folds case on Windows only ([8cfbc30](https://github.com/amigo-labs/amigo-blinzel/commit/8cfbc3013c9d28f654b0d2e16c11f5e1c9ea199a))
* **core:** an animated GIF with a 0×0 screen plays at the first frame's extent ([eade330](https://github.com/amigo-labs/amigo-blinzel/commit/eade330b6c1248bf19793bd5b1632d832c93b5c1))
* **core:** an AVIF with major brand mif1 is detected as AVIF, not HEIF ([7346c4f](https://github.com/amigo-labs/amigo-blinzel/commit/7346c4f780f675e8487a0ae6dc5e8560224c0893))
* **core:** batch copies drop the source's TIFF tile tags (AC-09) ([5d34a0c](https://github.com/amigo-labs/amigo-blinzel/commit/5d34a0c04fc3c45ce31e0f88c969e5695976951e))
* **core:** drop ram_budget_mb and read settings more strictly (AC-02, AC-04, AC-07, AC-09, AC-16) ([f66b7c5](https://github.com/amigo-labs/amigo-blinzel/commit/f66b7c59551f192a37f26692be4dbc67d9aba6d6))
* **core:** Format::is_native names TIFF, AVIF and (with heic) HEIF ([1261425](https://github.com/amigo-labs/amigo-blinzel/commit/1261425dbbadac36f3ed11bc43aeff8b040f5f3b))
* **core:** gray + alpha TIFFs decode natively ([d0e2932](https://github.com/amigo-labs/amigo-blinzel/commit/d0e2932b8156bcd539d21dc97cb222f8708b6233))
* **core:** hex keys, seeds and checksums take hex digits only (release AC-10) ([4bb233a](https://github.com/amigo-labs/amigo-blinzel/commit/4bb233ab16156a434a009843ccc8e122e41c846d))
* **core:** read a HEIC's colour profile from the container past its essential hvcC (AC-03) ([d9de6fa](https://github.com/amigo-labs/amigo-blinzel/commit/d9de6fa211d148e828f41de8a7b47d35fbd25b74))
* **core:** the GIF probe takes the first frame's extent for a 0×0 screen ([a08be66](https://github.com/amigo-labs/amigo-blinzel/commit/a08be66134c6405ce562a1cb04b357261406f36c))
* **core:** the thumbnail cache key folds case on Windows only ([e5f76a9](https://github.com/amigo-labs/amigo-blinzel/commit/e5f76a9f36c8c0c25decbf14e93a52071c1d535b))
* **core:** write maximized only together with the window rect (AC-14) ([c1d52c9](https://github.com/amigo-labs/amigo-blinzel/commit/c1d52c916b54e6042b4d0c3045b57e96c7dc2d77))
* **linux:** a drop of several files reaches the App as one drop (AC-23) ([e837a06](https://github.com/amigo-labs/amigo-blinzel/commit/e837a068fd9deed0a19252b712c552e237c6024c))
* **macos:** a --resident start with the setting off removes the LaunchAgent (AC-19) ([64b4557](https://github.com/amigo-labs/amigo-blinzel/commit/64b4557bd842d968a178c7586649e542d02f0c66))
* **macos:** files dropped onto the batch dialog fill its list (AC-23) ([f293813](https://github.com/amigo-labs/amigo-blinzel/commit/f293813ec8083e59e7b26045aa3c7ee34b1651c2))
* **macos:** name the icon type for ImageIO, which does not sniff it from data (AC-23) ([118476a](https://github.com/amigo-labs/amigo-blinzel/commit/118476ad2693d41b50fc5ede5a76d2322bd83eb8))
* **macos:** the batch drop handler compiles inside define_class ([c5f3220](https://github.com/amigo-labs/amigo-blinzel/commit/c5f32205a22d9171a7e4ba27872c3699952d6211))
* **release:** sign refuses an all-zero seed instead of panicking (AC-09, AC-10) ([a3c56f9](https://github.com/amigo-labs/amigo-blinzel/commit/a3c56f9824186bd828f7665b9331fd0ed78266cf))
* signing certificate script refuses to be sourced ([7d7fc93](https://github.com/amigo-labs/amigo-blinzel/commit/7d7fc932cfd75ddb6aa8be5319357f96d71b7b1e))
* signing certificate script refuses to be sourced ([e6d8156](https://github.com/amigo-labs/amigo-blinzel/commit/e6d8156baf58510348f3d441b6a95b2308ac69fd))
* **site:** an unknown race chart kind fails the build before its report exists (AC-12) ([5360caa](https://github.com/amigo-labs/amigo-blinzel/commit/5360caa1fd047e1029e760563475051e6bc23656))
* **site:** the download button offers the Linux files only on Linux, not on a BSD desktop (AC-16, AC-17) ([8c06691](https://github.com/amigo-labs/amigo-blinzel/commit/8c066914a017164947e8793bd8ba9cf472946019))
* **ui:** a click on a pannable image clears the selection (AC-05..AC-08, AC-12..AC-14) ([15ea3d4](https://github.com/amigo-labs/amigo-blinzel/commit/15ea3d4a7c2582c1a3d2a0ff781bf2f07d1faf2a))
* **ui:** a decode error midway keeps the animation's loop count (AC-11, AC-12) ([4b229e6](https://github.com/amigo-labs/amigo-blinzel/commit/4b229e64d34585c397ec89c4b6d9b228a1ddcd3d))
* **ui:** a measurement without an image quits with exit code 1 (AC-09, AC-11) ([63ceb52](https://github.com/amigo-labs/amigo-blinzel/commit/63ceb521b867e448f47dfa72cf7baa66c13c582c))
* **ui:** animation decoding runs inside catch_unwind (AC-11) ([8086406](https://github.com/amigo-labs/amigo-blinzel/commit/808640676bb0942fb25a7cfcde5a26e9b91bf43b))
* **ui:** Delete in the grid recycles the selected tile ([386cd8c](https://github.com/amigo-labs/amigo-blinzel/commit/386cd8cf8c5956196ccaf2022fa030cbc25c0fd5))
* **ui:** keep browsing when the folder changes under the current image (AC-14) ([3e66501](https://github.com/amigo-labs/amigo-blinzel/commit/3e665016b05e60000ed40a5f06f25a163f1f3de6))
* **ui:** saving a cutout never replaces its source (AC-20..AC-22) ([7721927](https://github.com/amigo-labs/amigo-blinzel/commit/7721927351962eeebc2ebe6356173ee555fd2dcf))
* **ui:** show the window at once when no image is coming (AC-03) ([11b2f58](https://github.com/amigo-labs/amigo-blinzel/commit/11b2f58602efca85cf8d6d2f6b27de32f4fafe0a))
* **windows:** a Print key that comes free on the last retry shows no notice (AC-03) ([bb6e0c8](https://github.com/amigo-labs/amigo-blinzel/commit/bb6e0c8703a66b8297a2ffa13277911fc012f991))
* **windows:** clippy explicit_auto_deref in the clipboard window procedure ([8c2ab01](https://github.com/amigo-labs/amigo-blinzel/commit/8c2ab01c0a3e376f665ade22ed75db3aa2a56e18))
* **windows:** import THREAD_PRIORITY for the scheduling helper (AC-18) ([7b1c78f](https://github.com/amigo-labs/amigo-blinzel/commit/7b1c78faf29268745bb0a638bce5309d75ccaf1e))
* **windows:** the startup report's warp is true on the Basic Render Driver ([5b82970](https://github.com/amigo-labs/amigo-blinzel/commit/5b829705730e82ee012ced0a7a73dbc07e3be69e))
* **windows:** the tray menu opens leftwards while the UI language is right to left (AC-16) ([1e4162a](https://github.com/amigo-labs/amigo-blinzel/commit/1e4162ad50450428b5411c880b087f988716247c))
* **windows:** uninstalling from the downloaded setup shows why it failed ([39fa818](https://github.com/amigo-labs/amigo-blinzel/commit/39fa8184dcec8321b366b0165c0e8d77eba0bf2e))
* **windows:** warp recognises the Basic Render Driver by its vendor and device id ([8707b8b](https://github.com/amigo-labs/amigo-blinzel/commit/8707b8b38536e6f981d6e69ee7289d63032c04a1))

## [1.3.0](https://github.com/amigo-labs/amigo-blinzel/compare/v1.2.1...v1.3.0) (2026-10-29)


### Features

* **i18n:** add es, fr, it, ja, pt-BR, ru and zh-CN ([0dcf877](https://github.com/amigo-labs/amigo-blinzel/commit/0dcf877e1413d065daeae3818163e81b5046e923))
* **view:** grow the window for images larger than it ([c3864df](https://github.com/amigo-labs/amigo-blinzel/commit/c3864df38c3c56676a1e960785f9345107f97d79))
* **website:** one-page site with screenshots and batch tool comparison ([730777e](https://github.com/amigo-labs/amigo-blinzel/commit/730777e7e33e6a59e00f6728f3e562339782beba))
* **website:** one-page site with screenshots and batch tool comparison ([0ab9c76](https://github.com/amigo-labs/amigo-blinzel/commit/0ab9c765bc72a3131468002d42c2f01cc266b1ab))

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
