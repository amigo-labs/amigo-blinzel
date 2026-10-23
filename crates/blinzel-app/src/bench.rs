//! `blinzel.exe --bench <dir> [--warp] [--json out.json]`: headless timing
//! of the real pipeline (read → decode → upload → render), of prefetched
//! navigation at key-repeat speed, and of listing a 10k-file folder.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use blinzel_core::decode::Request;
use blinzel_core::filelist::FileList;
use blinzel_core::layout::{
    ViewOptions, ViewState, compute, content_in_stage, needed_decode_size, stage_to_client,
};
use blinzel_core::timing::{Stats, json_num, json_str};
use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

use crate::gfx::{Gfx, Offscreen, Renderer, Rgba, upload};
use crate::loader::{self, Job, Loader, Stage};
use crate::platform::{enumerate, io, threads};
use crate::wic;

const SCREEN: (u32, u32) = (2560, 1440);

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

pub fn run(dir: &Path, warp: bool, json: Option<&Path>) -> i32 {
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    match bench(dir, warp) {
        Ok(report) => {
            println!("{report}");
            if let Some(p) = json {
                let _ = std::fs::write(p, &report);
            }
            0
        }
        Err(e) => {
            eprintln!("bench failed: {e}");
            1
        }
    }
}

fn bench(dir: &Path, warp: bool) -> Result<String, String> {
    let t = Instant::now();
    let gfx = Gfx::create(warp).map_err(|e| e.message())?;
    let device_ms = ms(t);
    let mut renderer = Renderer::new(&gfx.device).map_err(|e| e.message())?;
    let target = Offscreen::new(&gfx.device, SCREEN).map_err(|e| e.message())?;

    let entries = enumerate::list_dir(dir).map_err(|e| e.to_string())?;
    let list = FileList::new(dir, entries);
    if list.is_empty() {
        return Err(format!("no images in {}", dir.display()));
    }

    // 1. Open: read → decode (screen size) → upload → render, per file.
    let mut files = Vec::new();
    for i in 0..list.len() {
        let path = list.path(i).unwrap();
        let t_all = Instant::now();
        let t = Instant::now();
        let data = io::read_file(&path).map_err(|e| e.to_string())?;
        let read_ms = ms(t);
        let t = Instant::now();
        let probe = match wic::probe_any(&data) {
            Ok(p) => p,
            Err(e) => {
                files.push(format!(
                    "    {{\"file\": {}, \"error\": {}}}",
                    json_str(&list.get(i).unwrap().name),
                    json_str(&e.to_string())
                ));
                continue;
            }
        };
        let probe_ms = ms(t);
        let (ow, oh) = probe.oriented_size();
        let fit = (SCREEN.0 as f64 / ow as f64).min(SCREEN.1 as f64 / oh as f64);
        let need = needed_decode_size((probe.width, probe.height), fit);
        let req = if need.0 >= probe.width {
            Request::Full
        } else {
            Request::Fit {
                w: need.0,
                h: need.1,
            }
        };
        let t = Instant::now();
        let img = wic::decode_any(&data, req).map_err(|e| e.to_string())?;
        let decode_ms = ms(t);
        let t = Instant::now();
        let gpu = upload(
            &gfx.device,
            &img,
            probe.orientation,
            gfx.max_texture_size().min(4096),
        )
        .map_err(|e| e.message())?;
        let upload_ms = ms(t);
        let t = Instant::now();
        let l = compute(
            (ow, oh),
            (SCREEN.0 as f64, SCREEN.1 as f64),
            &ViewOptions::default(),
            &ViewState::default(),
        );
        let full = (gpu.full_width, gpu.full_height);
        let stage = (gpu.width, gpu.height);
        let bg = Rgba(0.1, 0.1, 0.1, 1.0);
        renderer.begin(&gfx.context, &target.rtv, SCREEN, bg);
        renderer.draw_image(
            &gfx.context,
            &gpu,
            &stage_to_client(&l, probe.orientation, full, stage),
            &content_in_stage(&l, probe.orientation, full, stage),
            bg,
        );
        target.sync(&gfx.context).map_err(|e| e.message())?;
        let render_ms = ms(t);
        let total_ms = ms(t_all);
        files.push(format!(
            "    {{\"file\": {}, \"full\": [{}, {}], \"stage\": [{}, {}], \"read_ms\": {}, \"probe_ms\": {}, \"decode_ms\": {}, \"upload_ms\": {}, \"render_ms\": {}, \"total_ms\": {}}}",
            json_str(&list.get(i).unwrap().name),
            probe.width,
            probe.height,
            stage.0,
            stage.1,
            json_num(read_ms),
            json_num(probe_ms),
            json_num(decode_ms),
            json_num(upload_ms),
            json_num(render_ms),
            json_num(total_ms)
        ));
    }

    // 2. Navigation at 30 Hz key repeat with prefetching.
    let cpu = threads::cpu_info();
    let loader = Loader::new(2, cpu.performance.clamp(2, 6));
    loader.set_device(gfx.device.clone(), gfx.max_texture_size());
    loader.set_screen_box(SCREEN);
    let mut ready: HashSet<u64> = HashSet::new();
    let mut latencies = Vec::new();
    let n = list.len();
    let presses = (n * 2).max(8);
    for step in 0..presses {
        let i = step % n;
        let id = |j: usize| list.get(j).unwrap().cache_id();
        let mut jobs = Vec::new();
        for (k, prio) in [
            (0isize, 0u32),
            (1, 1),
            (2, 3),
            (3, 5),
            (4, 7),
            (-1, 3),
            (-2, 6),
        ] {
            let j = list.offset(i, k).unwrap();
            if ready.contains(&loader::key(id(j), Stage::Screen))
                || jobs.iter().any(|x: &Job| x.cache_id == id(j))
            {
                continue;
            }
            jobs.push(Job {
                path: list.path(j).unwrap(),
                cache_id: id(j),
                stage: Stage::Screen,
                with_preview: false,
                priority: prio,
            });
        }
        loader.set_wanted(jobs);
        let want = loader::key(id(i), Stage::Screen);
        let t = Instant::now();
        let deadline = t + Duration::from_secs(10);
        while !ready.contains(&want) && Instant::now() < deadline {
            if let Ok(l) = loader.results.recv_timeout(Duration::from_millis(5))
                && (l.image.is_ok() || l.stage != Stage::Preview)
            {
                ready.insert(loader::key(l.cache_id, l.stage));
            }
        }
        latencies.push(ms(t));
        // Key repeat: next press 33 ms after this one.
        while t.elapsed() < Duration::from_millis(33) {
            if let Ok(l) = loader.results.recv_timeout(Duration::from_millis(2)) {
                ready.insert(loader::key(l.cache_id, l.stage));
            }
        }
    }
    let hits = latencies.iter().filter(|&&l| l < 1.0).count();
    let nav = Stats::of(&latencies).unwrap();

    // 3. Listing a folder of 10k images.
    let enum_dir = std::env::temp_dir().join("blinzel-enum-10k");
    if !enum_dir.join("IMG_09999.jpg").exists() {
        let _ = std::fs::create_dir_all(&enum_dir);
        for i in 0..10_000 {
            let _ = std::fs::write(enum_dir.join(format!("IMG_{i:05}.jpg")), []);
        }
    }
    let mut enum_ms = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let e = enumerate::list_dir(&enum_dir).map_err(|e| e.to_string())?;
        let l = FileList::new(&enum_dir, e);
        std::hint::black_box(l.len());
        enum_ms.push(ms(t));
    }
    let enum_stats = Stats::of(&enum_ms).unwrap();

    Ok(format!(
        "{{\n  \"warp\": {warp},\n  \"device_ms\": {},\n  \"screen\": [{}, {}],\n  \"open\": [\n{}\n  ],\n  \"navigation\": {{\"presses\": {}, \"prefetch_hits\": {}, \"latency\": {}}},\n  \"list_10k\": {}\n}}",
        json_num(device_ms),
        SCREEN.0,
        SCREEN.1,
        files.join(",\n"),
        presses,
        hits,
        nav.json(),
        enum_stats.json()
    ))
}
