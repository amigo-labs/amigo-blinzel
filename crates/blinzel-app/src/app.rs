//! UI-thread state: the current image, navigation and prefetch policy, view
//! state, selection, rendering. It never decodes; it shows the best texture
//! that is ready and tells the loader what to prepare next.
//!
//! Anything that can re-enter the window procedure (menus, dialogs, window
//! moves) is returned as an [`Action`] and executed by `window.rs` after the
//! state borrow has been released.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use blinzel_core::cache::ByteLru;
use blinzel_core::decode::Probe;
use blinzel_core::filelist::{Entry, FileList};
use blinzel_core::geom::{Orientation, PixelRect, Rect};
use blinzel_core::layout::{
    self, Layout, ViewMode, ViewState, compute, content_in_stage, stage_to_client, zoom_at,
    zoom_step,
};
use blinzel_core::selection::{self, Drag, Hit, LeftDrag, hit_test, left_drag_action};
use blinzel_core::settings::Settings;
use blinzel_core::timing;
use blinzel_i18n::{Msg, tr, trf};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::SetWindowTextW;
use windows::core::PCWSTR;

use crate::browse::{Browse, strip_height};
use crate::chrome::{Button, Chrome};
use crate::clipboard;
use crate::color::ColorManager;
use crate::gfx::text::Text;
use crate::gfx::{Gfx, GpuImage, GpuLut, Renderer, Rgba, Shape, SwapChain};
use crate::loader::{self, GpuAnimation, Job, Loaded, Loader, Stage};
use crate::menu::{Cmd, MenuState};
use crate::platform::{enumerate, io, wide};
use crate::settings_store;

/// Deferred work that may re-enter the window procedure.
#[derive(Debug)]
pub enum Action {
    ShowMenu {
        x: i32,
        y: i32,
    },
    /// Resize the window so its client area becomes `client` (physical
    /// pixels), centred where it is now.
    FitWindow {
        client: (u32, u32),
    },
    ToggleFullscreen,
    ToggleMaximize,
    Minimize,
    OpenDialog,
    Recycle(PathBuf),
    /// Ask for a file name, then save the cutout in the background.
    SaveCutout(SaveJob),
    CheckUpdates,
    About,
    /// Open the third-party licence texts.
    Notices,
    /// Turn resident mode on or off (tray icon, start at sign-in).
    SetResident(bool),
    /// The tray icon's menu at a screen position.
    TrayMenu {
        x: i32,
        y: i32,
    },
    /// Show the window: a hidden resident one comes back (cloaked until
    /// the first frame of the file just `opened`), a visible one is
    /// activated.
    Wake {
        opened: bool,
    },
    /// The user closed the window: hides it in resident mode, else exits.
    Close,
    /// Exit for good, also in resident mode.
    Exit,
    Activate,
    Uncloak,
    Quit,
}

/// A cutout to save (selection, otherwise the view crop).
#[derive(Debug, Clone)]
pub struct SaveJob {
    pub source: PathBuf,
    pub cut: blinzel_core::export::Cutout,
    pub default_name: String,
    pub prefer_png: bool,
}

pub struct Current {
    pub path: PathBuf,
    pub entry: Entry,
    pub cache_id: u64,
    pub index: Option<usize>,
}

enum DragState {
    Pan {
        last: (f64, f64),
    },
    /// Pressed on the image; becomes a new selection once it moves.
    Pending {
        start: (f64, f64),
        start_img: (f64, f64),
    },
    Select(Drag),
}

/// Messages from helper threads to the UI thread.
pub const WM_LIST: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;
pub const WM_DIR_CHANGED: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 3;
/// Timer that advances an animation (window.rs dispatches it).
pub const TIMER_ANIM: usize = 3;

/// LPARAM: `Box<(isize, Option<Profile>)>`, a monitor and its colour target.
pub const WM_COLOR_TARGET: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 8;

pub struct App {
    pub hwnd: HWND,
    pub gfx: Gfx,
    renderer: Renderer,
    pub swap: SwapChain,
    text: Option<Result<Text, ()>>,
    pub settings: Settings,
    pub loader: Loader,
    list: FileList,
    list_rx: Option<Receiver<(PathBuf, std::io::Result<Vec<Entry>>)>>,
    watcher: Option<enumerate::DirWatcher>,
    pub current: Option<Current>,
    textures: ByteLru<u64, Arc<GpuImage>>,
    vram_budget: usize,
    probes: HashMap<u64, Probe>,
    failed: HashMap<u64, String>,
    /// What is on screen (may be the previous image while the next loads).
    shown: Option<(u64, Arc<GpuImage>)>,
    pub view: ViewState,
    user_rotation: Orientation,
    selection: Option<Rect>,
    drag: Option<DragState>,
    pub hover: Option<Button>,
    pub pressed: Option<Button>,
    pub fullscreen: bool,
    pub maximized: bool,
    pub cloaked: bool,
    /// Resident and closed: hidden, nothing loaded, device kept warm.
    pub hidden: bool,
    /// Take the foreground once the window is uncloaked (after a wake).
    pub activate_on_uncloak: bool,
    /// Benchmark of a resident instance (`--resident --json`): the timing
    /// marks of each wake go here.
    pub wake_report: Option<PathBuf>,
    /// Colour management (shared with the loader) and the GPU copies of
    /// its transforms, by transform.
    pub color: Arc<ColorManager>,
    gpu_luts: HashMap<usize, (Arc<blinzel_core::lut::Lut3d>, Option<GpuLut>)>,
    /// The monitor the colour target was chosen for.
    pub monitor: isize,
    /// The remembered colour target was checked against Windows.
    color_verified: bool,
    /// Grid browser and filmstrip.
    pub browse: Browse,
    /// The current image's animation (`None`: it is a still image).
    animations: HashMap<u64, Option<Arc<GpuAnimation>>>,
    /// Frame shown and complete plays so far.
    anim_frame: usize,
    anim_plays: u32,
    pub dpi: u32,
    pub client: (u32, u32),
    direction: isize,
    pub clip: Option<clipboard::Source>,
    first_image_shown: bool,
    sized_for: Option<u64>,
    pub exit_after_first_frame: Option<PathBuf>,
    /// Short message shown at the bottom (saved, errors), cleared by timer.
    pub notice: Option<String>,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hwnd: HWND,
        gfx: Gfx,
        renderer: Renderer,
        swap: SwapChain,
        settings: Settings,
        loader: Loader,
        color: Arc<ColorManager>,
        dpi: u32,
        client: (u32, u32),
    ) -> App {
        let vram_budget = gfx.texture_budget();
        App {
            hwnd,
            gfx,
            renderer,
            swap,
            text: None,
            settings,
            loader,
            list: FileList::default(),
            list_rx: None,
            watcher: None,
            current: None,
            textures: ByteLru::new(),
            vram_budget,
            probes: HashMap::new(),
            failed: HashMap::new(),
            shown: None,
            view: ViewState::default(),
            user_rotation: Orientation::NORMAL,
            selection: None,
            drag: None,
            hover: None,
            pressed: None,
            fullscreen: false,
            maximized: false,
            cloaked: true,
            hidden: false,
            activate_on_uncloak: false,
            wake_report: None,
            color,
            gpu_luts: HashMap::new(),
            monitor: 0,
            color_verified: false,
            browse: Browse::default(),
            animations: HashMap::new(),
            anim_frame: 0,
            anim_plays: 0,
            dpi,
            client,
            direction: 1,
            clip: None,
            first_image_shown: false,
            sized_for: None,
            exit_after_first_frame: None,
            notice: None,
        }
    }

    pub fn scale(&self) -> f64 {
        self.dpi as f64 / 96.0
    }

    pub fn chrome(&self) -> Chrome {
        Chrome {
            scale: self.scale(),
            client: (self.client.0 as f64, self.client.1 as f64),
        }
    }

    pub fn invalidate(&self) {
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    // ------------------------------------------------------------- resident

    /// Resident mode, window closed: forgets the session (image, folder,
    /// textures) and gives memory back. Device, shaders and threads stay.
    pub fn hide(&mut self) {
        self.loader.set_wanted(Vec::new());
        let evicted = self.textures.clear();
        self.loader.forget(evicted.into_iter().map(|(k, _)| k));
        self.current = None;
        self.shown = None;
        self.probes.clear();
        self.failed.clear();
        self.list = FileList::default();
        self.list_rx = None;
        self.watcher = None;
        self.reset_view();
        self.hover = None;
        self.pressed = None;
        self.sized_for = None;
        self.notice = None;
        self.direction = 1;
        self.hidden = true;
        self.cloaked = true;
        self.activate_on_uncloak = false;
        self.browse.reset();
        self.stop_animation();
        self.drop_animations(None);
        self.gfx.trim();
        // Also what prefetch jobs still in flight give back later.
        blinzel_core::bufpool::global().set_max_retained(0);
    }

    /// A hidden instance waking up: another window may have changed the
    /// settings meanwhile.
    pub fn reload_settings(&mut self) {
        self.settings = settings_store::load();
        self.color.set_enabled(self.settings.color_management);
        let lang = self
            .settings
            .language
            .as_deref()
            .and_then(blinzel_i18n::lang_from_code);
        blinzel_i18n::set_lang(lang.unwrap_or_else(crate::run::system_language));
    }

    /// Resident start: decodes, uploads and draws a generated JPEG once
    /// while hidden, so the first hand-off does not pay first-use costs
    /// (shader compilation in the driver, decoder threads, code paging).
    #[cfg(feature = "turbojpeg")]
    pub fn warm_up(&mut self, threads: usize) {
        use blinzel_core::decode::{self, jpeg};
        // About the size of a photo's screen stage: the driver's first
        // allocation of textures that large is slow too.
        let (w, h) = (3072, 2048);
        let pixels = blinzel_core::testimg::rgb(w, h);
        let Ok(data) = jpeg::encode_rgb(&pixels, w, h, jpeg::EncodeOptions::default()) else {
            return;
        };
        let req = decode::Request::Full;
        let Ok(img) = decode::decode_with(&data, req, threads) else {
            return;
        };
        let tile = self
            .gfx
            .max_texture_size()
            .min(crate::gfx::texture::DEFAULT_TILE);
        let Ok(gpu) =
            crate::gfx::texture::upload(&self.gfx.device, &img, Orientation::NORMAL, tile)
        else {
            return;
        };
        let Some(rtv) = self.swap.rtv().cloned() else {
            return;
        };
        let bg = Rgba::from_u8(self.settings.background, 1.0);
        let ctx = self.gfx.context.clone();
        let full = (gpu.full_width, gpu.full_height);
        let l = compute(
            full,
            (self.client.0 as f64, self.client.1 as f64),
            &self.settings.view,
            &ViewState::default(),
        );
        let stage = (gpu.width, gpu.height);
        let xform = stage_to_client(&l, Orientation::NORMAL, full, stage);
        let content = content_in_stage(&l, Orientation::NORMAL, full, stage);
        self.renderer.begin(&ctx, &rtv, self.swap.size, bg);
        self.renderer
            .draw_image(&ctx, &gpu, &xform, &content, bg, None);
        let _ = self.swap.present();
        // The colour system (mscms) loads on first use: once now, with the
        // profile most photos with one carry (phones: Display P3).
        if self.settings.color_management {
            use blinzel_core::icc::{Profile, Trc, colorants, matrix_profile};
            let p3 = Profile::new(matrix_profile(colorants::DISPLAY_P3, Trc::Srgb, "P3"));
            let _ = crate::color::build_lut(Some(&p3), None);
        }
    }

    // ---------------------------------------------------------------- files

    /// Opens a file: shows it as soon as it is decoded, lists its folder in
    /// the background.
    pub fn open(&mut self, path: PathBuf) -> Vec<Action> {
        if self.hidden {
            self.reload_settings();
        }
        let entry = match io::entry(&path) {
            Ok(e) => e,
            Err(e) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.failed
                    .insert(0, trf(Msg::ErrorOpen, &[("name", &name), ("reason", &e)]));
                self.current = None;
                self.invalidate();
                return vec![];
            }
        };
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        if self.list.dir() != dir || self.list.is_empty() {
            self.list = FileList::new(dir.clone(), Vec::new());
            self.start_listing(dir);
        }
        let cache_id = entry.cache_id();
        let index = self.list.index_of(&entry.name);
        self.current = Some(Current {
            path,
            entry,
            cache_id,
            index,
        });
        self.direction = 1;
        self.reset_view();
        self.after_navigation()
    }

    pub fn start_listing(&mut self, dir: PathBuf) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.list_rx = Some(rx);
        let hwnd = self.hwnd.0 as isize;
        let _ = std::thread::Builder::new()
            .name("blinzel-list".into())
            .spawn(move || {
                let t = std::time::Instant::now();
                let r = enumerate::list_dir(&dir);
                timing::record("list_dir_us", t.elapsed().as_secs_f64() * 1e6);
                let _ = tx.send((dir, r));
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(HWND(hwnd as *mut _)),
                        WM_LIST,
                        windows::Win32::Foundation::WPARAM(0),
                        windows::Win32::Foundation::LPARAM(0),
                    );
                }
            });
    }

    pub fn on_list(&mut self) -> Vec<Action> {
        let Some(rx) = &self.list_rx else {
            return vec![];
        };
        let Ok((dir, result)) = rx.try_recv() else {
            return vec![];
        };
        self.list_rx = None;
        let entries = result.unwrap_or_default();
        self.list = FileList::new(dir.clone(), entries);
        if let Some(cur) = &mut self.current {
            cur.index = self.list.index_of(&cur.entry.name);
        }
        if self.watcher.as_ref().map(|w| w.dir != dir).unwrap_or(true) {
            self.watcher = enumerate::DirWatcher::start(&dir, self.hwnd, WM_DIR_CHANGED).ok();
        }
        self.update_wanted();
        self.invalidate();
        vec![]
    }

    pub fn on_dir_changed(&mut self) {
        let dir = self.list.dir().to_path_buf();
        if !dir.as_os_str().is_empty() && self.list_rx.is_none() {
            self.start_listing(dir);
        }
    }

    fn go_to_index(&mut self, i: usize) -> Vec<Action> {
        let Some(entry) = self.list.get(i).cloned() else {
            return vec![];
        };
        let path = self.list.dir().join(&entry.name);
        let cache_id = entry.cache_id();
        self.current = Some(Current {
            path,
            entry,
            cache_id,
            index: Some(i),
        });
        self.reset_view();
        self.after_navigation()
    }

    fn step(&mut self, steps: isize) -> Vec<Action> {
        let Some(i) = self.current.as_ref().and_then(|c| c.index) else {
            return vec![];
        };
        self.direction = if steps < 0 { -1 } else { 1 };
        match self.list.offset(i, steps) {
            Some(j) if j != i => self.go_to_index(j),
            _ => vec![],
        }
    }

    fn reset_view(&mut self) {
        self.view = ViewState::default();
        self.user_rotation = Orientation::NORMAL;
        self.selection = None;
        self.drag = None;
    }

    fn stop_animation(&mut self) {
        self.anim_frame = 0;
        self.anim_plays = 0;
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd), TIMER_ANIM);
        }
    }

    /// Forgets the animations of every image but `keep` (the loader may
    /// decode them again).
    fn drop_animations(&mut self, keep: Option<u64>) {
        let gone: Vec<u64> = self
            .animations
            .keys()
            .copied()
            .filter(|&k| Some(k) != keep)
            .collect();
        for id in gone {
            self.animations.remove(&id);
            self.loader.forget([loader::key(id, Stage::Anim)]);
        }
    }

    fn schedule_frame(&self, ms: u32) {
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(
                Some(self.hwnd),
                TIMER_ANIM,
                ms.max(10),
                None,
            );
        }
    }

    /// The current image's animation, if it has one.
    fn animation(&self) -> Option<Arc<GpuAnimation>> {
        let c = self.current.as_ref()?;
        self.animations.get(&c.cache_id)?.clone()
    }

    /// The animation timer: next frame; at the end of the last play the
    /// last frame stays.
    pub fn on_anim_timer(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd), TIMER_ANIM);
        }
        let Some(a) = self.animation() else { return };
        let next = self.anim_frame + 1;
        if next == a.frames.len() {
            self.anim_plays += 1;
            if a.plays.is_some_and(|p| self.anim_plays >= p) {
                return;
            }
        }
        self.anim_frame = next % a.frames.len();
        self.schedule_frame(a.frames[self.anim_frame].1);
        self.invalidate();
    }

    fn after_navigation(&mut self) -> Vec<Action> {
        self.stop_animation();
        let keep = self.current.as_ref().map(|c| c.cache_id);
        self.drop_animations(keep);
        self.update_title();
        self.update_wanted();
        self.invalidate();
        self.fit_window_actions()
    }

    fn update_title(&self) {
        let title = match &self.current {
            Some(c) => trf(Msg::WindowTitle, &[("name", &c.entry.name)]),
            None => tr(Msg::AppName).to_string(),
        };
        let w = wide(&title);
        unsafe {
            let _ = SetWindowTextW(self.hwnd, PCWSTR(w.as_ptr()));
        }
    }

    // ------------------------------------------------------------- prefetch

    fn has_texture(&self, cache_id: u64, at_least: Stage) -> bool {
        [Stage::Full, Stage::Screen]
            .into_iter()
            .filter(|s| *s >= at_least)
            .any(|s| self.textures.contains(&loader::key(cache_id, s)))
    }

    /// Resolution the current view needs.
    fn current_stage_needed(&self) -> Stage {
        if self.settings.view.mode == ViewMode::ActualSize {
            return Stage::Full;
        }
        let Some(cur) = &self.current else {
            return Stage::Screen;
        };
        // Zoomed in beyond what the screen-sized stage resolves.
        if let (Some(img), Some(l)) = (self.best_texture(cur.cache_id), self.layout()) {
            let texel_per_px = l.scale * img.full_width as f64 / img.width.max(1) as f64;
            if texel_per_px > 1.05 && !img.is_full_resolution() {
                return Stage::Full;
            }
        }
        Stage::Screen
    }

    /// Tells the loader what to prepare: the current image first, then
    /// neighbours in browsing direction.
    pub fn update_wanted(&mut self) {
        let mut jobs = Vec::new();
        let Some(cur) = &self.current else {
            self.loader.set_wanted(jobs);
            return;
        };
        let stage = self.current_stage_needed();
        if !self.has_texture(cur.cache_id, stage) && !self.failed.contains_key(&cur.cache_id) {
            jobs.push(Job {
                path: cur.path.clone(),
                cache_id: cur.cache_id,
                stage,
                with_preview: !self
                    .textures
                    .contains(&loader::key(cur.cache_id, Stage::Preview)),
                priority: 0,
            });
        }
        // Animated formats; the Anim stage reports still files as such.
        let may_animate = self.probes.get(&cur.cache_id).is_some_and(|p| {
            matches!(
                p.format,
                blinzel_core::format::Format::Gif
                    | blinzel_core::format::Format::WebP
                    | blinzel_core::format::Format::Jxl
            )
        });
        if may_animate && !self.animations.contains_key(&cur.cache_id) {
            jobs.push(Job {
                path: cur.path.clone(),
                cache_id: cur.cache_id,
                stage: Stage::Anim,
                with_preview: false,
                priority: 1,
            });
        }
        let neighbour_stage = if self.settings.view.mode == ViewMode::ActualSize {
            Stage::Full
        } else {
            Stage::Screen
        };
        if let Some(i) = cur.index {
            let ahead = self.settings.prefetch_ahead as isize;
            let behind = self.settings.prefetch_behind as isize;
            let mut seen = vec![i];
            let mut add = |steps: isize, priority: u32, preview: bool, jobs: &mut Vec<Job>| {
                let Some(j) = self.list.offset(i, steps) else {
                    return;
                };
                if seen.contains(&j) {
                    return;
                }
                seen.push(j);
                let Some(e) = self.list.get(j) else { return };
                let id = e.cache_id();
                if e.is_placeholder()
                    || self.failed.contains_key(&id)
                    || self.has_texture(id, neighbour_stage)
                {
                    return;
                }
                jobs.push(Job {
                    path: self.list.dir().join(&e.name),
                    cache_id: id,
                    stage: neighbour_stage,
                    with_preview: preview,
                    priority,
                });
            };
            let d = self.direction;
            for k in 1..=ahead.max(behind) {
                if k <= ahead {
                    add(d * k, (2 * k - 1) as u32, k == 1, &mut jobs);
                }
                if k <= behind {
                    add(-d * k, (3 * k) as u32, k == 1, &mut jobs);
                }
            }
        }
        self.loader.set_wanted(jobs);
    }

    /// Drains finished loads into the texture cache.
    pub fn on_loaded(&mut self) -> Vec<Action> {
        if self.hidden {
            // Late results of the closed session: not kept.
            while let Ok(l) = self.loader.results.try_recv() {
                self.loader.forget([loader::key(l.cache_id, l.stage)]);
            }
            return vec![];
        }
        let mut changed_current = false;
        let current_id = self.current.as_ref().map(|c| c.cache_id);
        while let Ok(l) = self.loader.results.try_recv() {
            let Loaded {
                cache_id,
                stage,
                probe,
                image,
                animation,
                read_ms,
                decode_ms,
                upload_ms,
            } = l;
            if let Some(p) = probe {
                self.probes.insert(cache_id, p);
            }
            if stage == Stage::Anim {
                if Some(cache_id) == current_id {
                    if let Some(a) = &animation {
                        self.anim_frame = 0;
                        self.anim_plays = 0;
                        self.schedule_frame(a.frames[0].1);
                    }
                    self.animations.insert(cache_id, animation);
                    changed_current = true;
                }
                continue;
            }
            match image {
                Ok(img) => {
                    let bytes = img.bytes;
                    self.textures
                        .insert(loader::key(cache_id, stage), img, bytes);
                    // The image the user waits for (startup or wake).
                    if Some(cache_id) == current_id && (self.cloaked || !self.first_image_shown) {
                        timing::record("first_read_us", read_ms * 1000.0);
                        timing::record("first_decode_us", decode_ms * 1000.0);
                        timing::record("first_upload_us", upload_ms * 1000.0);
                        timing::mark(format!("first_{stage:?}_ready").to_lowercase());
                    }
                }
                Err(e) => {
                    // A failed preview is not a failed image.
                    if stage != Stage::Preview {
                        self.failed.insert(cache_id, e);
                    }
                }
            }
            if Some(cache_id) == current_id {
                changed_current = true;
            }
        }
        // Keep VRAM within budget; the current image and the wanted window
        // stay.
        let keep: Vec<u64> = self.wanted_ids();
        let evicted = self
            .textures
            .evict_to(self.vram_budget, |k| keep.contains(&(k & !3)));
        if !evicted.is_empty() {
            self.loader.forget(evicted.iter().map(|(k, _)| *k));
        }
        let mut actions = Vec::new();
        if changed_current {
            self.update_wanted();
            actions.extend(self.fit_window_actions());
            self.invalidate();
        }
        actions
    }

    /// Cache ids of the current image and its prefetch neighbourhood.
    fn wanted_ids(&self) -> Vec<u64> {
        let mut ids = Vec::new();
        if let Some(c) = &self.current {
            ids.push(c.cache_id & !3);
            if let Some(i) = c.index {
                let r = self
                    .settings
                    .prefetch_ahead
                    .max(self.settings.prefetch_behind) as isize;
                for k in -r..=r {
                    if let Some(e) = self.list.offset(i, k).and_then(|j| self.list.get(j)) {
                        ids.push(e.cache_id() & !3);
                    }
                }
            }
        }
        ids
    }

    fn best_texture(&self, cache_id: u64) -> Option<Arc<GpuImage>> {
        [Stage::Full, Stage::Screen, Stage::Preview]
            .into_iter()
            .find_map(|s| self.textures.peek(&loader::key(cache_id, s)).cloned())
    }

    // --------------------------------------------------------------- layout

    fn orientation_of(&self, img: &GpuImage) -> Orientation {
        img.orientation.then(self.user_rotation)
    }

    /// Image to display and its layout.
    fn displayed(&self) -> Option<(Arc<GpuImage>, Orientation, Layout)> {
        let frame = self
            .animation()
            .and_then(|a| a.frames.get(self.anim_frame).map(|f| f.0.clone()));
        let img = match &self.current {
            Some(_) if frame.is_some() => frame,
            // A broken file shows its error, not the previous image.
            Some(c) if self.failed.contains_key(&c.cache_id) => self.best_texture(c.cache_id),
            Some(c) => self
                .best_texture(c.cache_id)
                .or_else(|| self.shown.as_ref().map(|s| s.1.clone())),
            None => None,
        }?;
        let o = self.orientation_of(&img);
        let oriented = o.oriented_size(img.full_width, img.full_height);
        let l = compute(oriented, self.view_area(), &self.settings.view, &self.view);
        Some((img, o, l))
    }

    fn filmstrip_shown(&self) -> bool {
        self.settings.filmstrip && self.browse.grid.is_none()
    }

    fn client_f(&self) -> (f64, f64) {
        (self.client.0 as f64, self.client.1 as f64)
    }

    /// Where the image is laid out: the client area above the filmstrip.
    fn view_area(&self) -> (f64, f64) {
        let strip = if self.filmstrip_shown() {
            strip_height(self.scale())
        } else {
            0.0
        };
        (
            self.client.0 as f64,
            (self.client.1 as f64 - strip).max(1.0),
        )
    }

    /// Cache id of the image [`Self::displayed`] shows.
    fn displayed_id(&self) -> Option<u64> {
        let c = self.current.as_ref()?;
        if self.failed.contains_key(&c.cache_id) || self.best_texture(c.cache_id).is_some() {
            Some(c.cache_id)
        } else {
            self.shown.as_ref().map(|s| s.0)
        }
    }

    /// The colour transform for image `cache_id` on the GPU, if it needs
    /// one.
    fn gpu_lut(&mut self, cache_id: u64) -> Option<GpuLut> {
        let icc = self.probes.get(&cache_id).and_then(|p| p.icc.clone());
        let lut = self.color.lut(icc.as_ref())?;
        let key = Arc::as_ptr(&lut) as usize;
        let device = &self.gfx.device;
        self.gpu_luts
            .entry(key)
            .or_insert_with(|| {
                let gpu = crate::gfx::upload_lut(device, &lut).ok();
                (lut, gpu)
            })
            .1
            .clone()
    }

    /// Asks Windows for the monitor's colour target in the background (the
    /// start used the remembered one); the answer comes back as
    /// [`WM_COLOR_TARGET`].
    fn verify_color_target(&mut self) {
        if self.color_verified || !self.settings.color_management {
            return;
        }
        self.color_verified = true;
        let (hwnd, monitor) = (self.hwnd.0 as isize, self.monitor);
        let _ = std::thread::Builder::new()
            .name("blinzel-color-target".into())
            .spawn(move || {
                use windows::Win32::Foundation::{LPARAM, WPARAM};
                use windows::Win32::Graphics::Gdi::HMONITOR;
                use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
                let target = crate::color::display_target(HMONITOR(monitor as *mut _));
                let msg = Box::into_raw(Box::new((monitor, target)));
                unsafe {
                    if PostMessageW(
                        Some(HWND(hwnd as *mut _)),
                        WM_COLOR_TARGET,
                        WPARAM(0),
                        LPARAM(msg as isize),
                    )
                    .is_err()
                    {
                        drop(Box::from_raw(msg));
                    }
                }
            });
    }

    /// The colour target Windows reported for `monitor`.
    pub fn on_color_target(&mut self, monitor: isize, target: Option<blinzel_core::icc::Profile>) {
        if monitor == self.monitor && self.color.set_target(target) {
            self.gpu_luts.clear();
            self.invalidate();
        }
    }

    /// Re-targets colour management when the window moved to another
    /// monitor (or the displays changed: `force`).
    pub fn check_monitor(&mut self, force: bool) {
        use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
        let m = unsafe { MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST) };
        if !force && m.0 as isize == self.monitor {
            return;
        }
        self.monitor = m.0 as isize;
        if self.color.set_target(crate::color::display_target(m)) {
            self.gpu_luts.clear();
            self.invalidate();
        }
    }

    fn layout(&self) -> Option<Layout> {
        self.displayed().map(|d| d.2)
    }

    /// Oriented full-resolution size of the current image, once known.
    fn current_oriented_size(&self) -> Option<(u32, u32)> {
        let c = self.current.as_ref()?;
        let p = self.probes.get(&c.cache_id)?;
        let o = p.orientation.then(self.user_rotation);
        Some(o.oriented_size(p.width, p.height))
    }

    /// In "fit window to image" mode the window follows each image.
    fn fit_window_actions(&mut self) -> Vec<Action> {
        if self.settings.view.mode != ViewMode::FitWindowToImage
            || self.fullscreen
            || self.maximized
        {
            return vec![];
        }
        let Some(cur) = &self.current else {
            return vec![];
        };
        let Some(size) = self.current_oriented_size() else {
            return vec![];
        };
        let tag = cur.cache_id ^ (self.user_rotation.quarter_turns as u64);
        if self.sized_for == Some(tag) {
            return vec![];
        }
        self.sized_for = Some(tag);
        let size = match self.view.crop {
            Some(c) => (c.w, c.h),
            None => size,
        };
        let strip = if self.filmstrip_shown() {
            strip_height(self.scale()) as u32
        } else {
            0
        };
        vec![Action::FitWindow {
            client: (size.0, size.1 + strip),
        }]
    }

    // ------------------------------------------------------------ rendering

    pub fn render(&mut self) -> Vec<Action> {
        let Some(rtv) = self.swap.rtv().cloned() else {
            return vec![];
        };
        let bg = Rgba::from_u8(self.settings.background, 1.0);
        let ctx = self.gfx.context.clone();
        self.renderer.begin(&ctx, &rtv, self.swap.size, bg);
        let mut shapes: Vec<Shape> = Vec::new();
        let mut showed_image = false;
        let in_grid = self.browse.grid.is_some();
        if in_grid {
            let (client, scale) = (self.client_f(), self.scale());
            let background = self.settings.background;
            self.browse.draw_grid(
                &self.renderer,
                &ctx,
                self.hwnd,
                &self.list,
                client,
                scale,
                background,
            );
        } else if let Some((img, o, l)) = self.displayed() {
            let full = (img.full_width, img.full_height);
            let stage = (img.width, img.height);
            let xform = stage_to_client(&l, o, full, stage);
            let content = content_in_stage(&l, o, full, stage);
            let lut = self.displayed_id().and_then(|id| self.gpu_lut(id));
            self.renderer
                .draw_image(&ctx, &img, &xform, &content, bg, lut.as_ref());
            if let Some(cur) = &self.current {
                self.shown = Some((cur.cache_id, img.clone()));
                showed_image = self.best_texture(cur.cache_id).is_some();
            }
            if let Some(sel) = self.selection {
                self.selection_shapes(&l, &sel, &mut shapes);
            }
        }
        self.renderer.draw_shapes(&ctx, &shapes);
        if let Some(sel) = self.selection
            && !in_grid
            && let Some(l) = self.layout()
        {
            self.draw_selection_label(&l, &sel);
        }
        if !showed_image && !in_grid {
            self.draw_message();
        }
        if self.filmstrip_shown() {
            let current = self.current.as_ref().and_then(|c| c.index);
            let (client, scale) = (self.client_f(), self.scale());
            self.browse.draw_strip(
                &self.renderer,
                &ctx,
                self.hwnd,
                &self.list,
                current,
                client,
                scale,
                self.settings.background,
            );
        }
        self.draw_notice();
        if !self.fullscreen {
            let chrome = self
                .chrome()
                .shapes(self.hover, self.pressed, self.maximized);
            self.renderer.draw_shapes(&ctx, &chrome);
        }
        let _ = self.swap.present();

        // After the first frame: checking the remembered colour target
        // loads the ICM subsystem.
        self.verify_color_target();
        let mut actions = Vec::new();
        if showed_image && self.cloaked {
            actions.push(Action::Uncloak);
        }
        if showed_image && !self.first_image_shown {
            self.first_image_shown = true;
            timing::mark("first_present");
            crate::update::on_first_frame();
            if let Some(out) = &self.exit_after_first_frame {
                // The adapter tells a real GPU from WARP / Basic Render
                // Driver, which costs hundreds of ms per frame.
                let json = format!(
                    "{{\"adapter\": {}, \"warp\": {}, \"marks\": {}}}",
                    timing::json_str(&self.gfx.adapter_name()),
                    self.gfx.warp,
                    timing::marks_json()
                );
                let _ = std::fs::write(out, json);
                actions.push(Action::Quit);
            }
            crate::gfx::text::prewarm();
        }
        actions
    }

    fn selection_shapes(&self, l: &Layout, sel: &Rect, out: &mut Vec<Shape>) {
        let s = self.scale();
        let r = l.image_to_client().map_rect(sel);
        let c = l.content_rect();
        let dim = Rgba(0.0, 0.0, 0.0, 0.45);
        // Darken everything outside the selection (within the image).
        let bands = [
            Rect::new(c.x, c.y, c.w, (r.y - c.y).max(0.0)),
            Rect::new(c.x, r.bottom(), c.w, (c.bottom() - r.bottom()).max(0.0)),
            Rect::new(c.x, r.y, (r.x - c.x).max(0.0), r.h),
            Rect::new(r.right(), r.y, (c.right() - r.right()).max(0.0), r.h),
        ];
        for b in bands {
            if b.w > 0.0 && b.h > 0.0 {
                out.push(Shape::Fill {
                    rect: b,
                    radius: 0.0,
                    color: dim,
                });
            }
        }
        out.push(Shape::Outline {
            rect: r,
            thickness: 3.0,
            radius: 0.0,
            color: Rgba(0.0, 0.0, 0.0, 0.6),
        });
        out.push(Shape::Outline {
            rect: r,
            thickness: 1.0,
            radius: 0.0,
            color: Rgba(1.0, 1.0, 1.0, 0.95),
        });
        let hs = 7.0 * s;
        if r.w >= hs * 2.0 && r.h >= hs * 2.0 {
            for (_, h) in selection::handle_rects(&r, hs) {
                out.push(Shape::Fill {
                    rect: h,
                    radius: 1.0,
                    color: Rgba(1.0, 1.0, 1.0, 1.0),
                });
                out.push(Shape::Outline {
                    rect: h,
                    thickness: 1.0,
                    radius: 1.0,
                    color: Rgba(0.0, 0.0, 0.0, 0.7),
                });
            }
        }
    }

    fn text(&mut self) -> Option<&mut Text> {
        if self.text.is_none() {
            self.text = Some(Text::new(&self.gfx.device).map_err(|_| ()));
        }
        self.text.as_mut().and_then(|t| t.as_mut().ok())
    }

    fn draw_selection_label(&mut self, l: &Layout, sel: &Rect) {
        let s = self.scale();
        let label = format!("{} × {}", sel.w.round() as u32, sel.h.round() as u32);
        let r = l.image_to_client().map_rect(sel);
        let ctx = self.gfx.context.clone();
        let Some(text) = self.text() else { return };
        let Ok(t) = text.get(&label, (12.0 * s) as f32, 1000.0) else {
            return;
        };
        let (srv, size) = (t.srv.clone(), t.size);
        let pad = 4.0 * s;
        let x = r.x.round();
        let mut y = (r.y - size.1 as f64 - 2.0 * pad - 4.0 * s).round();
        if y < 0.0 {
            y = (r.y + 4.0 * s).round();
        }
        let bg = Rect::new(x, y, size.0 as f64 + 2.0 * pad, size.1 as f64 + 2.0 * pad);
        self.renderer.draw_shapes(
            &ctx,
            &[Shape::Fill {
                rect: bg,
                radius: (4.0 * s) as f32,
                color: Rgba(0.0, 0.0, 0.0, 0.72),
            }],
        );
        self.renderer.draw_text(
            &ctx,
            &srv,
            ((x + pad) as f32, (y + pad) as f32),
            size,
            Rgba(1.0, 1.0, 1.0, 1.0),
        );
    }

    fn draw_message(&mut self) {
        let message = match &self.current {
            Some(c) => match self.failed.get(&c.cache_id) {
                Some(e) => {
                    let reason = if e.contains("not decoded natively") || e.contains("WIC") {
                        tr(Msg::ErrorUnsupported).to_string()
                    } else {
                        e.clone()
                    };
                    Some(trf(
                        Msg::ErrorOpen,
                        &[("name", &c.entry.name), ("reason", &reason)],
                    ))
                }
                None => None, // still loading: keep the screen calm
            },
            None => Some(
                self.failed
                    .get(&0)
                    .cloned()
                    .unwrap_or_else(|| tr(Msg::HintOpen).to_string()),
            ),
        };
        let Some(message) = message else { return };
        let s = self.scale();
        let (cw, ch) = (self.client.0 as f64, self.client.1 as f64);
        let ctx = self.gfx.context.clone();
        let Some(text) = self.text() else { return };
        let Ok(t) = text.get(&message, (15.0 * s) as f32, (cw * 0.8).max(100.0) as f32) else {
            return;
        };
        let (srv, size) = (t.srv.clone(), t.size);
        let pos = (
            ((cw - size.0 as f64) / 2.0).round() as f32,
            ((ch - size.1 as f64) / 2.0).round() as f32,
        );
        self.renderer
            .draw_text(&ctx, &srv, pos, size, Rgba(0.85, 0.85, 0.85, 1.0));
    }

    // ---------------------------------------------------------------- input

    fn image_point(&self, l: &Layout, p: (f64, f64)) -> (f64, f64) {
        l.client_to_image(p)
    }

    fn content_bounds(l: &Layout) -> Rect {
        Rect::new(l.crop_origin.0, l.crop_origin.1, l.content.0, l.content.1)
    }

    pub fn mouse_down(&mut self, p: (f64, f64), shift: bool, middle: bool) -> Vec<Action> {
        if !middle && !self.fullscreen && self.chrome().hit(p) == Some(Button::Menu) {
            self.pressed = None;
            let r = self.chrome().rect(Button::Menu);
            let mut pt = windows::Win32::Foundation::POINT {
                x: r.x as i32,
                y: r.bottom() as i32,
            };
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut pt);
            }
            return vec![Action::ShowMenu { x: pt.x, y: pt.y }];
        }
        if let Some(handled) = self.browse_click(p) {
            return handled;
        }
        let Some(l) = self.layout() else {
            return vec![];
        };
        if middle {
            self.drag = Some(DragState::Pan { last: p });
            return vec![];
        }
        let pi = self.image_point(&l, p);
        if let Some(sel) = self.selection {
            let sel_client = l.image_to_client().map_rect(&sel);
            let hit = hit_test(&sel_client, p, 7.0 * self.scale());
            if hit != Hit::Outside {
                self.drag = Some(DragState::Select(Drag::begin(hit, pi, Some(sel))));
                return vec![];
            }
        }
        self.drag = Some(match left_drag_action(&l, shift) {
            LeftDrag::Pan => DragState::Pan { last: p },
            LeftDrag::Select => DragState::Pending {
                start: p,
                start_img: pi,
            },
        });
        vec![]
    }

    /// Clicks on the grid (selects) and the filmstrip (opens). `None`
    /// when `p` is on neither.
    fn browse_click(&mut self, p: (f64, f64)) -> Option<Vec<Action>> {
        let (client, scale, count) = (self.client_f(), self.scale(), self.list.len());
        if let Some(state) = &mut self.browse.grid {
            if let Some(i) = Browse::grid_layout(count, client, scale).hit(p, state.scroll) {
                state.selected = i;
                self.invalidate();
            }
            return Some(vec![]);
        }
        if self.filmstrip_shown() && p.1 >= client.1 - strip_height(scale) {
            let current = self.current.as_ref().and_then(|c| c.index)?;
            return Some(
                match self.browse.hit_strip(p, count, current, client, scale) {
                    Some(i) if i != current => self.go_to_index(i),
                    _ => vec![],
                },
            );
        }
        None
    }

    /// Double click: opens a grid item; on the filmstrip it does nothing.
    /// `None` leaves it to the window (full screen).
    pub fn double_click(&mut self, p: (f64, f64)) -> Option<Vec<Action>> {
        let (client, scale, count) = (self.client_f(), self.scale(), self.list.len());
        if self.browse.grid.is_some() {
            return Some(match self.browse.hit_grid(p, count, client, scale) {
                Some(i) => self.open_from_grid(i),
                None => vec![],
            });
        }
        (self.filmstrip_shown() && p.1 >= client.1 - strip_height(scale)).then(Vec::new)
    }

    fn open_from_grid(&mut self, i: usize) -> Vec<Action> {
        self.browse.grid = None;
        self.browse.hover = None;
        let mut actions = self.go_to_index(i);
        actions.extend(self.fit_window_actions());
        self.invalidate();
        actions
    }

    /// Thumbnails arrived (WM_THUMB).
    pub fn on_thumbs(&mut self) {
        if self.browse.on_thumbs(&self.gfx.device, &self.gfx.context) {
            self.invalidate();
        }
    }

    fn update_browse_hover(&mut self, p: (f64, f64)) {
        let (client, scale, count) = (self.client_f(), self.scale(), self.list.len());
        let hover = if self.browse.grid.is_some() {
            self.browse.hit_grid(p, count, client, scale)
        } else if self.filmstrip_shown() {
            self.current
                .as_ref()
                .and_then(|c| c.index)
                .and_then(|cur| self.browse.hit_strip(p, count, cur, client, scale))
        } else {
            None
        };
        if hover != self.browse.hover {
            self.browse.hover = hover;
            self.invalidate();
        }
    }

    pub fn mouse_move(&mut self, p: (f64, f64)) {
        self.update_browse_hover(p);
        let hover = if self.fullscreen {
            None
        } else {
            self.chrome().hit(p).filter(|b| *b == Button::Menu)
        };
        if hover != self.hover && (hover.is_some() || self.hover == Some(Button::Menu)) {
            self.hover = hover;
            self.invalidate();
        }
        let Some(l) = self.layout() else { return };
        match self.drag.take() {
            Some(DragState::Pan { last }) => {
                layout::pan(&mut self.view, &l, (p.0 - last.0, p.1 - last.1));
                self.drag = Some(DragState::Pan { last: p });
                self.invalidate();
            }
            Some(DragState::Pending { start, start_img }) => {
                let moved = (p.0 - start.0).abs().max((p.1 - start.1).abs());
                if moved > 3.0 * self.scale() {
                    let d = Drag::New { anchor: start_img };
                    self.selection = d.update(self.image_point(&l, p), &Self::content_bounds(&l));
                    self.drag = Some(DragState::Select(d));
                    self.invalidate();
                } else {
                    self.drag = Some(DragState::Pending { start, start_img });
                }
            }
            Some(DragState::Select(d)) => {
                if let Some(r) = d.update(self.image_point(&l, p), &Self::content_bounds(&l)) {
                    self.selection = Some(r);
                }
                self.drag = Some(DragState::Select(d));
                self.invalidate();
            }
            None => {}
        }
    }

    pub fn mouse_up(&mut self) {
        if let Some(DragState::Pending { .. }) = self.drag {
            // A click without dragging clears the selection.
            if self.selection.take().is_some() {
                self.invalidate();
            }
        }
        self.drag = None;
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Cursor shape for a client point.
    pub fn cursor_at(&self, p: (f64, f64), shift: bool) -> CursorKind {
        if self.browse.grid.is_some()
            || (!self.fullscreen && self.chrome().hit(p).is_some())
            || (self.filmstrip_shown() && p.1 >= self.view_area().1)
        {
            return CursorKind::Arrow;
        }
        if let Some(DragState::Pan { .. }) = self.drag {
            return CursorKind::Move;
        }
        let Some(l) = self.layout() else {
            return CursorKind::Arrow;
        };
        if let Some(sel) = self.selection {
            let r = l.image_to_client().map_rect(&sel);
            match hit_test(&r, p, 7.0 * self.scale()) {
                Hit::Handle(selection::Handle::N | selection::Handle::S) => {
                    return CursorKind::SizeNS;
                }
                Hit::Handle(selection::Handle::E | selection::Handle::W) => {
                    return CursorKind::SizeWE;
                }
                Hit::Handle(selection::Handle::NW | selection::Handle::SE) => {
                    return CursorKind::SizeNWSE;
                }
                Hit::Handle(_) => return CursorKind::SizeNESW,
                Hit::Inside => return CursorKind::Move,
                Hit::Outside => {}
            }
        }
        if !l.content_rect().contains(p) {
            return CursorKind::Arrow;
        }
        match left_drag_action(&l, shift) {
            LeftDrag::Pan => CursorKind::Hand,
            LeftDrag::Select => CursorKind::Cross,
        }
    }

    pub fn wheel(&mut self, p: (f64, f64), delta: i16) -> Vec<Action> {
        let (client, scale, count) = (self.client_f(), self.scale(), self.list.len());
        if let Some(state) = &mut self.browse.grid {
            let g = Browse::grid_layout(count, client, scale);
            let rows = -(delta as f64) / 120.0;
            state.scroll =
                (state.scroll + rows * (g.cell + g.gap)).clamp(0.0, g.max_scroll(client.1));
            self.invalidate();
            return vec![];
        }
        if self.filmstrip_shown() && p.1 >= client.1 - strip_height(scale) {
            return self.step(if delta > 0 { -1 } else { 1 });
        }
        self.zoom_wheel(p, delta);
        vec![]
    }

    fn zoom_wheel(&mut self, p: (f64, f64), delta: i16) {
        let Some(l) = self.layout() else { return };
        let factor = 1.2f64.powf(delta as f64 / 120.0);
        zoom_at(&mut self.view, &l, p, l.scale * factor);
        self.after_zoom();
    }

    fn after_zoom(&mut self) {
        self.update_wanted();
        self.invalidate();
    }

    /// Keyboard. Returns None for keys we do not handle.
    pub fn key(&mut self, vk: u16, ctrl: bool, shift: bool) -> Option<Vec<Action>> {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        if let Some(a) = self.grid_key(vk) {
            return Some(a);
        }
        match VIRTUAL_KEY(vk) {
            VK_G if !ctrl => return Some(self.command(Cmd::ToggleGrid)),
            VK_T if !ctrl => return Some(self.command(Cmd::ToggleFilmstrip)),
            _ => {}
        }
        let overflow = self.layout().map(|l| l.overflows).unwrap_or(false);
        let pan = |app: &mut App, dx: f64, dy: f64| {
            if let Some(l) = app.layout() {
                let step = 0.1 * app.client.0.min(app.client.1) as f64;
                layout::pan(&mut app.view, &l, (-dx * step, -dy * step));
                app.invalidate();
            }
            vec![]
        };
        let a = match VIRTUAL_KEY(vk) {
            VK_RIGHT if overflow && !ctrl => pan(self, 1.0, 0.0),
            VK_LEFT if overflow && !ctrl => pan(self, -1.0, 0.0),
            VK_UP if overflow => pan(self, 0.0, -1.0),
            VK_DOWN if overflow => pan(self, 0.0, 1.0),
            VK_RIGHT | VK_NEXT | VK_SPACE | VK_DOWN => self.command(Cmd::Next),
            VK_LEFT | VK_PRIOR | VK_BACK | VK_UP => self.command(Cmd::Prev),
            VK_HOME => self.command(Cmd::First),
            VK_END => self.command(Cmd::Last),
            VK_ADD | VK_OEM_PLUS => self.command(Cmd::ZoomIn),
            VK_SUBTRACT | VK_OEM_MINUS => self.command(Cmd::ZoomOut),
            VK_NUMPAD0 | VIRTUAL_KEY(0x30) => self.command(Cmd::ZoomReset),
            VIRTUAL_KEY(0x31) | VK_NUMPAD1 if !ctrl => {
                self.command(Cmd::View(ViewMode::FitWindowToImage))
            }
            VIRTUAL_KEY(0x32) | VK_NUMPAD2 if !ctrl => {
                self.command(Cmd::View(ViewMode::FitImageToWindow))
            }
            VIRTUAL_KEY(0x33) | VK_NUMPAD3 if !ctrl => {
                self.command(Cmd::View(ViewMode::ActualSize))
            }
            VK_R if !ctrl => self.command(if shift {
                Cmd::RotateLeft
            } else {
                Cmd::RotateRight
            }),
            VK_L if !ctrl => self.command(Cmd::RotateLeft),
            VK_F11 | VK_RETURN => self.command(Cmd::Fullscreen),
            VK_C if ctrl => self.command(Cmd::Copy),
            VK_X if ctrl => self.command(Cmd::Cut),
            VK_A if ctrl => self.command(Cmd::SelectAll),
            VK_O if ctrl => self.command(Cmd::Open),
            VK_S if ctrl => self.command(Cmd::SaveCutout),
            VK_W if ctrl => vec![Action::Close],
            VK_DELETE => self.command(Cmd::Recycle),
            VK_ESCAPE => self.escape(),
            VK_F10 | VK_APPS => {
                let r = self.chrome().rect(Button::Menu);
                let mut pt = windows::Win32::Foundation::POINT {
                    x: r.x as i32,
                    y: r.bottom() as i32,
                };
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut pt);
                }
                vec![Action::ShowMenu { x: pt.x, y: pt.y }]
            }
            _ => return None,
        };
        Some(a)
    }

    /// Keys in the grid: moving the selection, opening, leaving.
    fn grid_key(&mut self, vk: u16) -> Option<Vec<Action>> {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        let (client, scale, count) = (self.client_f(), self.scale(), self.list.len());
        let state = self.browse.grid.as_mut()?;
        let g = Browse::grid_layout(count, client, scale);
        let page = g.page_rows(client.1) as isize;
        let i = state.selected;
        let to = match VIRTUAL_KEY(vk) {
            VK_LEFT => g.step(i, -1, 0),
            VK_RIGHT => g.step(i, 1, 0),
            VK_UP => g.step(i, 0, -1),
            VK_DOWN => g.step(i, 0, 1),
            VK_PRIOR => g.step(i, 0, -page),
            VK_NEXT => g.step(i, 0, page),
            VK_HOME => 0,
            VK_END => count.saturating_sub(1),
            VK_RETURN | VK_SPACE => return Some(self.open_from_grid(i)),
            VK_ESCAPE | VK_G => {
                return Some(self.command(Cmd::ToggleGrid));
            }
            _ => return None,
        };
        state.selected = to;
        state.scroll = g.reveal(to, state.scroll, client.1);
        self.invalidate();
        Some(vec![])
    }

    fn escape(&mut self) -> Vec<Action> {
        if self.drag.take().is_some() {
            self.invalidate();
            return vec![];
        }
        if self.selection.take().is_some() {
            self.invalidate();
            return vec![];
        }
        if self.view.crop.is_some() {
            return self.command(Cmd::ShowOriginal);
        }
        if self.fullscreen {
            return vec![Action::ToggleFullscreen];
        }
        vec![Action::Close]
    }

    // ------------------------------------------------------------- commands

    pub fn menu_state(&self) -> MenuState {
        MenuState {
            mode: self.settings.view.mode,
            no_upscale: self.settings.view.no_upscale,
            fullscreen: self.fullscreen,
            has_image: self.current.is_some(),
            has_selection: self.selection.is_some(),
            is_cropped: self.view.crop.is_some(),
            has_neighbours: self.list.len() > 1,
            language: self
                .settings
                .language
                .as_deref()
                .and_then(blinzel_i18n::lang_from_code),
            resident: self.settings.resident,
            auto_update: self.settings.auto_update,
            single_instance: self.settings.single_instance,
            color_management: self.settings.color_management,
            filmstrip: self.settings.filmstrip,
            grid: self.browse.grid.is_some(),
        }
    }

    pub fn command(&mut self, cmd: Cmd) -> Vec<Action> {
        match cmd {
            Cmd::Open => vec![Action::OpenDialog],
            Cmd::View(mode) => {
                self.settings.view.mode = mode;
                settings_store::save(&self.settings);
                self.view.zoom = None;
                self.view.center = None;
                self.sized_for = None;
                self.after_zoom();
                self.fit_window_actions()
            }
            Cmd::ToggleNoUpscale => {
                self.settings.view.no_upscale = !self.settings.view.no_upscale;
                settings_store::save(&self.settings);
                self.invalidate();
                vec![]
            }
            Cmd::ZoomIn | Cmd::ZoomOut => {
                if let Some(l) = self.layout() {
                    let dir = if cmd == Cmd::ZoomIn { 1 } else { -1 };
                    let center = (self.client.0 as f64 / 2.0, self.client.1 as f64 / 2.0);
                    zoom_at(&mut self.view, &l, center, zoom_step(l.scale, dir));
                    self.after_zoom();
                }
                vec![]
            }
            Cmd::ZoomReset => {
                self.view.zoom = None;
                self.view.center = None;
                self.after_zoom();
                vec![]
            }
            Cmd::RotateRight | Cmd::RotateLeft => {
                self.user_rotation = if cmd == Cmd::RotateRight {
                    self.user_rotation.rotated_cw()
                } else {
                    self.user_rotation.rotated_ccw()
                };
                self.selection = None;
                self.view = ViewState::default();
                self.invalidate();
                self.fit_window_actions()
            }
            Cmd::Fullscreen => vec![Action::ToggleFullscreen],
            Cmd::Copy => {
                self.copy();
                vec![]
            }
            Cmd::Cut => {
                if let Some(sel) = self.selection
                    && let Some(r) = self
                        .current_oriented_size()
                        .and_then(|(w, h)| sel.to_pixels(w, h))
                {
                    self.copy();
                    self.view = ViewState {
                        crop: Some(r),
                        ..Default::default()
                    };
                    self.selection = None;
                    self.sized_for = None;
                    self.invalidate();
                    return self.fit_window_actions();
                }
                vec![]
            }
            Cmd::SelectAll => {
                if let Some(l) = self.layout() {
                    self.selection = Some(Self::content_bounds(&l));
                    self.invalidate();
                }
                vec![]
            }
            Cmd::ShowOriginal => {
                self.view = ViewState::default();
                self.sized_for = None;
                self.invalidate();
                self.fit_window_actions()
            }
            Cmd::Next => self.step(1),
            Cmd::Prev => self.step(-1),
            Cmd::First => {
                if self.list.is_empty() {
                    return vec![];
                }
                self.direction = 1;
                self.go_to_index(0)
            }
            Cmd::Last => {
                if self.list.is_empty() {
                    return vec![];
                }
                self.direction = -1;
                self.go_to_index(self.list.len() - 1)
            }
            Cmd::Recycle => match &self.current {
                Some(c) => vec![Action::Recycle(c.path.clone())],
                None => vec![],
            },
            Cmd::Language(lang) => {
                self.settings.language = lang.map(|l| l.code().to_string());
                blinzel_i18n::set_lang(lang.unwrap_or_else(crate::run::system_language));
                settings_store::save(&self.settings);
                self.update_title();
                self.invalidate();
                vec![]
            }
            Cmd::Exit => vec![Action::Close],
            Cmd::CheckUpdates => vec![Action::CheckUpdates],
            Cmd::ShowWindow => vec![Action::Wake { opened: false }],
            Cmd::Quit => vec![Action::Exit],
            Cmd::ToggleResident => {
                self.settings.resident = !self.settings.resident;
                settings_store::save(&self.settings);
                vec![Action::SetResident(self.settings.resident)]
            }
            Cmd::ToggleAutoUpdate => {
                self.settings.auto_update = !self.settings.auto_update;
                settings_store::save(&self.settings);
                vec![]
            }
            Cmd::ToggleGrid => {
                if self.browse.grid.is_some() {
                    self.browse.grid = None;
                    self.browse.hover = None;
                } else {
                    let current = self.current.as_ref().and_then(|c| c.index).unwrap_or(0);
                    let (client, scale) = (self.client_f(), self.scale());
                    self.browse
                        .open_grid(self.list.len(), current, client, scale);
                }
                self.invalidate();
                vec![]
            }
            Cmd::ToggleFilmstrip => {
                self.settings.filmstrip = !self.settings.filmstrip;
                settings_store::save(&self.settings);
                // The image area changed; the window grows or shrinks by the
                // strip in "fit window to image".
                self.sized_for = None;
                self.invalidate();
                self.fit_window_actions()
            }
            Cmd::ToggleColorManagement => {
                self.settings.color_management = !self.settings.color_management;
                settings_store::save(&self.settings);
                self.color.set_enabled(self.settings.color_management);
                self.gpu_luts.clear();
                self.invalidate();
                vec![]
            }
            Cmd::ToggleSingleInstance => {
                self.settings.single_instance = !self.settings.single_instance;
                settings_store::save(&self.settings);
                vec![]
            }
            Cmd::About => vec![Action::About],
            Cmd::Notices => vec![Action::Notices],
            Cmd::SaveCutout => match self.save_job() {
                Some(job) => vec![Action::SaveCutout(job)],
                None => vec![],
            },
        }
    }

    /// Announces the selection (or crop, or whole image) on the clipboard;
    /// the pixels are produced only when someone pastes.
    fn copy(&mut self) {
        let Some(cur) = &self.current else { return };
        let Some(p) = self.probes.get(&cur.cache_id) else {
            return;
        };
        let o = p.orientation.then(self.user_rotation);
        let (ow, oh) = o.oriented_size(p.width, p.height);
        let rect = self
            .selection
            .and_then(|s| s.to_pixels(ow, oh))
            .or(self.view.crop)
            .unwrap_or(PixelRect::new(0, 0, ow, oh));
        let stored_rect = selection::to_stored(&rect, o, (p.width, p.height));
        if clipboard::announce(self.hwnd) {
            self.clip = Some(clipboard::Source {
                path: cur.path.clone(),
                stored_rect,
                orientation: o,
            });
        }
    }

    /// The selection (or, without one, the view crop) as a save job.
    fn save_job(&self) -> Option<SaveJob> {
        let cur = self.current.as_ref()?;
        let p = self.probes.get(&cur.cache_id)?;
        let view = p.orientation.then(self.user_rotation);
        let (ow, oh) = view.oriented_size(p.width, p.height);
        let rect = self
            .selection
            .and_then(|s| s.to_pixels(ow, oh))
            .or(self.view.crop)?;
        let stored_rect = selection::to_stored(&rect, view, (p.width, p.height));
        let prefer_png = p.format != blinzel_core::format::Format::Jpeg;
        let stem = cur
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = if prefer_png { "png" } else { "jpg" };
        Some(SaveJob {
            source: cur.path.clone(),
            cut: blinzel_core::export::Cutout {
                stored_rect,
                file_orientation: p.orientation,
                view_orientation: view,
            },
            default_name: format!("{stem}_{}.{ext}", tr(Msg::SaveSuffix)),
            prefer_png,
        })
    }

    pub fn show_notice(&mut self, text: String) {
        self.notice = Some(text);
        self.invalidate();
    }

    fn draw_notice(&mut self) {
        let Some(text) = self.notice.clone() else {
            return;
        };
        let s = self.scale();
        let (cw, ch) = (self.client.0 as f64, self.client.1 as f64);
        let ctx = self.gfx.context.clone();
        let Some(t) = self.text() else { return };
        let Ok(t) = t.get(&text, (13.0 * s) as f32, (cw * 0.8).max(100.0) as f32) else {
            return;
        };
        let (srv, size) = (t.srv.clone(), t.size);
        let pad = 8.0 * s;
        let (w, h) = (size.0 as f64 + 2.0 * pad, size.1 as f64 + 2.0 * pad);
        let x = ((cw - w) / 2.0).round();
        let y = (ch - h - 24.0 * s).round();
        self.renderer.draw_shapes(
            &ctx,
            &[Shape::Fill {
                rect: Rect::new(x, y, w, h),
                radius: (6.0 * s) as f32,
                color: Rgba(0.0, 0.0, 0.0, 0.78),
            }],
        );
        self.renderer.draw_text(
            &ctx,
            &srv,
            ((x + pad) as f32, (y + pad) as f32),
            size,
            Rgba(1.0, 1.0, 1.0, 1.0),
        );
    }

    /// After the file was moved to the recycle bin.
    pub fn removed(&mut self, path: &Path) -> Vec<Action> {
        let Some(cur) = &self.current else {
            return vec![];
        };
        if cur.path != path {
            return vec![];
        }
        match cur.index.and_then(|i| self.list.remove(i)) {
            Some(next) => self.go_to_index(next),
            None => {
                self.current = None;
                self.shown = None;
                self.update_title();
                self.invalidate();
                vec![]
            }
        }
    }

    pub fn resized(&mut self, client: (u32, u32)) {
        if client.0 == 0 || client.1 == 0 {
            return;
        }
        self.client = client;
        let _ = self.swap.resize(&self.gfx, client);
    }

    /// Screen box for the Screen stage: the monitor the window is on.
    pub fn update_screen_box(&self) {
        if let Some(m) = monitor_rect(self.hwnd, false) {
            self.loader
                .set_screen_box(((m.right - m.left) as u32, (m.bottom - m.top) as u32));
        }
    }
}

/// Cursor shapes the window procedure maps to system cursors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    Arrow,
    Cross,
    Hand,
    Move,
    SizeNS,
    SizeWE,
    SizeNWSE,
    SizeNESW,
}

/// Monitor rectangle (or work area) of the monitor showing `hwnd`.
pub fn monitor_rect(hwnd: HWND, work: bool) -> Option<RECT> {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    unsafe {
        let m = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(m, &mut mi)
            .as_bool()
            .then_some(if work { mi.rcWork } else { mi.rcMonitor })
    }
}
