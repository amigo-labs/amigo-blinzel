//! Platform-independent core of amigo-blinzel: decoding at the resolution
//! the screen needs, view layout, selection, prefetch scheduling, caches.
//! Everything here is unit-tested on Linux; the Windows app wires it to
//! Win32 and Direct3D.

#[cfg(feature = "batch")]
pub mod batch;
pub mod bufpool;
pub mod cache;
pub mod cpuref;
pub mod decode;
pub mod exif;
pub mod export;
pub mod filelist;
pub mod format;
pub mod geom;
pub mod grid;
pub mod icc;
pub mod json;
pub mod kernels;
pub mod layout;
pub mod lut;
pub mod natsort;
pub mod pixel;
pub mod preview;
pub mod raw;
pub mod sched;
pub mod selection;
pub mod settings;
pub mod testimg;
pub mod thumb;
pub mod thumbcache;
pub mod timing;
pub mod update;
