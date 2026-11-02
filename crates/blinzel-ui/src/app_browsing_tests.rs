//! App-level tests of browsing a folder: the listing, navigation, folder
//! watching and prefetch (`docs/specs/browsing.md`).

use super::*;
use crate::testkit::Harness;
use Key::*;

fn index(h: &Harness) -> Option<usize> {
    h.app.current.as_ref().and_then(|c| c.index)
}

// --------------------------------------------------------------- listing

/// AC-03.
#[test]
fn an_opened_file_shows_before_its_folder_is_listed_and_keys_wait_for_the_listing() {
    let mut h = Harness::new();
    for n in ["a.png", "B.png", "b.png", "c.png"] {
        h.write_png(n, 64, 48);
    }
    let _ = h.app.open(h.images.join("b.png"));
    h.pump_holding_listing();
    assert!(h.app.is_listing());
    assert_eq!(h.frame().images.len(), 1, "shown without the listing");
    assert_eq!(index(&h), None);
    for k in [Right, Left, Home, End] {
        h.app.key(k, false, false);
        assert_eq!(h.current_name().as_deref(), Some("b.png"), "{k:?}");
    }
    h.release_listing();
    h.pump();
    assert_eq!(h.names(), ["a.png", "B.png", "b.png", "c.png"]);
    assert_eq!(index(&h), Some(2), "the exact name, not B.png");
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("c.png"));
}

// ------------------------------------------------------------ navigation

/// AC-04, AC-05.
#[test]
fn navigation_keys_step_jump_wrap_and_pan_a_zoomed_image() {
    let mut h = Harness::new();
    for n in ["a.png", "b.png", "c.png", "d.png"] {
        h.write_png(n, 64, 48);
    }
    h.write_png("e.png", 1600, 1200);
    h.open("a.png");
    let steps = [
        (Right, "b"),
        (PageDown, "c"),
        (Space, "d"),
        (Down, "e"),
        (Right, "a"),
        (Left, "e"),
        (PageUp, "d"),
        (Backspace, "c"),
        (Up, "b"),
        (End, "e"),
        (Home, "a"),
    ];
    for (k, want) in steps {
        h.key(k);
        assert_eq!(h.current_name(), Some(format!("{want}.png")), "{k:?}");
    }
    // Right-to-left languages swap Right and Left only.
    assert_eq!(nav_cmd(Right, true), Some(Cmd::Prev));
    assert_eq!(nav_cmd(Left, true), Some(Cmd::Next));
    assert_eq!(nav_cmd(Right, false), Some(Cmd::Next));
    assert_eq!(nav_cmd(PageDown, true), Some(Cmd::Next));
    assert_eq!(nav_cmd(Up, true), Some(Cmd::Prev));

    // At 100 % e.png overflows the window: the arrows pan, Page Down
    // still pages.
    h.key(End);
    h.key(Digit(3));
    assert!(h.layout().unwrap().overflows);
    for k in [Right, Down, Left, Up] {
        let before = h.layout().unwrap().origin;
        h.key(k);
        assert_eq!(h.current_name().as_deref(), Some("e.png"), "{k:?}");
        assert_ne!(h.layout().unwrap().origin, before, "{k:?} pans");
    }
    h.key(PageDown);
    assert_eq!(h.current_name().as_deref(), Some("a.png"));
}

// ------------------------------------------------------- folder watching

/// What the shell does about 150 ms after a change in the folder.
fn folder_changed(h: &mut Harness) {
    h.app.on_dir_changed();
    h.pump();
}

/// AC-14.
#[test]
fn a_changed_folder_is_listed_again_and_the_current_image_stays() {
    let mut h = Harness::with_images(&["b.png", "a.png", "c.png"]);
    assert_eq!(h.host().watched, [h.images.clone()]);
    h.write_png("d.png", 64, 48);
    std::fs::rename(h.images.join("c.png"), h.images.join("e.png")).unwrap();
    std::fs::remove_file(h.images.join("a.png")).unwrap();
    folder_changed(&mut h);
    assert_eq!(h.names(), ["b.png", "d.png", "e.png"]);
    assert_eq!(h.current_name().as_deref(), Some("b.png"));
    assert_eq!(index(&h), Some(0));
    assert_eq!(h.frame().images.len(), 1);
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("d.png"), "added");
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("e.png"), "renamed");
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("b.png"), "a.png is gone");
}

/// AC-14: the current file deleted or renamed outside the viewer stays on
/// screen, and Next and Previous go on from its place in the folder.
#[test]
fn after_the_current_file_left_the_folder_next_and_previous_go_on_from_its_place() {
    for (key, want) in [(Right, "c.png"), (Left, "a.png")] {
        let mut h = Harness::with_images(&["a.png", "b.png", "c.png"]);
        h.open("b.png");
        std::fs::remove_file(h.images.join("b.png")).unwrap();
        folder_changed(&mut h);
        assert_eq!(h.names(), ["a.png", "c.png"]);
        assert_eq!(h.current_name().as_deref(), Some("b.png"));
        assert_eq!(h.frame().images.len(), 1, "still on screen");
        h.key(key);
        assert_eq!(h.current_name().as_deref(), Some(want), "{key:?}");
        drop(h);
    }
    // Renamed: from where the old name sorted.
    let mut h = Harness::with_images(&["a.png", "b.png", "c.png", "d.png"]);
    h.open("b.png");
    std::fs::rename(h.images.join("b.png"), h.images.join("z.png")).unwrap();
    folder_changed(&mut h);
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("c.png"));
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("d.png"));
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("z.png"));
}

/// AC-14: the current file rewritten in place (new size or write time)
/// loads anew.
#[test]
fn the_current_file_rewritten_in_place_is_loaded_anew() {
    let mut h = Harness::with_images(&["a.png", "b.png"]);
    let old = h.app.current.as_ref().unwrap().cache_id;
    h.write_png("a.png", 30, 20);
    folder_changed(&mut h);
    let cur = h.app.current.as_ref().unwrap();
    assert_ne!(cur.cache_id, old);
    assert_eq!(cur.index, Some(0));
    assert_eq!(h.frame().images[0].full_size, (30, 20));
}

/// AC-14: a change while the folder is still being listed is listed once
/// that listing is in.
#[test]
fn a_change_during_the_listing_is_listed_afterwards() {
    let mut h = Harness::new();
    h.write_png("a.png", 64, 48);
    h.write_png("b.png", 64, 48);
    let _ = h.app.open(h.images.join("a.png"));
    h.pump_holding_listing();
    h.write_png("c.png", 64, 48);
    h.app.on_dir_changed();
    h.release_listing();
    h.pump();
    assert_eq!(h.names(), ["a.png", "b.png", "c.png"]);
}

// -------------------------------------------------------------- prefetch

/// The nine images `a.png` … `i.png`.
const NINE: [&str; 9] = [
    "a.png", "b.png", "c.png", "d.png", "e.png", "f.png", "g.png", "h.png", "i.png",
];

/// What the App wants prepared: (file, priority, stage, with preview), in
/// order of priority, then name.
fn wanted(h: &Harness) -> Vec<(String, u32, Stage, bool)> {
    let mut w: Vec<_> = h
        .app
        .wanted_jobs()
        .into_iter()
        .map(|j| {
            let name = j.path.file_name().unwrap().to_string_lossy().into_owned();
            (name, j.priority, j.stage, j.with_preview)
        })
        .collect();
    w.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    w
}

fn job(name: &str, priority: u32, stage: Stage, preview: bool) -> (String, u32, Stage, bool) {
    (name.to_string(), priority, stage, preview)
}

/// `names` in a folder with `settings`; `open` is loaded, its folder just
/// listed and nothing else taken from the loader yet.
fn listed(settings: Settings, names: &[&str], open: &str) -> Harness {
    let mut h = Harness::with_settings(settings);
    for n in names {
        h.write_png(n, 64, 48);
    }
    let _ = h.app.open(h.images.join(open));
    h.pump_holding_listing();
    h.release_listing();
    h
}

/// AC-06: the order, the previews and the browsing direction.
#[test]
fn the_neighbours_are_wanted_in_browsing_direction_four_ahead_and_two_behind() {
    use Stage::Screen;
    let mut h = Harness::new();
    for n in NINE {
        h.write_png(n, 64, 48);
    }
    let _ = h.app.open(h.images.join("e.png"));
    assert_eq!(wanted(&h), [job("e.png", 0, Screen, true)], "current first");
    h.pump_holding_listing();
    h.release_listing();
    // e.png is prepared; nothing else is taken from the loader below.
    let forwards_from_e = [
        job("f.png", 1, Screen, true),
        job("d.png", 3, Screen, true),
        job("g.png", 3, Screen, false),
        job("h.png", 5, Screen, false),
        job("c.png", 6, Screen, false),
        job("i.png", 7, Screen, false),
    ];
    assert_eq!(wanted(&h), forwards_from_e);
    h.app.command(Cmd::Prev);
    assert_eq!(
        wanted(&h),
        [
            job("d.png", 0, Screen, true),
            job("c.png", 1, Screen, true),
            job("b.png", 3, Screen, false),
            job("a.png", 5, Screen, false),
            job("f.png", 6, Screen, false),
            job("i.png", 7, Screen, false),
        ],
        "backwards; e.png is prepared"
    );
    h.app.command(Cmd::Next);
    assert_eq!(wanted(&h), forwards_from_e, "forwards again");
    h.app.command(Cmd::Last);
    let names: Vec<_> = wanted(&h).into_iter().map(|j| (j.0, j.1)).collect();
    let last = [("i.png", 0), ("h.png", 1), ("a.png", 3), ("g.png", 3)];
    let last: Vec<_> = last
        .iter()
        .map(|(n, p)| (n.to_string(), *p))
        .chain([("f.png".into(), 5), ("b.png".into(), 6)])
        .collect();
    assert_eq!(names, last, "Last pages backwards; e.png is prepared");
    h.app.command(Cmd::First);
    let first: Vec<_> = wanted(&h).into_iter().map(|j| (j.0, j.1)).collect();
    let want: Vec<(String, u32)> = [
        ("a.png", 0),
        ("b.png", 1),
        ("c.png", 3),
        ("i.png", 3),
        ("d.png", 5),
        ("h.png", 6),
    ]
    .iter()
    .map(|(n, p)| (n.to_string(), *p))
    .collect();
    assert_eq!(first, want, "First pages forwards");
    h.app.command(Cmd::Last);
    let _ = h.app.open(h.images.join("c.png"));
    let open: Vec<_> = wanted(&h).into_iter().map(|j| (j.0, j.1)).collect();
    let want: Vec<(String, u32)> = [
        ("c.png", 0),
        ("d.png", 1),
        ("b.png", 3),
        ("f.png", 5),
        ("a.png", 6),
        ("g.png", 7),
    ]
    .iter()
    .map(|(n, p)| (n.to_string(), *p))
    .collect();
    assert_eq!(open, want, "opening pages forwards; e.png is prepared");
}

/// AC-06: what is read, in which order, and what is skipped.
#[test]
fn neighbours_are_read_most_urgent_first_once_and_failed_ones_are_skipped() {
    let mut h = Harness::new();
    for n in NINE {
        h.write_png(n, 64, 48);
    }
    h.write_file("h.png", b"not an image");
    h.open("e.png");
    let reads = h.view_reads();
    assert_eq!(reads[..2], ["e.png", "f.png"]);
    let mut tier: Vec<_> = reads[2..4].to_vec();
    tier.sort();
    assert_eq!(tier, ["d.png", "g.png"]);
    assert_eq!(reads[4..], ["h.png", "c.png", "i.png"]);
    let id = h.app.list().get(7).unwrap().cache_id();
    assert!(h.app.error_of(id).is_some());
    assert_eq!(wanted(&h), [], "prepared or failed");
    // Stepping on reads only what came into the window.
    h.key(Right);
    h.key(Right);
    assert_eq!(h.view_reads()[7..], ["a.png", "b.png"]);
    drop(h);

    // A folder smaller than the window: each other image once.
    let h = listed(Settings::default(), &["a.png", "b.png", "c.png"], "a.png");
    let w = wanted(&h);
    assert_eq!(
        w.iter().map(|j| j.0.as_str()).collect::<Vec<_>>(),
        ["b.png", "c.png"]
    );
}

/// AC-07.
#[test]
fn the_settings_file_sets_how_many_are_prepared_ahead_and_behind() {
    let s = Settings::parse("prefetch_ahead = 1\nprefetch_behind = many\n");
    let h = listed(s, &NINE, "e.png");
    let w: Vec<_> = wanted(&h).into_iter().map(|j| (j.0, j.1)).collect();
    assert_eq!(
        w,
        [
            ("f.png".into(), 1),
            ("d.png".into(), 3),
            ("c.png".into(), 6)
        ],
        "1 ahead, the default 2 behind"
    );
    drop(h);
    let s = Settings::parse("prefetch_ahead = 0\nprefetch_behind = 3\n");
    let h = listed(s, &NINE, "e.png");
    let w: Vec<_> = wanted(&h).into_iter().map(|j| (j.0, j.1)).collect();
    assert_eq!(
        w,
        [
            ("d.png".into(), 3),
            ("c.png".into(), 6),
            ("b.png".into(), 9)
        ]
    );
    drop(h);
    let s = Settings::parse("prefetch_ahead = 0\nprefetch_behind = 0\n");
    let h = listed(s, &NINE, "e.png");
    assert_eq!(wanted(&h), [], "off");
}

/// AC-10: zoomed beyond what the screen-sized stage resolves.
#[test]
fn zooming_past_the_screen_stage_prepares_the_full_resolution() {
    let mut h = Harness::new();
    h.write_jpeg("big.jpg", 3600, 2400);
    h.write_png("next.png", 64, 48);
    h.open("big.jpg");
    let screen = h.frame().images[0].size;
    assert!(screen.0 < 3600, "{screen:?}");
    // Image texels per screen pixel = zoom × 3600 / screen width.
    let per_texel = screen.0 as f64 / 3600.0;
    h.app.view.zoom = Some(1.04 * per_texel);
    h.app.after_zoom();
    assert_eq!(wanted(&h), []);
    h.app.view.zoom = Some(1.06 * per_texel);
    h.app.after_zoom();
    assert_eq!(wanted(&h), [job("big.jpg", 0, Stage::Full, true)]);
    h.pump();
    assert_eq!(h.frame().images[0].size, (3600, 2400), "replaces it");
}

/// AC-10: "100 %" prepares the current image and the neighbours at full
/// resolution.
#[test]
fn actual_size_prepares_everything_at_full_resolution() {
    let mut s = Settings::default();
    s.view.mode = ViewMode::ActualSize;
    let mut h = Harness::with_settings(s);
    for n in ["a.png", "b.png", "c.png"] {
        h.write_png(n, 64, 48);
    }
    let _ = h.app.open(h.images.join("b.png"));
    assert_eq!(wanted(&h), [job("b.png", 0, Stage::Full, true)]);
    h.pump_holding_listing();
    h.release_listing();
    let stages: Vec<_> = wanted(&h).into_iter().map(|j| (j.0, j.2)).collect();
    assert_eq!(
        stages,
        [("c.png".into(), Stage::Full), ("a.png".into(), Stage::Full)]
    );
}

/// AC-17 (the App's part; Windows reports the attributes).
#[test]
fn cloud_placeholders_are_not_read_ahead_but_load_when_navigated_to() {
    use blinzel_core::filelist::attr;
    let mut h = Harness::new();
    for n in ["a.png", "b.png", "c.png", "d.png", "e.png"] {
        h.write_png(n, 64, 48);
    }
    h.set_attributes("c.png", attr::OFFLINE);
    h.set_attributes("d.png", attr::RECALL_ON_DATA_ACCESS);
    h.set_attributes("e.png", attr::RECALL_ON_OPEN);
    h.open("b.png");
    assert_eq!(h.view_reads(), ["b.png", "a.png"]);
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("c.png"));
    assert_eq!(h.view_reads(), ["b.png", "a.png", "c.png"]);
    assert_eq!(h.frame().images.len(), 1, "c.png is shown");
}

// ------------------------------------------------------ reading, errors

/// A 3600 × 2400 JPEG named `name` followed by `tail`, which the probe
/// reports as its 960 × 640 embedded preview.
fn with_preview(h: &Harness, name: &str, tail: &[u8]) {
    let mut data = crate::testkit::jpeg(3600, 2400);
    let at = data.len();
    data.extend_from_slice(tail);
    h.set_preview(data.len(), at..data.len(), (960, 640));
    h.write_file(name, &data);
}

fn reads_of(h: &Harness, name: &str) -> usize {
    h.view_reads().iter().filter(|r| *r == name).count()
}

/// AC-09.
#[test]
fn one_read_gives_the_preview_and_the_screen_stage_and_the_current_image_gets_every_thread() {
    let mut h = Harness::new();
    with_preview(&h, "a.jpg", &crate::testkit::jpeg(960, 640));
    h.write_png("b.png", 64, 48);
    h.open("a.jpg");
    let id = h.app.current.as_ref().unwrap().cache_id;
    assert!(h.app.has_stage(id, Stage::Preview));
    assert!(h.app.has_stage(id, Stage::Screen));
    assert_eq!(reads_of(&h, "a.jpg"), 1);
    assert_eq!(reads_of(&h, "b.png"), 1);
    // The harness's loader decodes the visible image with 2 threads.
    let mut decodes = h.decodes();
    decodes.sort();
    assert_eq!(decodes, [((64, 48), 1), ((3600, 2400), 2)]);
    // Zoomed in: the full resolution needs a read of its own.
    h.app.view.zoom = Some(1.0);
    h.app.after_zoom();
    h.pump();
    assert!(h.app.has_stage(id, Stage::Full));
    assert_eq!(reads_of(&h, "a.jpg"), 2);
}

/// AC-20.
#[test]
fn a_broken_file_is_not_tried_again_until_it_changes_and_browsing_goes_on() {
    let mut h = Harness::new();
    h.write_png("a.png", 64, 48);
    h.write_file("b.png", b"not an image");
    h.write_png("c.png", 64, 48);
    h.open("a.png");
    h.key(Right);
    let id = h.app.current.as_ref().unwrap().cache_id;
    assert!(h.app.error_of(id).is_some());
    assert!(h.frame().images.is_empty(), "not the previous image");
    assert!(
        h.frame()
            .texts
            .iter()
            .any(|t| t.starts_with("Could not open “b.png”"))
    );
    h.key(Right);
    assert_eq!(h.current_name().as_deref(), Some("c.png"));
    assert_eq!(h.frame().images.len(), 1);
    h.key(Left);
    h.key(Left);
    assert_eq!(h.current_name().as_deref(), Some("a.png"));
    assert_eq!(reads_of(&h, "b.png"), 1, "not tried again");
    // Rewritten: a new size and write time, so it is tried again.
    h.write_png("b.png", 64, 48);
    folder_changed(&mut h);
    h.key(Right);
    assert_eq!(h.frame().images.len(), 1);
    assert_eq!(reads_of(&h, "b.png"), 2);
    drop(h);

    // A broken embedded preview: the image itself still shows.
    let mut h = Harness::new();
    with_preview(&h, "d.jpg", b"not a preview");
    h.open("d.jpg");
    let id = h.app.current.as_ref().unwrap().cache_id;
    assert_eq!(h.app.error_of(id), None);
    assert!(!h.app.has_stage(id, Stage::Preview));
    assert!(h.app.has_stage(id, Stage::Screen));
    assert_eq!(h.frame().images.len(), 1);
}
