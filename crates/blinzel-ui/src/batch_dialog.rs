//! The batch dialog: resize many images at once (`blinzel_core::batch`).
//! A modal panel drawn over the viewer, with the same controls on every
//! platform. Images and folders are dropped onto it ([`BatchFiles`], which
//! shows the name every copy will get); it edits a [`Job`], hands both to
//! the app to run, then shows the progress and the result.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use blinzel_core::batch::naming;
use blinzel_core::batch::plan::Item;
use blinzel_core::batch::run::{Progress, Summary};
use blinzel_core::batch::{Codec, Job, Output, Sharpen, Size};
use blinzel_core::geom::Rect;
use blinzel_i18n::{Msg, PluralMsg, tr, trf, trp};

use crate::backend::{Canvas, Key};
use crate::batch_files::{self, BatchFiles, Inspected, ROW, file_name};
use crate::gfx::Rgba;
use crate::widgets::{self, CHECK_ROW, CONTROL_H, Painter, TEXT_BODY, TEXT_SMALL, TEXT_TITLE};

/// Messages from the running batch.
pub enum RunMsg {
    Progress(Progress),
    Finished(Summary),
}

/// What an input asks the app to do.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Unchanged,
    Redraw,
    Close,
    /// Run `job` over `items`.
    Start(Job, Vec<Item>),
    /// Read these files' headers (then [`BatchDialog::inspecting`]).
    Inspect(Vec<PathBuf>),
    /// Ask the platform for a folder (then [`BatchDialog::set_folder`]).
    PickFolder,
    OpenFolder(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    First,
    Second,
    Pattern,
    Folder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Remove(usize),
    Clear,
    Mode(usize),
    Format(usize),
    NoEnlarge,
    Lossless,
    Sharpen,
    KeepMeta,
    StripGps,
    KeepIcc,
    Field(Field),
    Slider,
    PickFolder,
    Start,
    Cancel,
    Close,
    OpenFolder,
}

/// Size modes in the order of the segmented control.
const MODES: [Msg; 6] = [
    Msg::BatchLongEdge,
    Msg::BatchWidth,
    Msg::BatchHeight,
    Msg::BatchFit,
    Msg::BatchFill,
    Msg::BatchPercent,
];

const FORMATS: [Option<Codec>; 5] = [
    None,
    Some(Codec::Jpeg),
    Some(Codec::Png),
    Some(Codec::Webp),
    Some(Codec::Avif),
];

/// The placeholders, under the name field (not translated: they are
/// what is typed).
const PLACEHOLDERS: &str = "{name}  {n:3}  {w}  {h}  {date}  {date:DD.MM.YYYY_hh-mm}";

// The layout (logical pixels). A label gutter on the left, every control
// from one content edge right of it; the panel is as tall as its content
// and the window allow, the list takes the whole rows that are left.

/// The spacing scale: inside a group, between rows, between groups, and
/// the panel's padding (also above the footer).
const GAP_TIGHT: f64 = 4.0;
const GAP_ROW: f64 = 8.0;
const GAP_GROUP: f64 = 16.0;
const PAD: f64 = 24.0;
/// The panel: width, corner, and the least room to the window's edge.
const WIDTH: f64 = 640.0;
const RADIUS: f64 = 12.0;
const MARGIN: f64 = 8.0;
/// The header row (title, count, "Remove all"); also a run's headline
/// and the header of its error list.
const HEAD_H: f64 = 24.0;
/// Where the baseline sits, as a share of the line height the canvas
/// measures (cosmic-text centres a sans font's glyphs in 1.3 × size
/// lines: baseline at 0.76), to set smaller text on the title's baseline.
const ASCENT: f64 = 0.76;
const HINT_H: f64 = 16.0;
const BUTTON_H: f64 = 32.0;
const BAR_H: f64 = 8.0;
/// A size field (six digits), the folder picker, the quality number.
const FIELD_W: f64 = 80.0;
const PICK_W: f64 = 40.0;
const VALUE_W: f64 = 32.0;
/// The label gutter: its widest label plus a group gap, within limits.
const GUTTER_MIN: f64 = 96.0;
const GUTTER_MAX: f64 = 160.0;
/// The list shows at least this many rows (fewer only when the window
/// is too small); a taller window gives all its spare height to the list.
const LIST_MIN_ROWS: usize = 4;
/// A run keeps (and shows) its first failures, this many.
const ERRORS_SHOWN: usize = 4;
/// The labels of the gutter, top to bottom.
const LABELS: [Msg; 6] = [
    Msg::BatchSize,
    Msg::BatchFormat,
    Msg::BatchQuality,
    Msg::BatchOptions,
    Msg::BatchName,
    Msg::BatchFolder,
];

struct Run {
    total: usize,
    done: usize,
    failed: usize,
    /// The first failures: file name and reason.
    errors: Vec<(String, String)>,
    /// A folder that received output.
    output: Option<PathBuf>,
    cancel: Arc<AtomicBool>,
    rx: Receiver<RunMsg>,
    summary: Option<Summary>,
}

impl Run {
    /// Rows of the error list: the most a run can show, reserved from the
    /// start and kept after it, so the sheet never changes its size.
    fn error_rows(&self) -> usize {
        ERRORS_SHOWN.min(self.total).max(1)
    }

    /// The headline: how far it is, or how it went.
    fn headline(&self) -> String {
        match &self.summary {
            Some(s) if s.skipped > 0 => trf(
                Msg::BatchCancelled,
                &[("done", &s.done), ("total", &self.total)],
            ),
            Some(s) => trf(Msg::BatchDone, &[("done", &s.done), ("failed", &s.failed)]),
            None => trf(
                Msg::BatchProgress,
                &[("done", &(self.done + self.failed)), ("total", &self.total)],
            ),
        }
    }
}

pub struct BatchDialog {
    files: BatchFiles,
    mode: usize,
    first: String,
    second: String,
    no_enlarge: bool,
    format: usize,
    quality: u8,
    lossless: bool,
    sharpen: bool,
    keep_meta: bool,
    strip_gps: bool,
    keep_icc: bool,
    pattern: String,
    folder: String,
    focus: Option<Field>,
    hover: Option<Hit>,
    dragging_slider: bool,
    /// Hit areas of the last frame.
    hits: Vec<(Rect, Hit)>,
    slider: Rect,
    run: Option<Run>,
}

impl Default for BatchDialog {
    fn default() -> Self {
        BatchDialog::new()
    }
}

impl BatchDialog {
    pub fn new() -> BatchDialog {
        let job = Job::default();
        BatchDialog {
            files: BatchFiles::default(),
            mode: 0,
            first: "1920".into(),
            second: "1080".into(),
            no_enlarge: !job.enlarge,
            format: 0,
            quality: 85,
            lossless: false,
            sharpen: false,
            keep_meta: job.keep_exif,
            strip_gps: !job.keep_gps,
            keep_icc: job.keep_icc,
            pattern: job.pattern,
            folder: job.out_dir.to_string_lossy().into_owned(),
            focus: None,
            hover: None,
            dragging_slider: false,
            hits: Vec::new(),
            slider: Rect::default(),
            run: None,
        }
    }

    /// The same settings with an empty list (the dialog opened again).
    pub fn reopen(mut self) -> BatchDialog {
        self.files.clear();
        self.run = None;
        self.focus = None;
        self.hover = None;
        self
    }

    /// The files in the list, in order (tests).
    #[cfg(test)]
    pub(crate) fn file_paths(&self) -> Vec<PathBuf> {
        self.files.paths()
    }

    /// Where `hit` was drawn last (tests).
    #[cfg(test)]
    fn drawn_at(&self, hit: Hit) -> Option<Rect> {
        self.hits.iter().find(|(_, h)| *h == hit).map(|(r, _)| *r)
    }

    /// Where the "Open folder" button was drawn last (tests).
    #[cfg(test)]
    pub(crate) fn open_folder_button(&self) -> Option<Rect> {
        self.drawn_at(Hit::OpenFolder)
    }

    /// Where the Cancel button was drawn last (tests).
    #[cfg(test)]
    pub(crate) fn cancel_button(&self) -> Option<Rect> {
        self.drawn_at(Hit::Cancel)
    }

    /// Where the "sharpen" switch was drawn last (tests).
    #[cfg(test)]
    pub(crate) fn sharpen_switch(&self) -> Option<Rect> {
        self.drawn_at(Hit::Sharpen)
    }

    pub fn is_running(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.summary.is_none())
    }

    pub fn set_folder(&mut self, folder: PathBuf) {
        self.folder = folder.to_string_lossy().into_owned();
    }

    /// Dropped files and folders, for the list (not while a batch runs or
    /// shows its result).
    pub fn add(&mut self, paths: Vec<PathBuf>) -> Outcome {
        if self.run.is_some() {
            return Outcome::Unchanged;
        }
        let new = self.files.add(paths);
        if new.is_empty() {
            Outcome::Unchanged
        } else {
            Outcome::Inspect(new)
        }
    }

    /// Header results for an [`Outcome::Inspect`] come through `rx`.
    pub fn inspecting(&mut self, rx: Receiver<Inspected>) {
        self.files.inspecting(rx);
    }

    fn two_values(&self) -> bool {
        matches!(self.mode, 3 | 4)
    }

    fn number(s: &str) -> Option<u32> {
        s.trim().parse().ok().filter(|&n| n > 0 && n <= 100_000)
    }

    fn size(&self) -> Option<Size> {
        let a = Self::number(&self.first)?;
        Some(match self.mode {
            0 => Size::LongEdge(a),
            1 => Size::Width(a),
            2 => Size::Height(a),
            3 => Size::Fit {
                w: a,
                h: Self::number(&self.second)?,
            },
            4 => Size::Fill {
                w: a,
                h: Self::number(&self.second)?,
            },
            _ => Size::Percent(a.min(1000) as f32),
        })
    }

    /// The job as edited, if it is complete.
    pub fn job(&self) -> Option<Job> {
        let q = self.quality.clamp(1, 100);
        let job = Job {
            size: self.size()?,
            enlarge: !self.no_enlarge,
            sharpen: self.sharpen.then(Sharpen::default),
            output: FORMATS[self.format].map_or(Output::Same, Output::Codec),
            quality: blinzel_core::batch::Quality {
                jpeg: q,
                webp: q,
                webp_lossless: self.lossless,
                avif: q,
                ..Default::default()
            },
            keep_exif: self.keep_meta,
            keep_gps: !self.strip_gps,
            keep_icc: self.keep_icc,
            pattern: self.pattern.clone(),
            out_dir: PathBuf::from(self.folder.trim()),
        };
        (!self.folder.trim().is_empty() && job.validate().is_ok()).then_some(job)
    }

    /// The batch started over `total` files: progress comes through `rx`.
    pub fn started(&mut self, total: usize, rx: Receiver<RunMsg>, cancel: Arc<AtomicBool>) {
        self.focus = None;
        self.run = Some(Run {
            total,
            done: 0,
            failed: 0,
            errors: Vec::new(),
            output: None,
            cancel,
            rx,
            summary: None,
        });
    }

    /// Takes the header results and progress messages that arrived. True
    /// if any did.
    pub fn poll(&mut self) -> bool {
        let mut any = self.files.poll();
        let Some(run) = &mut self.run else {
            return any;
        };
        while let Ok(m) = run.rx.try_recv() {
            any = true;
            match m {
                RunMsg::Progress(Progress::Done { output, .. }) => {
                    run.done += 1;
                    if run.output.is_none() {
                        run.output = output.parent().map(PathBuf::from);
                    }
                }
                RunMsg::Progress(Progress::Failed { source, error, .. }) => {
                    run.failed += 1;
                    if run.errors.len() < ERRORS_SHOWN {
                        run.errors.push((file_name(&source), error));
                    }
                }
                RunMsg::Finished(s) => run.summary = Some(s),
            }
        }
        any
    }

    fn cancel(&mut self) {
        if let Some(run) = &self.run {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn hit(&self, p: (f64, f64)) -> Option<Hit> {
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(p))
            .map(|(_, h)| *h)
    }

    fn field_mut(&mut self, f: Field) -> &mut String {
        match f {
            Field::First => &mut self.first,
            Field::Second => &mut self.second,
            Field::Pattern => &mut self.pattern,
            Field::Folder => &mut self.folder,
        }
    }

    fn set_quality_at(&mut self, x: f64) {
        let r = self.slider;
        if r.w > 0.0 {
            let v = ((x - r.x) / r.w).clamp(0.0, 1.0);
            self.quality = (1.0 + v * 99.0).round() as u8;
        }
    }

    pub fn mouse_down(&mut self, p: (f64, f64)) -> Outcome {
        let Some(h) = self.hit(p) else {
            if self.focus.take().is_some() {
                return Outcome::Redraw;
            }
            return Outcome::Unchanged;
        };
        if !matches!(h, Hit::Field(_)) {
            self.focus = None;
        }
        match h {
            Hit::Remove(i) => self.files.remove(i),
            Hit::Clear => self.files.clear(),
            Hit::Mode(i) => self.mode = i,
            Hit::Format(i) => self.format = i,
            Hit::NoEnlarge => self.no_enlarge = !self.no_enlarge,
            Hit::Lossless => self.lossless = !self.lossless,
            Hit::Sharpen => self.sharpen = !self.sharpen,
            Hit::KeepMeta => self.keep_meta = !self.keep_meta,
            Hit::StripGps if self.keep_meta => self.strip_gps = !self.strip_gps,
            Hit::StripGps => return Outcome::Unchanged,
            Hit::KeepIcc => self.keep_icc = !self.keep_icc,
            Hit::Field(f) => self.focus = Some(f),
            Hit::Slider => {
                self.dragging_slider = true;
                self.set_quality_at(p.0);
            }
            Hit::PickFolder => return Outcome::PickFolder,
            Hit::Start => return self.start(),
            Hit::Cancel if self.is_running() => self.cancel(),
            Hit::Cancel | Hit::Close => return Outcome::Close,
            Hit::OpenFolder => {
                return self
                    .run
                    .as_ref()
                    .and_then(|r| r.output.clone())
                    .map_or(Outcome::Unchanged, Outcome::OpenFolder);
            }
        }
        Outcome::Redraw
    }

    pub fn mouse_move(&mut self, p: (f64, f64)) -> Outcome {
        if self.dragging_slider {
            self.set_quality_at(p.0);
            return Outcome::Redraw;
        }
        let h = self.hit(p);
        if h != self.hover {
            self.hover = h;
            return Outcome::Redraw;
        }
        Outcome::Unchanged
    }

    pub fn mouse_up(&mut self) {
        self.dragging_slider = false;
    }

    /// The wheel scrolls the file list under it.
    pub fn wheel(&mut self, p: (f64, f64), delta: i16) -> Outcome {
        if self.run.is_none() && self.files.wheel(p, delta) {
            Outcome::Redraw
        } else {
            Outcome::Unchanged
        }
    }

    fn start(&mut self) -> Outcome {
        if self.run.is_some() {
            return Outcome::Unchanged;
        }
        let Some(job) = self.job() else {
            return Outcome::Unchanged;
        };
        match self.files.items(job.clone()) {
            Some(items) => Outcome::Start(job, items),
            None => Outcome::Unchanged,
        }
    }

    pub fn key(&mut self, key: Key) -> Outcome {
        match key {
            Key::Escape if self.is_running() => {
                self.cancel();
                Outcome::Redraw
            }
            Key::Escape => Outcome::Close,
            Key::Enter if self.run.as_ref().is_some_and(|r| r.summary.is_some()) => Outcome::Close,
            Key::Enter => self.start(),
            Key::Backspace => match self.focus {
                Some(f) => {
                    self.field_mut(f).pop();
                    Outcome::Redraw
                }
                None => Outcome::Unchanged,
            },
            _ => Outcome::Unchanged,
        }
    }

    /// Tab moves between the text fields.
    pub fn tab(&mut self, back: bool) -> Outcome {
        let mut order = vec![Field::First];
        if self.two_values() {
            order.push(Field::Second);
        }
        order.extend([Field::Pattern, Field::Folder]);
        let i = self.focus.and_then(|f| order.iter().position(|&o| o == f));
        let n = order.len();
        self.focus = Some(
            order[match (i, back) {
                (None, false) => 0,
                (None, true) => n - 1,
                (Some(i), false) => (i + 1) % n,
                (Some(i), true) => (i + n - 1) % n,
            }],
        );
        Outcome::Redraw
    }

    /// Typed text for the focused field.
    pub fn text_input(&mut self, text: &str) -> Outcome {
        let Some(f) = self.focus else {
            return Outcome::Unchanged;
        };
        let digits = matches!(f, Field::First | Field::Second);
        let field = self.field_mut(f);
        for c in text.chars().filter(|c| !c.is_control()) {
            if digits && (!c.is_ascii_digit() || field.len() >= 6) {
                continue;
            }
            field.push(c);
        }
        Outcome::Redraw
    }

    fn format_labels() -> Vec<&'static str> {
        FORMATS
            .iter()
            .map(|f| match f {
                None => tr(Msg::BatchSame),
                Some(Codec::Jpeg) => "JPEG",
                Some(Codec::Png) => "PNG",
                Some(Codec::Webp) => "WebP",
                Some(Codec::Avif) => "AVIF",
            })
            .collect()
    }

    /// Output with a quality setting (not PNG, not lossless WebP).
    fn lossy(&self) -> bool {
        FORMATS[self.format] != Some(Codec::Png)
            && !(FORMATS[self.format] == Some(Codec::Webp) && self.lossless)
    }

    /// What Start will do (the number of copies), or why it cannot: the
    /// message for the footer's status. Ok exactly when Start runs.
    fn readiness(&mut self) -> Result<usize, Msg> {
        if self.files.is_empty() {
            return Err(Msg::BatchNeedFiles);
        }
        if self.files.reading() {
            return Err(Msg::BatchWaitReading);
        }
        if !self.files.ready() {
            return Err(Msg::BatchNeedReadable);
        }
        if self.size().is_none() {
            return Err(Msg::BatchNeedSize);
        }
        if naming::validate(&self.pattern).is_err() {
            return Err(Msg::BatchNeedPattern);
        }
        // Size and pattern are fine: only the folder is left.
        let Some(job) = self.job() else {
            return Err(Msg::BatchNeedFolder);
        };
        // A name that cannot be made comes from the pattern.
        self.files
            .items(job)
            .map(|items| items.len())
            .ok_or(Msg::BatchNeedPattern)
    }

    /// Draws the dialog centred in `client` (physical pixels) and
    /// remembers where its controls are.
    pub fn draw(&mut self, canvas: &mut impl Canvas, client: (f64, f64), scale: f64) {
        let mut hits = Vec::new();
        let mut p = Painter { canvas, scale };
        // Dim the viewer behind the modal panel.
        p.fill(Rect::new(0.0, 0.0, client.0, client.1), 0.0, widgets::SCRIM);
        let (pad, margin) = (p.px(PAD), p.px(MARGIN));
        let (tight, gap, group) = (p.px(GAP_TIGHT), p.px(GAP_ROW), p.px(GAP_GROUP));
        let (head, ctrl, check, row) = (p.px(HEAD_H), p.px(CONTROL_H), p.px(CHECK_ROW), p.px(ROW));
        let w = p.px(WIDTH).min(client.0 - 2.0 * margin);
        let inner = w - 2.0 * pad;
        // The header above, the footer below: in every mode. Editing puts
        // the list right under its header, a run its headline.
        let top = pad + head + tight;
        let bottom = pad + p.px(BUTTON_H) + pad;
        let inset = 2.0 * p.px(batch_files::INSET_V);
        let edit = self.run.is_none().then(|| Edit::measure(&mut p, inner));
        let (h, list_h) = match (&self.run, &edit) {
            (None, Some(e)) => {
                // The list takes the whole rows the window leaves.
                let fixed = top + group + e.body + bottom;
                let fit = ((client.1 - 2.0 * margin - fixed - inset) / row).floor();
                let rows = self
                    .files
                    .len()
                    .max(LIST_MIN_ROWS)
                    .min(fit.max(1.0) as usize);
                let list_h = rows as f64 * row + inset;
                (fixed + list_h, list_h)
            }
            (Some(run), _) => {
                // Title · headline and bar · error list · buttons.
                let status = group + head + gap + p.px(BAR_H);
                let errors = group + head + tight + row * run.error_rows() as f64 + inset;
                (pad + head + status + errors + bottom, 0.0)
            }
            (None, None) => (top + bottom, 0.0),
        };
        let panel = Rect::new(
            ((client.0 - w) / 2.0).round(),
            ((client.1 - h) / 2.0).max(margin).round(),
            w,
            h,
        );
        p.fill(panel, RADIUS, widgets::PANEL);
        p.outline(panel, 1.0, RADIUS, widgets::BORDER);
        let x0 = panel.x + pad;
        let mut y = panel.y + pad;
        let hover = self.hover;
        // The title, and while editing on the same line at the right, the
        // file count and "Remove all" on the title's baseline: the header
        // of the list right under it.
        let mut title_w = inner;
        let title_text = tr(Msg::BatchTitle);
        let (_, title_h) = p.text_size(title_text, TEXT_TITLE);
        let title_top = y + (head - title_h) / 2.0;
        if self.run.is_none() {
            let on_baseline = |p: &mut Painter<_>, text: &str, x: f64, color: Rgba| {
                let (_, h) = p.text_size(text, TEXT_BODY);
                let top = title_top + ASCENT * (title_h - h);
                p.text(text, TEXT_BODY, x, top, color);
            };
            let mut right = x0 + inner;
            if !self.files.is_empty() {
                let clear = tr(Msg::BatchClear);
                let (cw, _) = p.text_size(clear, TEXT_BODY);
                let cr = Rect::new(right - cw - gap, y, cw + gap, head);
                let color = if hover == Some(Hit::Clear) {
                    widgets::FG
                } else {
                    widgets::ACCENT
                };
                on_baseline(&mut p, clear, right - cw, color);
                hits.push((cr, Hit::Clear));
                right -= cw + group;
            }
            let count = trp(PluralMsg::ImagesCount, self.files.len() as u64, &[]);
            let (nw, _) = p.text_size(&count, TEXT_BODY);
            on_baseline(&mut p, &count, right - nw, widgets::DIM);
            title_w = (right - nw - group - x0).max(0.0);
        }
        let title = p.elide_end(title_text, TEXT_TITLE, title_w);
        p.text(&title, TEXT_TITLE, x0, title_top, widgets::FG);
        y += head;
        if self.run.is_none() {
            y += tight;
        }

        let footer = Rect::new(
            x0,
            panel.bottom() - pad - p.px(BUTTON_H),
            inner,
            p.px(BUTTON_H),
        );
        match (&self.run, edit) {
            (None, Some(e)) => {
                let hover_remove = match hover {
                    Some(Hit::Remove(i)) => Some(i),
                    _ => None,
                };
                let job = self.job();
                let list = Rect::new(x0, y, inner, list_h);
                for (r, i) in self.files.draw(&mut p, list, job, hover_remove) {
                    hits.push((r, Hit::Remove(i)));
                }
                y += list_h + group;
                // Every control starts at the content edge `cx`; the
                // second column (`col2`) holds the switches.
                let cx = x0 + e.gutter;
                let cw = inner - e.gutter;
                let col2 = cx + (cw / 2.0).round();
                let check_dy = ((ctrl - check) / 2.0).round();
                let label = |p: &mut Painter<_>, msg: Msg, y: f64, h: f64| {
                    let r = Rect::new(x0, y, e.gutter, h);
                    p.text_in(tr(msg), TEXT_BODY, &r, 0.0, widgets::DIM);
                };

                // Size: the mode, then the value(s).
                label(&mut p, Msg::BatchSize, y, ctrl);
                let modes: Vec<&str> = MODES.iter().map(|m| tr(*m)).collect();
                let hm = match hover {
                    Some(Hit::Mode(i)) => Some(i),
                    _ => None,
                };
                let rects = p.segmented(&modes, self.mode, hm, cx, y, cw);
                y = rects.last().map_or(y + ctrl, |r| r.bottom()) + gap;
                hits.extend(
                    rects
                        .into_iter()
                        .enumerate()
                        .map(|(i, r)| (r, Hit::Mode(i))),
                );
                let fw = p.px(FIELD_W);
                let f1 = Rect::new(cx, y, fw, ctrl);
                p.field(
                    &self.first,
                    self.focus == Some(Field::First),
                    Self::number(&self.first).is_none(),
                    f1,
                );
                hits.push((f1, Hit::Field(Field::First)));
                let mut x = f1.right() + gap;
                if self.two_values() {
                    let (tw, _) = p.text_size("×", TEXT_BODY);
                    let times = Rect::new(x, y, tw, ctrl);
                    p.text_in("×", TEXT_BODY, &times, 0.0, widgets::DIM);
                    x += tw + gap;
                    let f2 = Rect::new(x, y, fw, ctrl);
                    p.field(
                        &self.second,
                        self.focus == Some(Field::Second),
                        Self::number(&self.second).is_none(),
                        f2,
                    );
                    hits.push((f2, Hit::Field(Field::Second)));
                    x = f2.right() + gap;
                }
                let unit = if self.mode == 5 { "%" } else { "px" };
                let (uw, _) = p.text_size(unit, TEXT_BODY);
                p.text_in(
                    unit,
                    TEXT_BODY,
                    &Rect::new(x, y, uw, ctrl),
                    0.0,
                    widgets::DIM,
                );
                let no_enlarge = tr(Msg::BatchNoEnlarge);
                let cb = p.checkbox(no_enlarge, self.no_enlarge, true, col2, y + check_dy);
                hits.push((cb, Hit::NoEnlarge));
                y += ctrl + group;

                // Output: format, quality, options, name, folder.
                label(&mut p, Msg::BatchFormat, y, ctrl);
                let formats = Self::format_labels();
                let hf = match hover {
                    Some(Hit::Format(i)) => Some(i),
                    _ => None,
                };
                let rects = p.segmented(&formats, self.format, hf, cx, y, cw);
                y = rects.last().map_or(y + ctrl, |r| r.bottom()) + gap;
                hits.extend(
                    rects
                        .into_iter()
                        .enumerate()
                        .map(|(i, r)| (r, Hit::Format(i))),
                );

                // Quality, a row for every format: column 1 holds the value
                // (the slider and its number, or why there is none), column
                // 2 WebP's lossless switch, as on the size row.
                let value_w = p.px(VALUE_W);
                let right = col2 - group;
                let slider = Rect::new(cx, y, right - cx - gap - value_w, ctrl);
                label(&mut p, Msg::BatchQuality, y, ctrl);
                if self.lossy() {
                    p.slider((self.quality as f64 - 1.0) / 99.0, slider);
                    hits.push((slider, Hit::Slider));
                    // The number, right-aligned after the track.
                    let v = self.quality.to_string();
                    let (vw, _) = p.text_size(&v, TEXT_BODY);
                    let vr = Rect::new(right - vw, y, vw, ctrl);
                    p.text_in(&v, TEXT_BODY, &vr, 0.0, widgets::FG);
                } else {
                    let note = p.elide_end(tr(Msg::BatchNoQuality), TEXT_BODY, right - cx);
                    let nr = Rect::new(cx, y, right - cx, ctrl);
                    p.text_in(&note, TEXT_BODY, &nr, 0.0, widgets::DIM);
                }
                if FORMATS[self.format] == Some(Codec::Webp) {
                    let lossless = tr(Msg::BatchLossless);
                    let cb = p.checkbox(lossless, self.lossless, true, col2, y + check_dy);
                    hits.push((cb, Hit::Lossless));
                }
                y += ctrl + gap;
                self.slider = slider;

                label(&mut p, Msg::BatchOptions, y, check);
                let cb = p.checkbox(tr(Msg::BatchSharpen), self.sharpen, true, cx, y);
                hits.push((cb, Hit::Sharpen));
                let cb = p.checkbox(tr(Msg::BatchKeepIcc), self.keep_icc, true, col2, y);
                hits.push((cb, Hit::KeepIcc));
                y += check + gap;
                let cb = p.checkbox(tr(Msg::BatchKeepMeta), self.keep_meta, true, cx, y);
                hits.push((cb, Hit::KeepMeta));
                let strip = tr(Msg::BatchStripGps);
                let cb = p.checkbox(strip, self.strip_gps, self.keep_meta, col2, y);
                hits.push((cb, Hit::StripGps));
                y += check + gap;

                label(&mut p, Msg::BatchName, y, ctrl);
                let pf = Rect::new(cx, y, cw, ctrl);
                p.field(
                    &self.pattern,
                    self.focus == Some(Field::Pattern),
                    naming::validate(&self.pattern).is_err(),
                    pf,
                );
                hits.push((pf, Hit::Field(Field::Pattern)));
                y += ctrl + tight;
                let hint = Rect::new(cx, y, cw, p.px(HINT_H));
                p.text_in(PLACEHOLDERS, TEXT_SMALL, &hint, 0.0, widgets::DIM);
                y += hint.h + gap;

                label(&mut p, Msg::BatchFolder, y, ctrl);
                let bw = p.px(PICK_W);
                let ff = Rect::new(cx, y, cw - gap - bw, ctrl);
                p.field(
                    &self.folder,
                    self.focus == Some(Field::Folder),
                    self.folder.trim().is_empty(),
                    ff,
                );
                hits.push((ff, Hit::Field(Field::Folder)));
                let pick = Rect::new(ff.right() + gap, y, bw, ctrl);
                p.button("…", false, hover == Some(Hit::PickFolder), true, pick);
                hits.push((pick, Hit::PickFolder));

                // Footer: Start, the one primary action, rightmost; left
                // of the buttons what it will do, or why it cannot.
                let ready = self.readiness();
                let (start, cancel) = (tr(Msg::BatchStart), tr(Msg::BatchCancel));
                let (sw, cw) = (p.button_width(start), p.button_width(cancel));
                let sr = Rect::new(footer.right() - sw, footer.y, sw, footer.h);
                let cr = Rect::new(sr.x - gap - cw, footer.y, cw, footer.h);
                p.button(cancel, false, hover == Some(Hit::Cancel), true, cr);
                p.button(start, true, hover == Some(Hit::Start), ready.is_ok(), sr);
                hits.push((cr, Hit::Cancel));
                hits.push((sr, Hit::Start));
                let room = cr.x - group - x0;
                let (text, color) = match ready {
                    Ok(n) => (copies(&mut p, n, self.folder.trim(), room), widgets::DIM),
                    Err(m @ (Msg::BatchNeedFiles | Msg::BatchWaitReading)) => {
                        (tr(m).to_string(), widgets::DIM)
                    }
                    Err(m) => (tr(m).to_string(), widgets::DANGER),
                };
                status(&mut p, &text, color, &footer, room);
            }
            (Some(run), _) => {
                // The headline says how far it is (how it went), the bar
                // under it shows the same; the details follow.
                y += group;
                let headline = p.elide_end(&run.headline(), TEXT_TITLE, inner);
                let hr = Rect::new(x0, y, inner, head);
                p.text_in(&headline, TEXT_TITLE, &hr, 0.0, widgets::FG);
                y += head + gap;
                let bar = Rect::new(x0, y, inner, p.px(BAR_H));
                let handled = run.done + run.failed;
                p.progress(handled as f64 / run.total.max(1) as f64, bar);
                y += bar.h + group;

                // The failures: a labelled list of fixed height, with a
                // note when there are more than it shows.
                let er = Rect::new(x0, y, inner, head);
                p.text_in(tr(Msg::BatchErrors), TEXT_BODY, &er, 0.0, widgets::DIM);
                if run.failed > run.errors.len() {
                    let n = run.errors.len();
                    let more = trf(Msg::BatchErrorsFirst, &[("n", &n), ("failed", &run.failed)]);
                    let (mw, _) = p.text_size(&more, TEXT_BODY);
                    let mr = Rect::new(er.right() - mw, y, mw, head);
                    p.text_in(&more, TEXT_BODY, &mr, 0.0, widgets::DIM);
                }
                y += head + tight;
                let list_h = row * run.error_rows() as f64 + inset;
                let list = Rect::new(x0, y, inner, list_h);
                let radius = batch_files::FRAME_RADIUS;
                p.edge_outline(list, 1.0, radius, widgets::BORDER);
                if run.errors.is_empty() {
                    p.text_centred(tr(Msg::BatchNoErrors), TEXT_BODY, &list, widgets::DIM);
                }
                let inset_h = p.px(batch_files::INSET_H);
                let line_w = inner - 2.0 * inset_h;
                let mut ly = y + inset / 2.0;
                for (name, error) in &run.errors {
                    let line = error_line(&mut p, name, error, line_w);
                    let r = Rect::new(x0 + inset_h, ly, line_w, row);
                    p.text_in(&line, TEXT_BODY, &r, 0.0, widgets::DANGER);
                    ly += row;
                }

                // The footer holds only the buttons.
                if run.summary.is_some() {
                    let (close, open) = (tr(Msg::BatchClose), tr(Msg::BatchOpenFolder));
                    let (clw, ow) = (p.button_width(close), p.button_width(open));
                    let clr = Rect::new(footer.right() - clw, footer.y, clw, footer.h);
                    let or = Rect::new(clr.x - gap - ow, footer.y, ow, footer.h);
                    let has_output = run.output.is_some();
                    p.button(open, false, hover == Some(Hit::OpenFolder), has_output, or);
                    p.button(close, true, hover == Some(Hit::Close), true, clr);
                    if has_output {
                        hits.push((or, Hit::OpenFolder));
                    }
                    hits.push((clr, Hit::Close));
                } else {
                    let cancel = tr(Msg::BatchCancel);
                    let cw = p.button_width(cancel);
                    let cr = Rect::new(footer.right() - cw, footer.y, cw, footer.h);
                    let cancelling = run.cancel.load(Ordering::Relaxed);
                    p.button(cancel, false, hover == Some(Hit::Cancel), !cancelling, cr);
                    hits.push((cr, Hit::Cancel));
                }
            }
            (None, None) => {}
        }
        self.hits = hits;
    }
}

/// The edit mode's measured parts.
struct Edit {
    /// Width of the label gutter.
    gutter: f64,
    /// Height of everything between the list and the footer's gap.
    body: f64,
}

impl Edit {
    fn measure(p: &mut Painter<impl Canvas>, inner: f64) -> Edit {
        let widest = LABELS
            .iter()
            .map(|m| p.text_size(tr(*m), TEXT_BODY).0)
            .fold(0.0, f64::max);
        let step = p.px(GAP_ROW);
        let gutter = (((widest + p.px(GAP_GROUP)) / step).ceil() * step)
            .clamp(p.px(GUTTER_MIN), p.px(GUTTER_MAX));
        let cw = inner - gutter;
        let modes: Vec<&str> = MODES.iter().map(|m| tr(*m)).collect();
        let segments_h = |p: &mut Painter<_>, labels: &[&str]| {
            p.segment_rects(labels, 0.0, 0.0, cw)
                .last()
                .map_or(0.0, |r| r.bottom())
        };
        let modes_h = segments_h(p, &modes);
        let formats_h = segments_h(p, &BatchDialog::format_labels());
        let (tight, gap, group) = (p.px(GAP_TIGHT), p.px(GAP_ROW), p.px(GAP_GROUP));
        let (ctrl, check) = (p.px(CONTROL_H), p.px(CHECK_ROW));
        // Size: modes, values. Output: format, quality, two option rows,
        // name and hint, folder. As drawn in `BatchDialog::draw`.
        let size = modes_h + gap + ctrl;
        let output = formats_h
            + gap
            + ctrl
            + gap
            + 2.0 * (check + gap)
            + ctrl
            + tight
            + p.px(HINT_H)
            + gap
            + ctrl;
        Edit {
            gutter,
            body: size + group + output,
        }
    }
}

/// A failure as "name: reason" in `w`: a long name is shortened in the
/// middle so the reason stays whole, else the line is cut at its end.
fn error_line(p: &mut Painter<impl Canvas>, name: &str, error: &str, w: f64) -> String {
    let full = format!("{name}: {error}");
    if p.text_size(&full, TEXT_BODY).0 <= w {
        return full;
    }
    let reason = format!(": {error}");
    let room = w - p.text_size(&reason, TEXT_BODY).0;
    if room >= w / 3.0 {
        p.elide_middle(name, TEXT_BODY, room) + &reason
    } else {
        p.elide_end(&full, TEXT_BODY, w)
    }
}

/// "{n} copies into {folder}" in `w`: a long folder is shortened in the
/// middle, so the count and the folder's end stay readable.
fn copies(p: &mut Painter<impl Canvas>, n: usize, folder: &str, w: f64) -> String {
    let text = |folder: &str| trp(PluralMsg::BatchCopies, n as u64, &[("folder", &folder)]);
    let full = text(folder);
    let over = p.text_size(&full, TEXT_BODY).0 - w;
    if over <= 0.0 {
        return full;
    }
    let room = p.text_size(folder, TEXT_BODY).0 - over;
    let short = p.elide_middle(folder, TEXT_BODY, room.max(0.0));
    p.elide_end(&text(&short), TEXT_BODY, w)
}

/// The footer's status: one line left in `footer`, at most `w` wide.
fn status(p: &mut Painter<impl Canvas>, text: &str, color: Rgba, footer: &Rect, w: f64) {
    let line = p.elide_end(text, TEXT_BODY, w.max(0.0));
    p.text_in(&line, TEXT_BODY, footer, 0.0, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn dialog() -> BatchDialog {
        let mut d = BatchDialog::new();
        d.files.push_ready(
            "/photos/IMG_0001.HEIC".into(),
            crate::batch_files::tests::info(),
        );
        d
    }

    fn new_name(d: &mut BatchDialog) -> String {
        let job = d.job();
        d.files.texts(job)[0].1.clone()
    }

    #[test]
    fn edits_make_the_job() {
        let mut d = dialog();
        let job = d.job().unwrap();
        assert_eq!(job.size, Size::LongEdge(1920));
        assert_eq!(job.output, Output::Same);
        assert!(!job.enlarge && job.keep_exif && job.keep_gps);
        d.mode = 4;
        d.focus = Some(Field::First);
        d.first.clear();
        d.text_input("1x08a0");
        assert_eq!(d.first, "1080");
        assert_eq!(d.job().unwrap().size, Size::Fill { w: 1080, h: 1080 });
        d.format = 4;
        d.strip_gps = true;
        d.quality = 60;
        let job = d.job().unwrap();
        assert_eq!(job.output, Output::Codec(Codec::Avif));
        assert_eq!(job.quality.avif, 60);
        assert!(!job.keep_gps);
        d.first.clear();
        assert!(d.job().is_none(), "no size");
    }

    #[test]
    fn new_names_follow_the_pattern() {
        let mut d = dialog();
        assert_eq!(new_name(&mut d), "IMG_0001.jpg");
        d.pattern = "{name}_{w}x{h}".into();
        d.format = 3;
        assert_eq!(new_name(&mut d), "IMG_0001_1920x1440.webp", "the real size");
        d.pattern = "{date:DD.MM.YYYY_hh-mm}_{n:3}".into();
        assert_eq!(
            new_name(&mut d),
            "17.05.2024_09-04_001.webp",
            "the file's date"
        );
        d.pattern = "{bad".into();
        assert_eq!(new_name(&mut d), "–");
        assert!(d.job().is_none());
    }

    #[test]
    fn dropped_files_wait_for_their_headers() {
        let mut d = BatchDialog::new();
        assert_eq!(d.key(Key::Enter), Outcome::Unchanged, "nothing to do");
        let a = std::env::temp_dir().join(format!("blinzel-dialog-{}.jpg", std::process::id()));
        std::fs::write(&a, b"x").unwrap();
        assert_eq!(d.add(vec![a.clone()]), Outcome::Inspect(vec![a.clone()]));
        assert_eq!(d.key(Key::Enter), Outcome::Unchanged, "header still read");

        // A readable file next to one still being read: no start either.
        let mut d = dialog();
        assert!(matches!(d.start(), Outcome::Start(..)), "readable alone");
        assert_eq!(d.add(vec![a.clone()]), Outcome::Inspect(vec![a.clone()]));
        assert_eq!(
            d.key(Key::Enter),
            Outcome::Unchanged,
            "one header still read"
        );
        assert!(d.run.is_none());
        std::fs::remove_file(&a).unwrap();
    }

    #[test]
    fn keys_and_tab_order() {
        let mut d = dialog();
        assert_eq!(d.tab(false), Outcome::Redraw);
        assert_eq!(d.focus, Some(Field::First));
        d.tab(false);
        assert_eq!(d.focus, Some(Field::Pattern), "one value: no second field");
        d.tab(true);
        assert_eq!(d.focus, Some(Field::First));
        d.key(Key::Backspace);
        assert_eq!(d.first, "192");
        match d.key(Key::Enter) {
            Outcome::Start(job, items) => {
                assert_eq!(job.size, Size::LongEdge(192));
                assert_eq!(
                    items[0].target.as_deref(),
                    Some(Path::new("/photos/resized/IMG_0001.jpg"))
                );
            }
            o => panic!("{o:?}"),
        }
        assert_eq!(d.key(Key::Escape), Outcome::Close);
    }

    #[test]
    fn progress_and_summary() {
        let mut d = dialog();
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        d.started(1, rx, cancel.clone());
        assert!(d.is_running());
        assert_eq!(d.add(vec!["/photos/x.jpg".into()]), Outcome::Unchanged);
        tx.send(RunMsg::Progress(Progress::Done {
            index: 0,
            source: "/photos/IMG_0001.HEIC".into(),
            output: "/photos/resized/IMG_0001.jpg".into(),
            bytes: 1,
        }))
        .unwrap();
        assert!(d.poll());
        assert_eq!(d.key(Key::Escape), Outcome::Redraw);
        assert!(cancel.load(Ordering::Relaxed), "escape cancels a run");
        tx.send(RunMsg::Finished(Summary {
            done: 1,
            failed: 0,
            skipped: 0,
        }))
        .unwrap();
        d.poll();
        assert!(!d.is_running());
        assert_eq!(
            d.run.as_ref().unwrap().output.as_deref(),
            Some(Path::new("/photos/resized"))
        );
    }

    /// `docs/specs/batch-resize.md` AC-25.
    #[test]
    fn start_needs_a_valid_size_pattern_folder_and_a_readable_file() {
        let mut d = dialog();
        d.focus = Some(Field::First);
        d.first.clear();
        d.text_input("1234567");
        assert_eq!(d.first, "123456", "six digits at most");
        let refused = |d: &mut BatchDialog, why: &str| {
            assert!(d.job().is_none(), "{why}");
            assert_eq!(d.key(Key::Enter), Outcome::Unchanged, "{why}");
            assert_eq!(new_name(d), "–", "{why}");
        };
        refused(&mut d, "above 100000");
        d.first = "100000".into();
        assert!(d.job().is_some(), "100000 is allowed");
        for size in ["", "0"] {
            d.first = size.into();
            refused(&mut d, "empty or zero");
        }
        d.first = "1920".into();
        d.mode = 3;
        d.second = "0".into();
        refused(&mut d, "second size zero");
        d.mode = 0;
        for bad in ["{name", "{size}", "", "web/{name}", "web\\{name}"] {
            d.pattern = bad.into();
            refused(&mut d, bad);
        }
        d.pattern = "{name}".into();
        d.folder = "  ".into();
        refused(&mut d, "no folder");
        d.folder = "resized".into();
        assert!(matches!(d.key(Key::Enter), Outcome::Start(..)));

        // Only an unreadable file: nothing to start.
        let mut d = BatchDialog::new();
        d.add(vec!["/photos/broken.jpg".into()]);
        let (tx, rx) = std::sync::mpsc::channel();
        d.inspecting(rx);
        tx.send(("/photos/broken.jpg".into(), Err("broken".into())))
            .unwrap();
        d.poll();
        assert!(d.job().is_some());
        assert_eq!(d.key(Key::Enter), Outcome::Unchanged, "no readable file");
    }

    /// A dialog with `n` readable files.
    fn dialog_with(n: usize) -> BatchDialog {
        let mut d = BatchDialog::new();
        for i in 0..n {
            d.files.push_ready(
                format!("/photos/IMG_{i:04}.jpg").into(),
                crate::batch_files::tests::info(),
            );
        }
        d
    }

    fn drawn(d: &mut BatchDialog, client: (f64, f64)) {
        let mut canvas = crate::testkit::FakeCanvas::default();
        d.draw(&mut canvas, client, 1.0);
    }

    /// The whole panel, footer included, fits the narrowest window, and
    /// the footer sits one padding under the last control: the measured
    /// height is the drawn one.
    #[test]
    fn the_panel_fits_the_window_and_its_content() {
        for client in [(960.0, 640.0), (1200.0, 800.0), (1600.0, 1067.0)] {
            let mut d = dialog_with(48);
            drawn(&mut d, client);
            let window = Rect::new(0.0, 0.0, client.0, client.1);
            for (r, h) in &d.hits {
                assert!(
                    r.x >= MARGIN
                        && r.y >= MARGIN
                        && r.right() <= window.right() - MARGIN
                        && r.bottom() <= window.bottom() - MARGIN,
                    "{h:?} at {r:?} outside {client:?}"
                );
            }
            let start = d.drawn_at(Hit::Start).unwrap();
            let pick = d.drawn_at(Hit::PickFolder).unwrap();
            assert_eq!(start.y - pick.bottom(), PAD, "{client:?}");
            assert_eq!(start.right(), d.drawn_at(Hit::Clear).unwrap().right());
            let cancel = d.drawn_at(Hit::Cancel).unwrap();
            assert!(cancel.right() < start.x, "Cancel left of Start");
            let rows = (0..48)
                .filter(|&i| d.drawn_at(Hit::Remove(i)).is_some())
                .count();
            assert!(rows >= 1, "{rows} rows");
            // The list takes the height that is left: the panel ends less
            // than a row above the window's margin.
            let bottom = start.bottom() + PAD;
            assert!(
                bottom > client.1 - MARGIN - ROW,
                "{client:?}: panel ends at {bottom}, {rows} rows"
            );
            if client.1 > 1000.0 {
                assert!(rows > 12, "a tall window gives the list its height: {rows}");
            }
        }
        // A short list keeps its least height; the panel shrinks with it.
        let mut d = dialog_with(3);
        drawn(&mut d, (1600.0, 1067.0));
        assert!(d.drawn_at(Hit::Remove(2)).is_some());
        let few = d.drawn_at(Hit::Start).unwrap().y;
        let mut d = dialog_with(48);
        drawn(&mut d, (1600.0, 1067.0));
        assert!(d.drawn_at(Hit::Start).unwrap().y > few);
    }

    /// Every control of the size and output groups starts at one content
    /// edge right of the label gutter.
    #[test]
    fn controls_share_the_content_edge() {
        let mut d = dialog_with(3);
        drawn(&mut d, (1200.0, 800.0));
        let edge = d.drawn_at(Hit::Mode(0)).unwrap().x;
        for hit in [
            Hit::Format(0),
            Hit::Field(Field::First),
            Hit::Slider,
            Hit::Sharpen,
            Hit::KeepMeta,
            Hit::Field(Field::Pattern),
            Hit::Field(Field::Folder),
        ] {
            assert_eq!(d.drawn_at(hit).unwrap().x, edge, "{hit:?}");
        }
        let col2 = d.drawn_at(Hit::KeepIcc).unwrap().x;
        assert_eq!(d.drawn_at(Hit::StripGps).unwrap().x, col2);
        assert_eq!(d.drawn_at(Hit::NoEnlarge).unwrap().x, col2);
        // WebP: the slider where it always is, its lossless switch in the
        // second column, like "don't enlarge" on the size row.
        let slider = d.drawn_at(Hit::Slider);
        d.format = 3;
        drawn(&mut d, (1200.0, 800.0));
        assert_eq!(d.drawn_at(Hit::Lossless).unwrap().x, col2);
        assert_eq!(d.drawn_at(Hit::Slider), slider);
        // The panel's inner left edge: its right edge less the inner width.
        let x0 = d.drawn_at(Hit::PickFolder).unwrap().right() - (WIDTH - 2.0 * PAD);
        assert!(
            edge - x0 >= GUTTER_MIN,
            "the label gutter left of the controls"
        );
    }

    /// Segments are as wide as their labels; options that do not fit one
    /// row wrap into rows of equal count, each row the control's width.
    #[test]
    fn segments_wrap_into_rows_of_equal_count() {
        let mut canvas = crate::testkit::FakeCanvas::default();
        let mut p = Painter {
            canvas: &mut canvas,
            scale: 1.0,
        };
        let labels = ["a long label", "b", "c", "another one", "e", "f"];
        let one = p.segment_rects(&labels, 10.0, 20.0, 600.0);
        assert!(one.iter().all(|r| r.y == 20.0), "fits one row");
        assert!(one[0].w > one[1].w, "width from the text");
        assert_eq!(one[5].right(), 610.0);
        let two = p.segment_rects(&labels, 10.0, 20.0, 200.0);
        let rows: Vec<f64> = two.iter().map(|r| r.y).collect();
        assert_eq!(rows, [20.0, 20.0, 20.0, 50.0, 50.0, 50.0]);
        assert_eq!((two[2].right(), two[5].right()), (210.0, 210.0));
        assert_eq!((two[0].x, two[3].x), (10.0, 10.0));
        // One column grid: the second row's dividers under the first's.
        assert_eq!((two[1].x, two[2].x), (two[4].x, two[5].x));
        assert!(two[0].w > two[1].w, "the widest label sets its column");
        // A short last row keeps the columns and ends at the edge.
        let five = p.segment_rects(&labels[..5], 10.0, 20.0, 200.0);
        assert_eq!(five[3].x, five[0].x);
        assert_eq!(five[4].x, five[1].x);
        assert_eq!(five[4].right(), 210.0);
    }

    /// The texts drawn left of Cancel and Start, with their colours.
    fn footer_status(d: &mut BatchDialog) -> (String, Rgba) {
        let mut canvas = crate::testkit::FakeCanvas::default();
        d.draw(&mut canvas, (1200.0, 800.0), 1.0);
        let start = d.drawn_at(Hit::Start).unwrap();
        let f = &canvas.frame;
        let i = (0..f.texts.len())
            .rev()
            .find(|&i| {
                let (x, y) = f.text_positions[i];
                y as f64 >= start.y && (y as f64) < start.bottom() && (x as f64) < start.x - 120.0
            })
            .expect("a status left of the buttons");
        (f.texts[i].clone(), f.text_colors[i])
    }

    /// The footer says what Start will do, or why it cannot, and Start
    /// is enabled exactly when it says what it will do.
    #[test]
    fn the_footer_states_what_start_will_do() {
        let mut d = dialog_with(3);
        assert_eq!(d.readiness(), Ok(3));
        let (text, color) = footer_status(&mut d);
        assert_eq!(
            text,
            trp(PluralMsg::BatchCopies, 3, &[("folder", &"resized")])
        );
        assert_eq!(color, widgets::DIM);
        let blocked = |d: &mut BatchDialog, msg: Msg| {
            assert_eq!(d.readiness(), Err(msg));
            assert_eq!(d.key(Key::Enter), Outcome::Unchanged, "{msg:?}");
            let (text, color) = footer_status(d);
            assert_eq!(text, tr(msg));
            color
        };
        d.first.clear();
        assert_eq!(blocked(&mut d, Msg::BatchNeedSize), widgets::DANGER);
        d.first = "1920".into();
        d.pattern = "{bad".into();
        assert_eq!(blocked(&mut d, Msg::BatchNeedPattern), widgets::DANGER);
        d.pattern = "{name}".into();
        d.folder = " ".into();
        assert_eq!(blocked(&mut d, Msg::BatchNeedFolder), widgets::DANGER);
        d.folder = "resized".into();
        assert!(matches!(d.key(Key::Enter), Outcome::Start(..)));

        let mut d = BatchDialog::new();
        assert_eq!(blocked(&mut d, Msg::BatchNeedFiles), widgets::DIM);
        d.add(vec!["/photos/broken.jpg".into()]);
        assert_eq!(blocked(&mut d, Msg::BatchWaitReading), widgets::DIM);
        let (tx, rx) = std::sync::mpsc::channel();
        d.inspecting(rx);
        tx.send(("/photos/broken.jpg".into(), Err("broken".into())))
            .unwrap();
        d.poll();
        assert_eq!(blocked(&mut d, Msg::BatchNeedReadable), widgets::DANGER);
    }

    /// A long folder is shortened in the middle; the count stays.
    #[test]
    fn a_long_folder_keeps_the_count_and_its_end() {
        let mut canvas = crate::testkit::FakeCanvas::default();
        let mut p = Painter {
            canvas: &mut canvas,
            scale: 1.0,
        };
        let folder = "/home/someone/Pictures/2026/Summer holiday/exports/small";
        let line = copies(&mut p, 47, folder, 300.0);
        assert!(p.text_size(&line, TEXT_BODY).0 <= 300.0, "{line}");
        assert!(line.starts_with("47 "), "{line}");
        assert!(line.contains("…") && line.contains("small"), "{line}");
    }

    /// The slider is there only for lossy output; PNG and lossless WebP
    /// say why there is none in its place. The quality row stays, so no
    /// format moves a control or changes the list's height.
    #[test]
    fn the_slider_is_there_only_for_lossy_output_and_nothing_moves() {
        let mut d = dialog_with(48);
        let (quality, note) = (tr(Msg::BatchQuality), tr(Msg::BatchNoQuality));
        let count = |d: &mut BatchDialog, text: &str| {
            let mut canvas = crate::testkit::FakeCanvas::default();
            d.draw(&mut canvas, (960.0, 640.0), 1.0);
            canvas.frame.texts.iter().filter(|t| *t == text).count()
        };
        let still = |d: &BatchDialog| {
            let rows = (0..48).filter(|&i| d.drawn_at(Hit::Remove(i)).is_some());
            (
                rows.count(),
                d.drawn_at(Hit::Format(3)),
                d.drawn_at(Hit::Sharpen),
                d.drawn_at(Hit::Start),
            )
        };
        assert_eq!((count(&mut d, quality), count(&mut d, note)), (1, 0));
        assert!(d.drawn_at(Hit::Slider).is_some() && d.drawn_at(Hit::Lossless).is_none());
        let jpeg = still(&d);
        d.format = 2;
        assert_eq!((count(&mut d, quality), count(&mut d, note)), (1, 1), "PNG");
        assert!(d.drawn_at(Hit::Slider).is_none() && d.drawn_at(Hit::Lossless).is_none());
        assert_eq!(still(&d), jpeg, "PNG");
        d.format = 3;
        assert_eq!(count(&mut d, note), 0, "lossy WebP");
        assert!(d.drawn_at(Hit::Slider).is_some() && d.drawn_at(Hit::Lossless).is_some());
        assert_eq!(still(&d), jpeg, "lossy WebP");
        d.lossless = true;
        assert_eq!(
            (count(&mut d, quality), count(&mut d, note)),
            (1, 1),
            "lossless WebP"
        );
        assert!(d.drawn_at(Hit::Slider).is_none() && d.drawn_at(Hit::Lossless).is_some());
        assert_eq!(still(&d), jpeg, "lossless WebP");
    }

    /// The title, the file count and "Remove all" share one header line,
    /// the count and the link at its right edge; the list starts right
    /// under it and gets the band a separate count row would take.
    #[test]
    fn the_header_holds_the_title_the_count_and_remove_all() {
        let mut d = dialog_with(48);
        let mut canvas = crate::testkit::FakeCanvas::default();
        d.draw(&mut canvas, (960.0, 640.0), 1.0);
        let at = |text: &str| {
            let f = &canvas.frame;
            let i = f.texts.iter().position(|t| t == text).unwrap();
            (f.text_positions[i], f.text_colors[i])
        };
        let title = at(tr(Msg::BatchTitle)).0;
        let count = trp(PluralMsg::ImagesCount, 48, &[]);
        let ((cx, cy), color) = at(&count);
        assert_eq!(color, widgets::DIM);
        let ((lx, ly), _) = at(tr(Msg::BatchClear));
        assert!(title.0 < cx && cx < lx, "title, count, Remove all");
        assert_eq!(cy, ly, "one baseline");
        let clear = d.drawn_at(Hit::Clear).unwrap();
        assert!(title.1 >= clear.y as f32 && title.1 < clear.bottom() as f32);
        assert_eq!(clear.right(), d.drawn_at(Hit::Start).unwrap().right());
        // The first row lies one tight gap plus the list's inset under the
        // header row.
        let first = d.drawn_at(Hit::Remove(0)).unwrap();
        let inset = batch_files::INSET_V;
        assert_eq!(first.y, clear.bottom() + GAP_TIGHT + inset);
        // 960 × 640 shows more than the list's least rows of the 48.
        let rows = (0..48)
            .filter(|&i| d.drawn_at(Hit::Remove(i)).is_some())
            .count();
        assert!(rows > LIST_MIN_ROWS, "{rows} rows");
    }

    /// A running batch keeps its panel still while errors arrive.
    #[test]
    fn errors_during_a_run_do_not_move_the_panel() {
        let mut d = dialog_with(10);
        let (tx, rx) = std::sync::mpsc::channel();
        d.started(10, rx, Arc::new(AtomicBool::new(false)));
        drawn(&mut d, (960.0, 640.0));
        let cancel = d.cancel_button().unwrap();
        for i in 0..6 {
            tx.send(RunMsg::Progress(Progress::Failed {
                index: i,
                source: format!("/photos/{i}.jpg").into(),
                error: "broken".into(),
            }))
            .unwrap();
            d.poll();
            drawn(&mut d, (960.0, 640.0));
            assert_eq!(d.cancel_button(), Some(cancel), "after {} errors", i + 1);
        }
    }

    /// A run reads top down: its count as a headline under the title,
    /// the bar, the errors; the footer holds only buttons. The sheet keeps
    /// its size when the run ends.
    #[test]
    fn a_run_leads_with_its_count_and_keeps_its_size() {
        let mut d = dialog_with(10);
        let (tx, rx) = std::sync::mpsc::channel();
        d.started(10, rx, Arc::new(AtomicBool::new(false)));
        let texts = |d: &mut BatchDialog| {
            let mut canvas = crate::testkit::FakeCanvas::default();
            d.draw(&mut canvas, (960.0, 640.0), 1.0);
            let f = canvas.frame;
            f.texts
                .into_iter()
                .zip(f.text_positions)
                .collect::<Vec<_>>()
        };
        let shown = texts(&mut d);
        let cancel = d.cancel_button().unwrap();
        let progress = trf(Msg::BatchProgress, &[("done", &0), ("total", &10)]);
        let (_, at) = shown
            .iter()
            .find(|(t, _)| *t == progress)
            .expect("the count");
        let (_, title) = shown
            .iter()
            .find(|(t, _)| t == tr(Msg::BatchTitle))
            .unwrap();
        assert!(at.1 > title.1 && (at.1 as f64) < title.1 as f64 + 2.0 * HEAD_H);
        assert!(shown.iter().any(|(t, _)| t == tr(Msg::BatchNoErrors)));
        let in_footer = |shown: &[(String, (f32, f32))], r: Rect| {
            shown
                .iter()
                .filter(|(_, (_, y))| *y as f64 >= r.y && (*y as f64) < r.bottom())
                .map(|(t, _)| t.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(in_footer(&shown, cancel), [tr(Msg::BatchCancel)]);

        for i in 0..6 {
            tx.send(RunMsg::Progress(Progress::Failed {
                index: i,
                source: format!("/photos/{i}.jpg").into(),
                error: "broken".into(),
            }))
            .unwrap();
        }
        tx.send(RunMsg::Finished(Summary {
            done: 4,
            failed: 6,
            skipped: 0,
        }))
        .unwrap();
        d.poll();
        let shown = texts(&mut d);
        let close = d.drawn_at(Hit::Close).unwrap();
        assert_eq!(close.bottom(), cancel.bottom(), "the sheet keeps its size");
        let more = trf(Msg::BatchErrorsFirst, &[("n", &4), ("failed", &6)]);
        assert!(
            shown.iter().any(|(t, _)| *t == more),
            "a note on the hidden errors"
        );
        let errors = shown
            .iter()
            .filter(|(t, _)| t.ends_with(": broken"))
            .count();
        assert_eq!(errors, ERRORS_SHOWN);
        let footer = in_footer(&shown, close);
        assert_eq!(footer, [tr(Msg::BatchOpenFolder), tr(Msg::BatchClose)]);
    }

    /// A long failure keeps its reason whole and fits the panel.
    #[test]
    fn long_error_lines_keep_their_reason() {
        let mut canvas = crate::testkit::FakeCanvas::default();
        let mut p = Painter {
            canvas: &mut canvas,
            scale: 1.0,
        };
        let name = "Beschädigte Übertragung aus dem Familienarchiv Großeltern \
                    Weihnachten 1987 – Scan Teil 1.png";
        let error = "corrupt image: unexpected end of file";
        let line = error_line(&mut p, name, error, 592.0);
        assert!(line.ends_with("Teil 1.png: corrupt image: unexpected end of file"));
        assert!(line.starts_with("Beschädigte"), "{line}");
        assert!(p.text_size(&line, TEXT_BODY).0 <= 592.0);
        assert_eq!(
            error_line(&mut p, "a.jpg", "broken", 592.0),
            "a.jpg: broken"
        );
    }

    /// `docs/specs/batch-resize.md` AC-26.
    #[test]
    fn two_sizes_take_part_in_the_tab_order_and_enter_closes_a_finished_run() {
        let mut d = dialog();
        d.mode = 3;
        let mut order = Vec::new();
        for _ in 0..5 {
            d.tab(false);
            order.push(d.focus.unwrap());
        }
        use Field::*;
        assert_eq!(order, [First, Second, Pattern, Folder, First]);
        d.tab(true);
        assert_eq!(d.focus, Some(Folder), "Shift+Tab goes back");
        d.mode = 4;
        d.tab(false);
        d.tab(false);
        assert_eq!(d.focus, Some(Second), "fill box: two values too");

        let (tx, rx) = std::sync::mpsc::channel();
        d.started(1, rx, Arc::new(AtomicBool::new(false)));
        assert_eq!(d.key(Key::Enter), Outcome::Unchanged, "running");
        tx.send(RunMsg::Finished(Summary::default())).unwrap();
        d.poll();
        assert_eq!(d.key(Key::Enter), Outcome::Close, "after the run");
    }
}
