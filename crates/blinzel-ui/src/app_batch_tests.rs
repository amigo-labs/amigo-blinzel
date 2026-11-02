//! App-level tests of the batch dialog: files dropped onto the window,
//! a run and its report (`docs/specs/batch-resize.md`).

use super::*;
use crate::testkit::Harness;
use Key::*;
use blinzel_core::batch::run::{Progress, Summary};
use blinzel_i18n::{Msg, tr};
use std::sync::atomic::AtomicBool;

fn centre(r: Rect) -> (f64, f64) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// The names in the dialog's list (`None`: it is closed).
fn listed(h: &Harness) -> Option<Vec<String>> {
    let images = h.images.clone();
    h.batch_files().map(|files| {
        files
            .iter()
            .map(|f| {
                let rel = f.strip_prefix(&images).unwrap_or(f);
                rel.to_string_lossy().replace('\\', "/")
            })
            .collect()
    })
}

/// The new names of the dialog's rows as last drawn (the text after
/// each arrow).
fn new_names(h: &Harness) -> Vec<String> {
    let t = &h.frame().texts;
    t.iter()
        .enumerate()
        .filter(|(_, s)| *s == "→")
        .filter_map(|(i, _)| t.get(i + 1).cloned())
        .collect()
}

/// Opens the dialog in a window large enough for its rows' names (the
/// harness's window fits its small images).
fn open_dialog(h: &mut Harness) {
    h.resize((1280, 900));
    h.ctrl(Letter('B'));
}

fn headers_read(h: &Harness) -> bool {
    let reading = tr(Msg::BatchReading);
    let names = new_names(h);
    !names.is_empty() && names.iter().all(|n| n != reading)
}

fn drawn(h: &Harness) -> Vec<String> {
    h.frame().texts.clone()
}

/// `docs/specs/batch-resize.md` AC-23: the shared path every shell hands
/// its drops to (`App::files_dropped`).
#[test]
fn drops_fill_the_open_dialog_and_otherwise_open_the_first_file() {
    let mut h = Harness::with_images(&["a.png", "b.png"]);
    std::fs::create_dir_all(h.images.join("trip/sub")).unwrap();
    for f in ["trip/img10.png", "trip/img9.png", "trip/sub/deep.png"] {
        h.write_png(f, 40, 30);
    }
    h.write_file("trip/notes.txt", b"text");
    h.write_file("c.txt", b"text");
    let at = |h: &Harness, f: &str| h.images.join(f);

    // Without the dialog the first dropped file is shown.
    let a = h.app.files_dropped(vec![at(&h, "b.png"), at(&h, "a.png")]);
    h.shell(a);
    assert_eq!(h.current_name().as_deref(), Some("b.png"));
    assert_eq!(h.batch_files(), None);

    open_dialog(&mut h);
    let drop = vec![
        at(&h, "c.txt"),
        at(&h, "trip"),
        at(&h, "a.png"),
        at(&h, "trip/img9.png"),
    ];
    let a = h.app.files_dropped(drop);
    assert_eq!(a, vec![], "nothing for the shell to do");
    assert_eq!(
        listed(&h).unwrap(),
        ["trip/img9.png", "trip/img10.png", "a.png"],
        "the folder's own images in natural order, each file once, no text files"
    );
    // The headers are read in the background: until they arrive the
    // rows say so.
    h.render();
    assert_eq!(new_names(&h), [tr(Msg::BatchReading); 3]);
    h.wait_for(headers_read);
    assert_eq!(new_names(&h), ["img9.png", "img10.png", "a.png"]);
    assert_eq!(
        h.current_name().as_deref(),
        Some("b.png"),
        "the viewer stays"
    );

    let a = h.app.files_dropped(vec![at(&h, "a.png"), at(&h, "trip")]);
    h.shell(a);
    assert_eq!(listed(&h).unwrap().len(), 3, "listed already");
    assert_eq!(h.current_name().as_deref(), Some("b.png"));
}

/// `docs/specs/batch-resize.md` AC-27 and AC-26 (Enter after a run).
#[test]
fn a_batch_reports_its_result_and_opens_the_output_folder() {
    let mut h = Harness::with_images(&["a.png", "b.png", "gone.png"]);
    open_dialog(&mut h);
    let files = ["a.png", "b.png", "gone.png"].map(|f| h.images.join(f));
    let a = h.app.files_dropped(files.to_vec());
    h.shell(a);
    h.wait_for(headers_read);
    // Planned, then gone before the run reads it.
    std::fs::remove_file(h.images.join("gone.png")).unwrap();
    h.key(Enter);
    h.wait_for(|h| h.app.dialog.as_ref().is_some_and(|d| !d.is_running()));
    h.render();
    let texts = drawn(&h);
    assert!(texts.iter().any(|t| t == "2 saved, 1 failed"), "{texts:?}");
    let errors: Vec<&String> = texts.iter().filter(|t| t.contains(": ")).collect();
    assert_eq!(errors.len(), 1, "{texts:?}");
    assert!(errors[0].starts_with("gone.png: "), "{errors:?}");
    let out = h.images.join("resized");
    assert!(out.join("a.png").is_file() && out.join("b.png").is_file());

    let button = h.app.dialog.as_ref().unwrap().open_folder_button();
    let a = h.click(centre(button.expect("an Open folder button")));
    assert_eq!(a, vec![Action::OpenFolder(out)]);
    h.key(Enter);
    assert!(
        h.app.dialog.is_none(),
        "Enter closes the dialog after a run"
    );
}

/// `docs/specs/batch-resize.md` AC-27: the texts while a batch runs and
/// after it, with at most four errors.
#[test]
fn progress_counts_and_the_first_four_errors_are_shown() {
    let mut h = Harness::with_images(&["a.png"]);
    open_dialog(&mut h);
    let (tx, rx) = std::sync::mpsc::channel();
    let d = h.app.dialog.as_mut().unwrap();
    d.started(6, rx, Arc::new(AtomicBool::new(false)));
    tx.send(RunMsg::Progress(Progress::Done {
        index: 0,
        source: "/photos/0.jpg".into(),
        output: "/photos/resized/0.jpg".into(),
        bytes: 1,
    }))
    .unwrap();
    h.app.on_batch();
    h.render();
    assert!(drawn(&h).iter().any(|t| t == "1 of 6"), "{:?}", drawn(&h));
    for i in 1..6 {
        tx.send(RunMsg::Progress(Progress::Failed {
            index: i,
            source: format!("/photos/{i}.jpg").into(),
            error: "broken".into(),
        }))
        .unwrap();
    }
    tx.send(RunMsg::Finished(Summary {
        done: 1,
        failed: 5,
        skipped: 0,
    }))
    .unwrap();
    h.app.on_batch();
    h.render();
    let texts = drawn(&h);
    assert!(texts.iter().any(|t| t == "1 saved, 5 failed"), "{texts:?}");
    let errors: Vec<&String> = texts.iter().filter(|t| t.ends_with(": broken")).collect();
    assert_eq!(
        errors,
        [
            "1.jpg: broken",
            "2.jpg: broken",
            "3.jpg: broken",
            "4.jpg: broken"
        ]
    );

    // Cancelled: the files not started are skipped.
    let (tx, rx) = std::sync::mpsc::channel();
    let d = h.app.dialog.as_mut().unwrap();
    d.started(3, rx, Arc::new(AtomicBool::new(true)));
    tx.send(RunMsg::Finished(Summary {
        done: 1,
        failed: 0,
        skipped: 2,
    }))
    .unwrap();
    h.app.on_batch();
    h.render();
    let texts = drawn(&h);
    assert!(
        texts.iter().any(|t| t == "Cancelled after 1 of 3"),
        "{texts:?}"
    );
}
