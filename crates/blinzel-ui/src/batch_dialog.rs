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
use crate::batch_files::{BatchFiles, Inspected, ROW, file_name};
use crate::widgets::{self, Painter};

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

struct Run {
    total: usize,
    done: usize,
    failed: usize,
    errors: Vec<String>,
    /// A folder that received output.
    output: Option<PathBuf>,
    cancel: Arc<AtomicBool>,
    rx: Receiver<RunMsg>,
    summary: Option<Summary>,
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
                    if run.errors.len() < 4 {
                        run.errors.push(format!("{}: {error}", file_name(&source)));
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

    /// Draws the dialog centred in `client` (physical pixels) and
    /// remembers where its controls are.
    pub fn draw(&mut self, canvas: &mut impl Canvas, client: (f64, f64), scale: f64) {
        let mut hits = Vec::new();
        let mut p = Painter { canvas, scale };
        // Dim the viewer behind the modal panel.
        p.fill(Rect::new(0.0, 0.0, client.0, client.1), 0.0, widgets::SCRIM);
        let pad = p.px(24.0);
        let w = p.px(640.0).min(client.0 - p.px(16.0));
        // Edit mode: title, file list, size, format, quality, options,
        // name with placeholders, folder, footer (see the layout below).
        // The list takes what the window leaves, within limits.
        let fixed = p.px(640.0);
        let list_h = (client.1 - p.px(16.0) - fixed).clamp(p.px(96.0), p.px(ROW * 9.0 + 12.0));
        let h = match &self.run {
            None => fixed + list_h,
            Some(r) if r.summary.is_some() => p.px(230.0) + p.px(20.0) * r.errors.len() as f64,
            Some(_) => p.px(200.0),
        };
        let panel = Rect::new(
            ((client.0 - w) / 2.0).round(),
            ((client.1 - h) / 2.0).max(p.px(8.0)).round(),
            w,
            h,
        );
        p.fill(panel, 12.0, widgets::PANEL);
        p.outline(panel, 1.0, 12.0, widgets::BORDER);
        let (x0, inner) = (panel.x + pad, w - 2.0 * pad);
        let mut y = panel.y + pad;
        let (_, th) = p.text(tr(Msg::BatchTitle), 18.0, x0, y, widgets::FG);
        y += th + p.px(4.0);
        let count = trp(PluralMsg::ImagesCount, self.files.len() as u64, &[]);
        let (_, sh) = p.text(&count, 13.0, x0, y, widgets::DIM);
        let hover = self.hover;
        if self.run.is_none() && !self.files.is_empty() {
            let clear = tr(Msg::BatchClear);
            let (cw, _) = p.text_size(clear, 13.0);
            let cr = Rect::new(x0 + inner - cw, y, cw, sh);
            let color = if hover == Some(Hit::Clear) {
                widgets::FG
            } else {
                widgets::ACCENT
            };
            p.text_in(clear, 13.0, &cr, 0.0, color);
            hits.push((cr, Hit::Clear));
        }
        y += sh + p.px(10.0);

        let footer_h = p.px(34.0);
        let footer_y = panel.bottom() - pad - footer_h;
        if self.run.is_none() {
            let hover_remove = match hover {
                Some(Hit::Remove(i)) => Some(i),
                _ => None,
            };
            let job = self.job();
            let list = Rect::new(x0, y, inner, list_h);
            for (r, i) in self.files.draw(&mut p, list, job, hover_remove) {
                hits.push((r, Hit::Remove(i)));
            }
            y += list_h + p.px(18.0);
        }
        match &self.run {
            None => {
                // Size.
                let (_, lh) = p.text(tr(Msg::BatchSize), 12.5, x0, y, widgets::DIM);
                y += lh + p.px(6.0);
                let modes: Vec<&str> = MODES.iter().map(|m| tr(*m)).collect();
                let hm = match hover {
                    Some(Hit::Mode(i)) => Some(i),
                    _ => None,
                };
                for (i, r) in p
                    .segmented(&modes, self.mode, hm, x0, y, inner)
                    .into_iter()
                    .enumerate()
                {
                    hits.push((r, Hit::Mode(i)));
                }
                y += p.px(30.0) + p.px(10.0);
                let fw = p.px(84.0);
                let fh = p.px(30.0);
                let f1 = Rect::new(x0, y, fw, fh);
                p.field(
                    &self.first,
                    self.focus == Some(Field::First),
                    Self::number(&self.first).is_none(),
                    f1,
                );
                hits.push((f1, Hit::Field(Field::First)));
                let mut x = f1.right() + p.px(8.0);
                if self.two_values() {
                    let (tw, _) = p.text_size("×", 14.0);
                    p.text_in("×", 14.0, &Rect::new(x, y, tw, fh), 0.0, widgets::DIM);
                    x += tw + p.px(8.0);
                    let f2 = Rect::new(x, y, fw, fh);
                    p.field(
                        &self.second,
                        self.focus == Some(Field::Second),
                        Self::number(&self.second).is_none(),
                        f2,
                    );
                    hits.push((f2, Hit::Field(Field::Second)));
                    x = f2.right() + p.px(8.0);
                }
                let unit = if self.mode == 5 { "%" } else { "px" };
                let (uw, _) = p.text_size(unit, 13.0);
                p.text_in(unit, 13.0, &Rect::new(x, y, uw, fh), 0.0, widgets::DIM);
                let cb = p.checkbox(
                    tr(Msg::BatchNoEnlarge),
                    self.no_enlarge,
                    true,
                    x + uw + p.px(24.0),
                    y + p.px(4.0),
                );
                hits.push((cb, Hit::NoEnlarge));
                y += fh + p.px(18.0);

                // Format and quality.
                let (_, lh) = p.text(tr(Msg::BatchFormat), 12.5, x0, y, widgets::DIM);
                y += lh + p.px(6.0);
                let formats: Vec<&str> = FORMATS
                    .iter()
                    .map(|f| match f {
                        None => tr(Msg::BatchSame),
                        Some(Codec::Jpeg) => "JPEG",
                        Some(Codec::Png) => "PNG",
                        Some(Codec::Webp) => "WebP",
                        Some(Codec::Avif) => "AVIF",
                    })
                    .collect();
                let hf = match hover {
                    Some(Hit::Format(i)) => Some(i),
                    _ => None,
                };
                for (i, r) in p
                    .segmented(&formats, self.format, hf, x0, y, inner)
                    .into_iter()
                    .enumerate()
                {
                    hits.push((r, Hit::Format(i)));
                }
                y += p.px(30.0) + p.px(12.0);
                let lossy = FORMATS[self.format] != Some(Codec::Png)
                    && !(FORMATS[self.format] == Some(Codec::Webp) && self.lossless);
                let ql = tr(Msg::BatchQuality);
                let (qw, _) = p.text_size(ql, 13.0);
                let row = Rect::new(x0, y, inner, p.px(22.0));
                p.text_in(
                    ql,
                    13.0,
                    &row,
                    0.0,
                    if lossy { widgets::FG } else { widgets::FAINT },
                );
                let sx = x0 + qw + p.px(14.0);
                let slider = Rect::new(sx, y, p.px(240.0), p.px(22.0));
                if lossy {
                    p.slider((self.quality as f64 - 1.0) / 99.0, slider);
                    hits.push((slider, Hit::Slider));
                    let v = self.quality.to_string();
                    p.text_in(
                        &v,
                        13.0,
                        &Rect::new(slider.right() + p.px(12.0), y, p.px(40.0), p.px(22.0)),
                        0.0,
                        widgets::FG,
                    );
                }
                self.slider = slider;
                if FORMATS[self.format] == Some(Codec::Webp) {
                    let cb = p.checkbox(
                        tr(Msg::BatchLossless),
                        self.lossless,
                        true,
                        slider.right() + p.px(64.0),
                        y,
                    );
                    hits.push((cb, Hit::Lossless));
                }
                y += p.px(22.0) + p.px(16.0);

                // Options in two columns.
                let col2 = x0 + inner / 2.0;
                let cb = p.checkbox(tr(Msg::BatchSharpen), self.sharpen, true, x0, y);
                hits.push((cb, Hit::Sharpen));
                let cb = p.checkbox(tr(Msg::BatchKeepIcc), self.keep_icc, true, col2, y);
                hits.push((cb, Hit::KeepIcc));
                y += p.px(28.0);
                let cb = p.checkbox(tr(Msg::BatchKeepMeta), self.keep_meta, true, x0, y);
                hits.push((cb, Hit::KeepMeta));
                let cb = p.checkbox(
                    tr(Msg::BatchStripGps),
                    self.strip_gps,
                    self.keep_meta,
                    col2,
                    y,
                );
                hits.push((cb, Hit::StripGps));
                y += p.px(22.0) + p.px(18.0);

                // Name and folder.
                let (_, lh) = p.text(tr(Msg::BatchName), 12.5, x0, y, widgets::DIM);
                y += lh + p.px(6.0);
                let pf = Rect::new(x0, y, inner, fh);
                p.field(
                    &self.pattern,
                    self.focus == Some(Field::Pattern),
                    naming::validate(&self.pattern).is_err(),
                    pf,
                );
                hits.push((pf, Hit::Field(Field::Pattern)));
                y += fh + p.px(4.0);
                let (_, eh) = p.text(PLACEHOLDERS, 12.0, x0, y, widgets::FAINT);
                y += eh + p.px(12.0);
                let (_, lh) = p.text(tr(Msg::BatchFolder), 12.5, x0, y, widgets::DIM);
                y += lh + p.px(6.0);
                let bw = p.px(40.0);
                let ff = Rect::new(x0, y, inner - bw - p.px(8.0), fh);
                p.field(
                    &self.folder,
                    self.focus == Some(Field::Folder),
                    self.folder.trim().is_empty(),
                    ff,
                );
                hits.push((ff, Hit::Field(Field::Folder)));
                let pick = Rect::new(ff.right() + p.px(8.0), y, bw, fh);
                p.button("…", false, hover == Some(Hit::PickFolder), true, pick);
                hits.push((pick, Hit::PickFolder));

                // Footer.
                let ok = self.job().is_some() && self.files.ready();
                let (start, cancel) = (tr(Msg::BatchStart), tr(Msg::BatchCancel));
                let (sw, cw) = (p.button_width(start), p.button_width(cancel));
                let sr = Rect::new(panel.right() - pad - sw, footer_y, sw, footer_h);
                let cr = Rect::new(sr.x - p.px(10.0) - cw, footer_y, cw, footer_h);
                p.button(cancel, false, hover == Some(Hit::Cancel), true, cr);
                p.button(start, true, hover == Some(Hit::Start), ok, sr);
                hits.push((cr, Hit::Cancel));
                hits.push((sr, Hit::Start));
            }
            Some(run) => {
                let finished = run.summary.is_some();
                let handled = run.done + run.failed;
                let bar = Rect::new(x0, y, inner, p.px(8.0));
                p.progress(handled as f64 / run.total.max(1) as f64, bar);
                y += p.px(8.0) + p.px(12.0);
                let status = match &run.summary {
                    Some(s) if s.skipped > 0 => trf(
                        Msg::BatchCancelled,
                        &[("done", &s.done), ("total", &run.total)],
                    ),
                    Some(s) => trf(Msg::BatchDone, &[("done", &s.done), ("failed", &s.failed)]),
                    None => trf(
                        Msg::BatchProgress,
                        &[("done", &handled), ("total", &run.total)],
                    ),
                };
                let (_, sh) = p.text(&status, 13.0, x0, y, widgets::FG);
                y += sh + p.px(8.0);
                for e in &run.errors {
                    let (_, eh) = p.text(e, 12.0, x0, y, widgets::DANGER);
                    y += eh + p.px(4.0);
                }
                if finished {
                    let (close, open) = (tr(Msg::BatchClose), tr(Msg::BatchOpenFolder));
                    let (clw, ow) = (p.button_width(close), p.button_width(open));
                    let clr = Rect::new(panel.right() - pad - clw, footer_y, clw, footer_h);
                    let or = Rect::new(clr.x - p.px(10.0) - ow, footer_y, ow, footer_h);
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
                    let cr = Rect::new(panel.right() - pad - cw, footer_y, cw, footer_h);
                    let cancelling = run.cancel.load(Ordering::Relaxed);
                    p.button(cancel, false, hover == Some(Hit::Cancel), !cancelling, cr);
                    hits.push((cr, Hit::Cancel));
                }
            }
        }
        self.hits = hits;
    }
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
}
