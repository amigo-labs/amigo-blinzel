//! UI-thread state: the current image, navigation and prefetch policy, view
//! state, selection, rendering. It never decodes; it shows the best texture
//! that is ready and tells the loader what to prepare next.
//!
//! Anything that can re-enter the platform's event loop (menus, dialogs,
//! window moves) is returned as an [`Action`] and executed by the shell
//! after the state borrow has been released.

use std::any::Any;
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

use crate::backend::{Backend, Canvas, CursorKind, Device, Host, Key, Services, Timer, Wake};
use crate::batch_dialog::{self, BatchDialog, RunMsg};
use crate::browse::{Browse, grid_layout, strip_height};
use crate::chrome::{Button, Chrome};
use crate::color::ColorManager;
use crate::gfx::{ImageInfo, Rgba, Shape};
use crate::loader::{self, GpuAnimation, Job, Loaded, Loader, Stage};
use crate::menu::{Cmd, MenuState};
use crate::popup::{Outcome, Popup};
use crate::settings_store;

/// An image stage on the GPU of backend `B`.
type Img<B> = <<B as Backend>::Device as Device>::Image;
type Lut<B> = <<B as Backend>::Device as Device>::Lut;

/// Deferred work that may re-enter the window procedure.
#[derive(Debug)]
pub enum Action {
    ShowMenu {
        x: i32,
        y: i32,
    },
    /// Resize the window so its client area becomes `client` (physical
    /// pixels), centred where it is now. With `grow_only` the window only
    /// grows towards that size when `client` exceeds it, never shrinks.
    FitWindow {
        client: (u32, u32),
        grow_only: bool,
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
    /// Ask for a folder for the batch dialog (then
    /// [`App::batch_folder_picked`]).
    PickFolder,
    /// Show a folder in the platform's file manager.
    OpenFolder(PathBuf),
    Activate,
    Uncloak,
    Quit,
    /// The system's "Open with" choice for a file.
    OpenWith(PathBuf),
    /// Start `editor` with `file`.
    EditWith {
        editor: PathBuf,
        file: PathBuf,
    },
    /// Ask for an editor program (then [`App::editor_chosen`]); `then`
    /// is the file to open with it.
    ChooseEditor {
        then: Option<PathBuf>,
    },
    /// Take a screenshot: pick a region (or window), or the whole monitor
    /// under the pointer at once.
    Capture {
        whole_screen: bool,
    },
    /// Register or drop the global screenshot key.
    SetScreenshotHotkey(bool),
    /// Ask for the screenshot folder (then
    /// [`App::screenshot_folder_picked`]).
    PickScreenshotFolder,
}

/// `file` in the configured editor, or the question for one first.
fn edit_action(editor: Option<&str>, file: PathBuf) -> Action {
    match editor {
        Some(e) => Action::EditWith {
            editor: PathBuf::from(e),
            file,
        },
        None => Action::ChooseEditor { then: Some(file) },
    }
}

/// A cutout to save (selection, otherwise the view crop).
#[derive(Debug, Clone)]
pub struct SaveJob {
    pub source: PathBuf,
    pub cut: blinzel_core::export::Cutout,
    pub default_name: String,
    pub prefer_png: bool,
}

/// What a copy put on the clipboard: decoded only when someone pastes.
#[derive(Clone, Debug)]
pub struct ClipSource {
    pub path: PathBuf,
    /// Region in stored (file) orientation.
    pub stored_rect: PixelRect,
    /// Orientation to apply (EXIF plus the user's rotation).
    pub orientation: Orientation,
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

pub struct App<B: Backend> {
    pub host: B::Host,
    pub device: Arc<B::Device>,
    pub canvas: B::Canvas,
    pub services: Arc<B::Services>,
    pub settings: Settings,
    pub loader: Loader<B::Device, B::Services>,
    list: FileList,
    list_rx: Option<Receiver<(PathBuf, std::io::Result<Vec<Entry>>)>>,
    /// The watched folder and the platform's watch on it.
    watcher: Option<(PathBuf, Box<dyn Any>)>,
    pub current: Option<Current>,
    textures: ByteLru<u64, Arc<Img<B>>>,
    vram_budget: usize,
    probes: HashMap<u64, Probe>,
    failed: HashMap<u64, String>,
    /// What is on screen (may be the previous image while the next loads).
    shown: Option<(u64, Arc<Img<B>>)>,
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
    gpu_luts: HashMap<usize, (Arc<blinzel_core::lut::Lut3d>, Option<Lut<B>>)>,
    /// The monitor the colour target was chosen for.
    pub monitor: isize,
    /// The remembered colour target was checked against Windows.
    color_verified: bool,
    /// Grid browser and filmstrip.
    pub browse: Browse<B::Services>,
    /// The current image's animation (`None`: it is a still image).
    animations: HashMap<u64, Option<Arc<GpuAnimation<Img<B>>>>>,
    /// Frame shown and complete plays so far.
    anim_frame: usize,
    anim_plays: u32,
    /// The timer reached the last frame decoded so far; the next delivery
    /// of frames continues playback.
    anim_waiting: bool,
    pub dpi: u32,
    pub client: (u32, u32),
    direction: isize,
    pub clip: Option<ClipSource>,
    first_image_shown: bool,
    sized_for: Option<u64>,
    pub exit_after_first_frame: Option<PathBuf>,
    /// Short message shown at the bottom (saved, errors), cleared by timer.
    pub notice: Option<String>,
    /// The menu drawn by the UI itself (platforms without native popup
    /// menus in the window).
    pub popup: Option<Popup>,
    /// The batch dialog, open over the viewer.
    pub dialog: Option<BatchDialog>,
    /// Its settings from the last time it was open.
    last_dialog: Option<BatchDialog>,
}

impl<B: Backend> App<B> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        host: B::Host,
        device: Arc<B::Device>,
        canvas: B::Canvas,
        services: Arc<B::Services>,
        settings: Settings,
        loader: Loader<B::Device, B::Services>,
        color: Arc<ColorManager>,
        dpi: u32,
        client: (u32, u32),
    ) -> App<B> {
        let vram_budget = device.texture_budget();
        App {
            host,
            device,
            canvas,
            services,
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
            anim_waiting: false,
            dpi,
            client,
            direction: 1,
            clip: None,
            first_image_shown: false,
            sized_for: None,
            exit_after_first_frame: None,
            notice: None,
            popup: None,
            dialog: None,
            last_dialog: None,
        }
    }

    pub fn scale(&self) -> f64 {
        self.dpi as f64 / 96.0
    }

    pub fn chrome(&self) -> Chrome {
        Chrome {
            scale: self.scale(),
            client: (self.client.0 as f64, self.client.1 as f64),
            rtl: blinzel_i18n::is_rtl(),
            system_buttons: cfg!(target_os = "macos"),
        }
    }

    pub fn invalidate(&self) {
        self.host.invalidate();
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
        self.canvas.trim();
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
        blinzel_i18n::set_lang(lang.unwrap_or_else(|| self.services.system_language()));
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
        let Ok(gpu) = self.device.upload(&img, Orientation::NORMAL) else {
            return;
        };
        let bg = Rgba::from_u8(self.settings.background, 1.0);
        let full = gpu.full_size();
        let l = compute(
            full,
            (self.client.0 as f64, self.client.1 as f64),
            &self.settings.view,
            &ViewState::default(),
        );
        let stage = gpu.size();
        let xform = stage_to_client(&l, Orientation::NORMAL, full, stage);
        let content = content_in_stage(&l, Orientation::NORMAL, full, stage);
        if self.canvas.begin(bg) {
            self.canvas.draw_image(&gpu, &xform, &content, bg, None);
            self.canvas.present();
        }
        // The colour system loads on first use: once now, with the profile
        // most photos with one carry (phones: Display P3).
        if self.settings.color_management {
            self.color.warm_up();
        }
    }

    // ---------------------------------------------------------------- files

    /// Opens a file: shows it as soon as it is decoded, lists its folder in
    /// the background.
    pub fn open(&mut self, path: PathBuf) -> Vec<Action> {
        if self.hidden {
            self.reload_settings();
        }
        let entry = match self.services.entry(&path) {
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
        let services = self.services.clone();
        let _ = std::thread::Builder::new()
            .name("blinzel-list".into())
            .spawn(move || {
                let t = std::time::Instant::now();
                let r = services.list_dir(&dir);
                timing::record("list_dir_us", t.elapsed().as_secs_f64() * 1e6);
                let _ = tx.send((dir, r));
                services.wake(Wake::List);
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
        if self.watcher.as_ref().is_none_or(|w| w.0 != dir) {
            self.watcher = self.host.watch_dir(&dir).map(|w| (dir.clone(), w));
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
        self.anim_waiting = false;
        self.host.kill_timer(Timer::Anim);
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
        self.host.set_timer(Timer::Anim, ms.max(10));
    }

    /// The current image's animation, if it has one.
    fn animation(&self) -> Option<Arc<GpuAnimation<Img<B>>>> {
        let c = self.current.as_ref()?;
        self.animations.get(&c.cache_id)?.clone()
    }

    /// The animation timer: next frame; at the end of the last play the
    /// last frame stays.
    pub fn on_anim_timer(&mut self) {
        self.host.kill_timer(Timer::Anim);
        let Some(a) = self.animation() else { return };
        let next = self.anim_frame + 1;
        if next == a.frames.len() && !a.complete {
            // Still decoding: hold this frame until more arrive.
            self.anim_waiting = true;
            return;
        }
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
        self.host.set_title(&title);
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
            let texel_per_px = l.scale * img.full_size().0 as f64 / img.size().0.max(1) as f64;
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
        // Wanted until complete: a growing animation is still decoding.
        let anim_done = self
            .animations
            .get(&cur.cache_id)
            .is_some_and(|a| a.as_ref().is_none_or(|a| a.complete));
        if may_animate && !anim_done {
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
        type Out<B> = Loaded<Img<B>>;
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
            let Out::<B> {
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
                    // Later deliveries of a growing animation keep playing.
                    let playing = matches!(self.animations.get(&cache_id), Some(Some(_)));
                    let first = animation
                        .as_ref()
                        .filter(|_| !playing)
                        .map(|a| a.frames[0].1);
                    // A still result after frames were shown keeps them.
                    if animation.is_some() || !playing {
                        self.animations.insert(cache_id, animation);
                    }
                    if let Some(delay) = first {
                        self.anim_frame = 0;
                        self.anim_plays = 0;
                        self.anim_waiting = false;
                        self.schedule_frame(delay);
                    } else if self.anim_waiting {
                        self.anim_waiting = false;
                        self.on_anim_timer();
                    }
                    changed_current = true;
                }
                continue;
            }
            match image {
                Ok(img) => {
                    let bytes = img.bytes();
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

    fn best_texture(&self, cache_id: u64) -> Option<Arc<Img<B>>> {
        [Stage::Full, Stage::Screen, Stage::Preview]
            .into_iter()
            .find_map(|s| self.textures.peek(&loader::key(cache_id, s)).cloned())
    }

    // --------------------------------------------------------------- layout

    fn orientation_of(&self, img: &Img<B>) -> Orientation {
        img.orientation().then(self.user_rotation)
    }

    /// Image to display and its layout.
    fn displayed(&self) -> Option<(Arc<Img<B>>, Orientation, Layout)> {
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
        let full = img.full_size();
        let oriented = o.oriented_size(full.0, full.1);
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
    fn gpu_lut(&mut self, cache_id: u64) -> Option<Lut<B>> {
        let icc = self.probes.get(&cache_id).and_then(|p| p.icc.clone());
        let lut = self.color.lut(icc.as_ref())?;
        let key = Arc::as_ptr(&lut) as usize;
        let device = &self.device;
        self.gpu_luts
            .entry(key)
            .or_insert_with(|| {
                let gpu = device.upload_lut(&lut).ok();
                (lut, gpu)
            })
            .1
            .clone()
    }

    /// Asks the platform for the monitor's colour target in the background
    /// (the start used the remembered one); the answer comes back as
    /// [`Wake::ColorTarget`].
    fn verify_color_target(&mut self) {
        if self.color_verified || !self.settings.color_management {
            return;
        }
        self.color_verified = true;
        let (services, monitor) = (self.services.clone(), self.monitor);
        let _ = std::thread::Builder::new()
            .name("blinzel-color-target".into())
            .spawn(move || {
                let target = services.display_target(monitor);
                services.wake(Wake::ColorTarget { monitor, target });
            });
    }

    /// The colour target the platform reported for `monitor`.
    pub fn on_color_target(&mut self, monitor: isize, target: Option<blinzel_core::icc::Profile>) {
        if monitor == self.monitor && self.color.set_target(target) {
            self.gpu_luts.clear();
            self.invalidate();
        }
    }

    /// Re-targets colour management when the window moved to another
    /// monitor (or the displays changed: `force`).
    pub fn check_monitor(&mut self, force: bool) {
        let Some((m, _)) = self.host.monitor() else {
            return;
        };
        if !force && m == self.monitor {
            return;
        }
        self.monitor = m;
        if self.color.set_target(self.services.display_target(m)) {
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

    /// In "fit window to image" mode the window follows each image; in "fit
    /// image to window" it grows for images larger than the window.
    fn fit_window_actions(&mut self) -> Vec<Action> {
        let grow_only = match self.settings.view.mode {
            ViewMode::FitWindowToImage => false,
            ViewMode::FitImageToWindow => true,
            ViewMode::ActualSize => return vec![],
        };
        if self.fullscreen || self.maximized {
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
            grow_only,
        }]
    }

    // ------------------------------------------------------------ rendering

    pub fn render(&mut self) -> Vec<Action> {
        let bg = Rgba::from_u8(self.settings.background, 1.0);
        if !self.canvas.begin(bg) {
            return vec![];
        }
        let mut shapes: Vec<Shape> = Vec::new();
        let mut showed_image = false;
        let in_grid = self.browse.grid.is_some();
        if in_grid {
            let (client, scale) = (self.client_f(), self.scale());
            let background = self.settings.background;
            self.browse.draw_grid(
                &mut self.canvas,
                &self.services,
                &self.list,
                client,
                scale,
                background,
            );
        } else if let Some((img, o, l)) = self.displayed() {
            let (full, stage) = (img.full_size(), img.size());
            let xform = stage_to_client(&l, o, full, stage);
            let content = content_in_stage(&l, o, full, stage);
            let lut = self.displayed_id().and_then(|id| self.gpu_lut(id));
            self.canvas
                .draw_image(&img, &xform, &content, bg, lut.as_ref());
            if let Some(cur) = &self.current {
                self.shown = Some((cur.cache_id, img.clone()));
                showed_image = self.best_texture(cur.cache_id).is_some();
            }
            if let Some(sel) = self.selection {
                self.selection_shapes(&l, &sel, &mut shapes);
            }
        }
        self.canvas.draw_shapes(&shapes);
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
                &mut self.canvas,
                &self.services,
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
            self.canvas.draw_shapes(&chrome);
        }
        let (client, scale) = (self.client_f(), self.scale());
        if let Some(d) = &mut self.dialog {
            d.draw(&mut self.canvas, client, scale);
        }
        if let Some(p) = &self.popup {
            p.draw(&mut self.canvas);
        }
        self.canvas.present();

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
            self.host.first_frame_shown();
            if let Some(out) = &self.exit_after_first_frame {
                // The adapter tells a real GPU from WARP / Basic Render
                // Driver, which costs hundreds of ms per frame.
                let json = format!(
                    "{{\"adapter\": {}, \"warp\": {}, \"marks\": {}}}",
                    timing::json_str(&self.device.adapter_name()),
                    self.device.is_software(),
                    timing::marks_json()
                );
                let _ = std::fs::write(out, json);
                actions.push(Action::Quit);
            }
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

    fn draw_selection_label(&mut self, l: &Layout, sel: &Rect) {
        let s = self.scale();
        let label = format!("{} × {}", sel.w.round() as u32, sel.h.round() as u32);
        let r = l.image_to_client().map_rect(sel);
        let px = (12.0 * s) as f32;
        let Some(size) = self.canvas.text_size(&label, px, 1000.0) else {
            return;
        };
        let pad = 4.0 * s;
        let x = r.x.round();
        let mut y = (r.y - size.1 as f64 - 2.0 * pad - 4.0 * s).round();
        if y < 0.0 {
            y = (r.y + 4.0 * s).round();
        }
        let bg = Rect::new(x, y, size.0 as f64 + 2.0 * pad, size.1 as f64 + 2.0 * pad);
        self.canvas.draw_shapes(&[Shape::Fill {
            rect: bg,
            radius: (4.0 * s) as f32,
            color: Rgba(0.0, 0.0, 0.0, 0.72),
        }]);
        self.canvas.draw_text(
            &label,
            px,
            1000.0,
            ((x + pad) as f32, (y + pad) as f32),
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
        let (px, max_w) = ((15.0 * s) as f32, (cw * 0.8).max(100.0) as f32);
        let Some(size) = self.canvas.text_size(&message, px, max_w) else {
            return;
        };
        let pos = (
            ((cw - size.0 as f64) / 2.0).round() as f32,
            ((ch - size.1 as f64) / 2.0).round() as f32,
        );
        self.canvas
            .draw_text(&message, px, max_w, pos, Rgba(0.85, 0.85, 0.85, 1.0));
    }

    // ---------------------------------------------------------------- input

    fn image_point(&self, l: &Layout, p: (f64, f64)) -> (f64, f64) {
        l.client_to_image(p)
    }

    fn content_bounds(l: &Layout) -> Rect {
        Rect::new(l.crop_origin.0, l.crop_origin.1, l.content.0, l.content.1)
    }

    /// The menu below its button: from the button's outer corner, which is
    /// the right one in right-to-left languages (the menu opens leftwards).
    fn menu_at_button(&self) -> Action {
        let r = self.chrome().rect(Button::Menu);
        let x = if blinzel_i18n::is_rtl() {
            r.right()
        } else {
            r.x
        };
        let (x, y) = self.host.to_screen((x, r.bottom()));
        Action::ShowMenu { x, y }
    }

    /// Opens the menu drawn by the UI at client point `at` (see
    /// [`App::popup`]).
    pub fn open_menu(&mut self, at: (f64, f64)) {
        let items = crate::menu::items(&self.menu_state());
        let (client, scale) = (self.client_f(), self.scale());
        self.popup = Some(Popup::open(
            items,
            at,
            client,
            scale,
            blinzel_i18n::is_rtl(),
            &mut self.canvas,
        ));
        self.invalidate();
    }

    fn popup_outcome(&mut self, o: Outcome) -> Vec<Action> {
        match o {
            Outcome::Unchanged => vec![],
            Outcome::Redraw => {
                self.invalidate();
                vec![]
            }
            Outcome::Closed => {
                self.popup = None;
                self.invalidate();
                vec![]
            }
            Outcome::Chosen(cmd) => {
                self.popup = None;
                self.invalidate();
                self.command(cmd)
            }
        }
    }

    pub fn mouse_down(
        &mut self,
        p: (f64, f64),
        shift: bool,
        ctrl: bool,
        middle: bool,
    ) -> Vec<Action> {
        if let Some(popup) = &mut self.popup {
            let o = popup.mouse_down(p, &mut self.canvas);
            return self.popup_outcome(o);
        }
        if let Some(d) = &mut self.dialog {
            let o = d.mouse_down(p);
            return self.dialog_outcome(o);
        }
        if !middle && !self.fullscreen && self.chrome().hit(p) == Some(Button::Menu) {
            self.pressed = None;
            return vec![self.menu_at_button()];
        }
        if let Some(handled) = self.browse_click(p, shift, ctrl) {
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
    fn browse_click(&mut self, p: (f64, f64), shift: bool, ctrl: bool) -> Option<Vec<Action>> {
        let (client, scale, count) = (self.client_f(), self.scale(), self.list.len());
        if let Some(state) = &mut self.browse.grid {
            if let Some(i) = grid_layout(count, client, scale).hit(p, state.scroll) {
                let name = |i: usize| self.list.get(i).map(|e| e.name.clone());
                if ctrl {
                    // Toggle one.
                    if let Some(n) = name(i)
                        && !state.marked.remove(&n)
                    {
                        state.marked.insert(n);
                    }
                } else if shift {
                    // Everything from the selected item to this one.
                    let (a, b) = (state.selected.min(i), state.selected.max(i));
                    state.marked.extend((a..=b).filter_map(name));
                } else {
                    state.marked.clear();
                }
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

    /// Thumbnails arrived ([`Wake::Thumbs`]).
    pub fn on_thumbs(&mut self) {
        if self.browse.on_thumbs(&mut self.canvas) {
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
        if let Some(popup) = &mut self.popup {
            let o = popup.mouse_move(p, &mut self.canvas);
            self.popup_outcome(o);
            return;
        }
        if let Some(d) = &mut self.dialog {
            let o = d.mouse_move(p);
            self.dialog_outcome(o);
            return;
        }
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
        if let Some(d) = &mut self.dialog {
            d.mouse_up();
            return;
        }
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
            || self.dialog.is_some()
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
        if self.popup.is_none()
            && let Some(d) = &mut self.dialog
        {
            let o = d.wheel(p, delta);
            return self.dialog_outcome(o);
        }
        if self.popup.is_some() || self.dialog.is_some() {
            return vec![];
        }
        let (client, scale, count) = (self.client_f(), self.scale(), self.list.len());
        if let Some(state) = &mut self.browse.grid {
            let g = grid_layout(count, client, scale);
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
    pub fn key(&mut self, key: Key, ctrl: bool, shift: bool) -> Option<Vec<Action>> {
        use Key::*;
        if let Some(popup) = &mut self.popup {
            let o = popup.key(key, &mut self.canvas);
            return Some(self.popup_outcome(o));
        }
        if let Some(d) = &mut self.dialog {
            let o = match key {
                Tab => d.tab(shift),
                k => d.key(k),
            };
            // The dialog is modal: every key stays with it.
            return Some(self.dialog_outcome(o));
        }
        if ctrl && key == Letter('B') {
            return Some(self.command(Cmd::Batch));
        }
        if let Some(a) = self.grid_key(key, ctrl) {
            return Some(a);
        }
        match key {
            Letter('G') if !ctrl => return Some(self.command(Cmd::ToggleGrid)),
            Letter('T') if !ctrl => return Some(self.command(Cmd::ToggleFilmstrip)),
            Letter('E') if !ctrl => return Some(self.command(Cmd::EditWith)),
            _ => {}
        }
        let overflow = self.layout().map(|l| l.overflows).unwrap_or(false);
        let pan = |app: &mut App<B>, dx: f64, dy: f64| {
            if let Some(l) = app.layout() {
                let step = 0.1 * app.client.0.min(app.client.1) as f64;
                layout::pan(&mut app.view, &l, (-dx * step, -dy * step));
                app.invalidate();
            }
            vec![]
        };
        let a = match key {
            Right if overflow && !ctrl => pan(self, 1.0, 0.0),
            Left if overflow && !ctrl => pan(self, -1.0, 0.0),
            Up if overflow => pan(self, 0.0, -1.0),
            Down if overflow => pan(self, 0.0, 1.0),
            // Right-to-left languages page leftwards.
            Right if blinzel_i18n::is_rtl() => self.command(Cmd::Prev),
            Left if blinzel_i18n::is_rtl() => self.command(Cmd::Next),
            Right | PageDown | Space | Down => self.command(Cmd::Next),
            Left | PageUp | Backspace | Up => self.command(Cmd::Prev),
            Home => self.command(Cmd::First),
            End => self.command(Cmd::Last),
            Plus => self.command(Cmd::ZoomIn),
            Minus => self.command(Cmd::ZoomOut),
            Digit(0) => self.command(Cmd::ZoomReset),
            Digit(1) if !ctrl => self.command(Cmd::View(ViewMode::FitWindowToImage)),
            Digit(2) if !ctrl => self.command(Cmd::View(ViewMode::FitImageToWindow)),
            Digit(3) if !ctrl => self.command(Cmd::View(ViewMode::ActualSize)),
            Letter('R') if !ctrl => self.command(if shift {
                Cmd::RotateLeft
            } else {
                Cmd::RotateRight
            }),
            Letter('L') if !ctrl => self.command(Cmd::RotateLeft),
            F11 | Enter => self.command(Cmd::Fullscreen),
            Letter('C') if ctrl => self.command(Cmd::Copy),
            Letter('X') if ctrl => self.command(Cmd::Cut),
            Letter('A') if ctrl => self.command(Cmd::SelectAll),
            Letter('O') if ctrl => self.command(Cmd::Open),
            Letter('S') if ctrl => self.command(Cmd::SaveCutout),
            Letter('W') if ctrl => vec![Action::Close],
            Delete => self.command(Cmd::Recycle),
            Escape => self.escape(),
            F10 | Menu => vec![self.menu_at_button()],
            _ => return None,
        };
        Some(a)
    }

    /// Keys in the grid: moving the selection, opening, leaving.
    fn grid_key(&mut self, key: Key, ctrl: bool) -> Option<Vec<Action>> {
        use Key::*;
        let (client, scale, count) = (self.client_f(), self.scale(), self.list.len());
        let state = self.browse.grid.as_mut()?;
        if ctrl && key == Letter('A') {
            state
                .marked
                .extend((0..count).filter_map(|i| self.list.get(i).map(|e| e.name.clone())));
            self.invalidate();
            return Some(vec![]);
        }
        if key == Escape && !state.marked.is_empty() {
            state.marked.clear();
            self.invalidate();
            return Some(vec![]);
        }
        let g = grid_layout(count, client, scale);
        let page = g.page_rows(client.1) as isize;
        let i = state.selected;
        let to = match key {
            Left => g.step(i, -1, 0),
            Right => g.step(i, 1, 0),
            Up => g.step(i, 0, -1),
            Down => g.step(i, 0, 1),
            PageUp => g.step(i, 0, -page),
            PageDown => g.step(i, 0, page),
            Home => 0,
            End => count.saturating_sub(1),
            Enter | Space => return Some(self.open_from_grid(i)),
            Escape | Letter('G') => {
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
            has_images: !self.list.is_empty() || self.current.is_some(),
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
            screenshot_hotkey: self.settings.screenshot_hotkey,
            screenshot_save: self.settings.screenshot_save,
            screenshot_open: self.settings.screenshot_open,
            screenshot_edit: self.settings.screenshot_edit,
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
                blinzel_i18n::set_lang(lang.unwrap_or_else(|| self.services.system_language()));
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
                let on = self.settings.resident;
                vec![
                    Action::SetResident(on),
                    Action::SetScreenshotHotkey(on && self.settings.screenshot_hotkey),
                ]
            }
            Cmd::ToggleScreenshotHotkey => {
                // The key lives in the resident instance: turning it on
                // turns resident mode on too.
                let on = !(self.settings.screenshot_hotkey && self.settings.resident);
                self.settings.screenshot_hotkey = on;
                let mut actions = Vec::new();
                if on && !self.settings.resident {
                    self.settings.resident = true;
                    actions.push(Action::SetResident(true));
                }
                settings_store::save(&self.settings);
                actions.push(Action::SetScreenshotHotkey(on));
                actions
            }
            Cmd::ToggleScreenshotSave => {
                self.settings.screenshot_save = !self.settings.screenshot_save;
                settings_store::save(&self.settings);
                vec![]
            }
            Cmd::ToggleScreenshotOpen => {
                self.settings.screenshot_open = !self.settings.screenshot_open;
                settings_store::save(&self.settings);
                vec![]
            }
            Cmd::ToggleScreenshotEdit => {
                self.settings.screenshot_edit = !self.settings.screenshot_edit;
                settings_store::save(&self.settings);
                vec![]
            }
            Cmd::ScreenshotFolder => vec![Action::PickScreenshotFolder],
            Cmd::Capture => vec![Action::Capture {
                whole_screen: false,
            }],
            Cmd::CaptureScreen => vec![Action::Capture { whole_screen: true }],
            Cmd::OpenWith => match &self.current {
                Some(c) => vec![Action::OpenWith(c.path.clone())],
                None => vec![],
            },
            Cmd::EditWith => {
                let Some(file) = self.current.as_ref().map(|c| c.path.clone()) else {
                    return vec![];
                };
                vec![edit_action(self.settings.editor.as_deref(), file)]
            }
            Cmd::ChooseEditor => vec![Action::ChooseEditor { then: None }],
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
            Cmd::Batch => {
                self.open_batch();
                vec![]
            }
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
        if self.host.offer_clipboard() {
            self.clip = Some(ClipSource {
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

    /// Opens the batch dialog with an empty list (files are dropped onto
    /// it), keeping the last settings.
    fn open_batch(&mut self) {
        self.dialog = Some(match self.last_dialog.take() {
            Some(d) => d.reopen(),
            None => BatchDialog::new(),
        });
        self.invalidate();
    }

    /// Files dropped onto the window: into the batch dialog's list while
    /// it is open, else the first one is shown.
    pub fn files_dropped(&mut self, paths: Vec<PathBuf>) -> Vec<Action> {
        if let Some(d) = &mut self.dialog {
            let o = d.add(paths);
            return self.dialog_outcome(o);
        }
        match paths.into_iter().next() {
            Some(p) => self.open(p),
            None => vec![],
        }
    }

    fn dialog_outcome(&mut self, o: batch_dialog::Outcome) -> Vec<Action> {
        use batch_dialog::Outcome as O;
        match o {
            O::Unchanged => vec![],
            O::Redraw => {
                self.invalidate();
                vec![]
            }
            O::Close => {
                self.last_dialog = self.dialog.take();
                self.invalidate();
                vec![]
            }
            O::Start(job, items) => {
                self.start_batch(job, items);
                self.invalidate();
                vec![]
            }
            O::Inspect(paths) => {
                self.inspect_batch(paths);
                self.invalidate();
                vec![]
            }
            O::PickFolder => vec![Action::PickFolder],
            O::OpenFolder(p) => vec![Action::OpenFolder(p)],
        }
    }

    /// Reads the headers of files dropped onto the batch dialog in the
    /// background; results go to the dialog.
    fn inspect_batch(&mut self, paths: Vec<PathBuf>) {
        let Some(d) = &mut self.dialog else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        d.inspecting(rx);
        let services = self.services.clone();
        let _ = std::thread::Builder::new()
            .name("blinzel-batch-inspect".into())
            .spawn(move || {
                services.tune_thread(false);
                let source = BatchSource(services.clone());
                for path in paths {
                    let info = blinzel_core::decode::guard(|| {
                        Ok(blinzel_core::batch::plan::inspect(&path, &source))
                    })
                    .unwrap_or_else(|e| Err(e.to_string()));
                    if tx.send((path, info)).is_err() {
                        return;
                    }
                    services.wake(Wake::Batch);
                }
            });
    }

    /// Runs a batch in the background; progress goes to the dialog.
    fn start_batch(
        &mut self,
        job: blinzel_core::batch::Job,
        items: Vec<blinzel_core::batch::plan::Item>,
    ) {
        use blinzel_core::batch::run::{self, Options};
        use std::sync::atomic::AtomicBool;

        let Some(d) = &mut self.dialog else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        d.started(items.len(), rx, cancel.clone());
        let services = self.services.clone();
        let _ = std::thread::Builder::new()
            .name("blinzel-batch".into())
            .spawn(move || {
                services.tune_thread(false);
                let tx2 = std::sync::Mutex::new(tx.clone());
                let progress = |p| {
                    let _ = tx2.lock().unwrap().send(RunMsg::Progress(p));
                    services.wake(Wake::Batch);
                };
                let source = BatchSource(services.clone());
                let summary = run::run(
                    &job,
                    &items,
                    &source,
                    &Options::default(),
                    &progress,
                    &cancel,
                );
                let _ = tx.send(RunMsg::Finished(summary));
                services.wake(Wake::Batch);
            });
    }

    /// Progress of a running batch ([`Wake::Batch`]).
    pub fn on_batch(&mut self) {
        if self.dialog.as_mut().is_some_and(|d| d.poll()) {
            self.invalidate();
        }
    }

    /// The folder the platform's dialog returned for the batch.
    /// The editor the user picked; opens `then` with it.
    pub fn editor_chosen(&mut self, editor: PathBuf, then: Option<PathBuf>) -> Vec<Action> {
        let Some(e) = editor.to_str().filter(|e| !e.contains(['"', '\n', '\r'])) else {
            return vec![];
        };
        self.settings.editor = Some(e.to_string());
        settings_store::save(&self.settings);
        then.map(|file| Action::EditWith { editor, file })
            .into_iter()
            .collect()
    }

    /// A screenshot is on the clipboard and, when the settings asked for
    /// a file, in `file`: shows it in the viewer, then hands it to the
    /// editor (which then lies on top), as configured.
    pub fn on_screenshot(&mut self, file: Option<PathBuf>) -> Vec<Action> {
        let Some(file) = file else {
            return vec![];
        };
        let mut actions = Vec::new();
        if self.settings.screenshot_open {
            actions.extend(self.open(file.clone()));
            actions.push(Action::Wake { opened: true });
        }
        if self.settings.screenshot_edit {
            actions.push(edit_action(self.settings.editor.as_deref(), file));
        }
        actions
    }

    pub fn screenshot_folder_picked(&mut self, folder: PathBuf) {
        if let Some(d) = folder.to_str().filter(|d| !d.contains(['"', '\n', '\r'])) {
            self.settings.screenshot_dir = Some(d.to_string());
            settings_store::save(&self.settings);
        }
    }

    pub fn batch_folder_picked(&mut self, folder: PathBuf) {
        if let Some(d) = &mut self.dialog {
            d.set_folder(folder);
            self.invalidate();
        }
    }

    /// Typed text (characters, not keys) for the batch dialog's fields.
    pub fn text_input(&mut self, text: &str) {
        if let Some(d) = &mut self.dialog {
            let o = d.text_input(text);
            self.dialog_outcome(o);
        }
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
        let (px, max_w) = ((13.0 * s) as f32, (cw * 0.8).max(100.0) as f32);
        let Some(size) = self.canvas.text_size(&text, px, max_w) else {
            return;
        };
        let pad = 8.0 * s;
        let (w, h) = (size.0 as f64 + 2.0 * pad, size.1 as f64 + 2.0 * pad);
        let x = ((cw - w) / 2.0).round();
        let y = (ch - h - 24.0 * s).round();
        self.canvas.draw_shapes(&[Shape::Fill {
            rect: Rect::new(x, y, w, h),
            radius: (6.0 * s) as f32,
            color: Rgba(0.0, 0.0, 0.0, 0.78),
        }]);
        self.canvas.draw_text(
            &text,
            px,
            max_w,
            ((x + pad) as f32, (y + pad) as f32),
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
        self.canvas.resize(client);
    }

    /// Screen box for the Screen stage: the monitor the window is on.
    pub fn update_screen_box(&self) {
        if let Some((_, size)) = self.host.monitor() {
            self.loader.set_screen_box(size);
        }
    }
}

/// Reads the source, exports the cutout and writes it atomically (temp
/// file + rename). Returns the message for the notice toast.
/// The platform's decoders and clock for batch work.
struct BatchSource<S>(Arc<S>);

impl<S: Services> blinzel_core::batch::run::Source for BatchSource<S> {
    fn probe(&self, data: &[u8]) -> Result<Probe, blinzel_core::decode::DecodeError> {
        self.0.probe(data)
    }
    fn decode(
        &self,
        data: &[u8],
        req: blinzel_core::decode::Request,
    ) -> Result<blinzel_core::pixel::Image, blinzel_core::decode::DecodeError> {
        self.0.decode(data, req, 1)
    }
    fn utc_offset(&self, t: std::time::SystemTime) -> i32 {
        self.0.utc_offset(t)
    }
}

pub fn save_cutout(services: &impl Services, job: &SaveJob, target: &Path) -> String {
    use blinzel_core::export::{SaveFormat, export};
    use blinzel_i18n::{Msg, trf};
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let result = (|| -> Result<bool, String> {
        let data = services.read_file(&job.source).map_err(|e| e.to_string())?;
        let decode = |d: &[u8], req| services.decode(d, req, 1);
        let saved = export(&data, &job.cut, SaveFormat::from_path(target), &decode)?;
        let tmp = target.with_extension("blinzel-tmp");
        std::fs::write(&tmp, &saved.bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, target).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        })?;
        Ok(saved.lossless)
    })();
    match result {
        Ok(true) => trf(Msg::SavedLossless, &[("name", &name)]),
        Ok(false) => trf(Msg::Saved, &[("name", &name)]),
        Err(e) => trf(Msg::ErrorSave, &[("name", &name), ("reason", &e)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_asks_for_an_editor_first() {
        let file = PathBuf::from("shot.png");
        match edit_action(None, file.clone()) {
            Action::ChooseEditor { then } => assert_eq!(then, Some(file.clone())),
            a => panic!("{a:?}"),
        }
        match edit_action(Some("paint.exe"), file.clone()) {
            Action::EditWith { editor, file: f } => {
                assert_eq!(editor, PathBuf::from("paint.exe"));
                assert_eq!(f, file);
            }
            a => panic!("{a:?}"),
        }
    }
}
