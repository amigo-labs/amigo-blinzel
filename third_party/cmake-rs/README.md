# cmake-rs 0.1.58, patched

Vendored copy of [cmake-rs](https://github.com/rust-lang/cmake-rs) 0.1.58 (MIT OR Apache-2.0), used by `turbojpeg-sys` to build libjpeg-turbo.

## The patch

With MSVC and the Visual Studio generator, cmake-rs sets `CMAKE_<LANG>_FLAGS_<CONFIG>` to the cc flags so that `/MT` vs. `/MD` reaches the build. This replaces CMake's defaults for that configuration, `/O2 /Ob1 /DNDEBUG` for RelWithDebInfo for example. It also drops every `/O` flag from the cc flags.

MSBuild then compiles with `/Od`. For libjpeg-turbo, decoding gets about 2× slower, and the Huffman decoder 2.6× slower. On ARM64 the NEON code, which is C intrinsics, is unoptimized as well.

The patch puts CMake's default flags for the configuration back in front, for C and C++ only (`msvc_config_flags` in `src/lib.rs`). Other generators (Ninja, NMake, cargo-xwin) are unaffected.

`blinzel-bench run --check` catches a regression: it fails as soon as zune-jpeg beats the libjpeg-turbo path.

## Updating

When updating, copy the new release's `src/lib.rs` over this one and re-apply the block marked `amigo-blinzel patch`. Once upstream fixes the problem, delete this directory and the `[patch.crates-io]` entry.
