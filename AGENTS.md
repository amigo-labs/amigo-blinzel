# AGENTS.md — amigo-blinzel implementation guide

This file is the operational contract between Claude Code and the amigo-blinzel codebase. It keeps work predictable and **scoped small enough that no single session runs into context limits or timeouts**.

Read this file before starting any task. Re-read Section 3 ("Session management") at the start of every session.

---

## 1. Project overview

amigo-blinzel is an image viewer for Windows, macOS and Linux with one goal: **maximum speed** when opening an image, when flipping through a folder and for thumbnails, measurably faster than every alternative and proven on the website with reproducible benchmarks. One platform-independent interface (`blinzel-ui`) runs inside one native shell per platform: Win32 + Direct3D 11, AppKit + Metal, winit + wgpu.

The design specification lives in **`docs/specs/blinzel.md`**: goal, decisions (§2), the benchmark gate (§3), architecture, platforms, measurements. It is the source of truth for *what* to build and *why*. The feature specs next to it (`docs/specs/<name>.md`) say what each feature does and prove it with tests. This file is the source of truth for *how* to build it.

**Read the relevant section of `docs/specs/blinzel.md` and the feature spec before starting an implementation task.** Load only what is needed.

---

## 2. Workflow

### 2.1 Spec-driven

Every implementation task traces back to `docs/specs/blinzel.md` or a feature spec. If a task implies a design decision the design spec does not cover, **stop and update the design spec first**: a row in the decisions table (§2) plus the details in the matching section. Do not improvise architecture.

New features and behavior changes get a **feature spec** `docs/specs/<name>.md` with testable acceptance criteria, written with `/spec`, built with `/spec-build`, accepted with `/spec-check`. Triage, the two kinds of spec and where knowledge goes: `.claude/rules/specs.md`. Bug fixes, refactorings, tooling and docs need no feature spec.

### 2.2 Compiler as gatekeeper

For Rust work the compiler is the primary verification tool:

1. State the change in one sentence.
2. Make the smallest change that could satisfy it.
3. `cargo check -p <crate>` must pass.
4. `cargo test -p <crate>` must pass for the changed module.
5. `cargo clippy -p <crate> --all-targets -- -D warnings` must pass.
6. Commit.

Never accumulate compiler errors.

### 2.3 Test first where sensible

Decoders, the batch engine, layout, selection, the scheduler, update decisions and everything in `blinzel-core` and `blinzel-ui` are test-first: fixture and assertion, run it red, implement, run it green. Pixel output is checked against a reference: libjpeg-turbo for `blinzel-jpeg` (byte-identical), the CPU reference renderer for the GPU shaders (§7.3).

Platform shell code (window, menus, dialogs) is test-after where a test is possible at all; what only a person can judge is a manual check (§7.5).

### 2.4 Commits

One concept per commit. Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/): `type: description` or `type(scope): description`, the scope being the crate or area (`core`, `jpeg`, `ui`, `app`, `macos`, `linux`, `windows`, `release`, `site`, `spec`, …):

```text
feat: screenshots on macOS with ScreenCaptureKit and a Cmd+Shift+1 hot key
fix: benchmark gate ignores a noisy median when the minimum disagrees
docs(spec): add batch-resize
```

The type drives releases (release-please, `release-please-config.json`): `feat` and `fix` go into `CHANGELOG.md` and start a release; `docs`, `refactor`, `test`, `build`, `ci` and `chore` don't. A breaking change (`feat!:`) bumps the major version, only on purpose.

If a commit message needs an "and" between two unrelated changes, split it.

---

## 3. Session management

### 3.1 Pre-flight checklist

```text
[ ] git status is clean (or I have a plan for the dirty state)
[ ] cargo check passes on the clean checkout
[ ] I have read the relevant docs/specs/blinzel.md section and feature spec
[ ] I can state the task in one sentence
[ ] I can estimate its size: XS / S / M / L
[ ] If L: STOP. Split into smaller tasks. Do not start.
```

### 3.2 Task sizing

| Size | Definition | Action |
| ---- | ---------- | ------ |
| **XS** | 1 file, ≤ 30 lines | inline, no ceremony |
| **S** | 1–3 files, ≤ 150 lines | one session, one commit |
| **M** | 3–6 files, ≤ 400 lines, several commits | one session, plan the commits up front |
| **L** | 6+ files, or a new module / crate | **split into M tasks** before starting |
| **XL** | a new milestone or platform | **split into L tasks**, one feature spec each |

A session produces one M task or two to three S tasks, never a single L task.

### 3.3 Stopping points

Stop and yield to human review when:

- A new dependency is added to a `Cargo.toml` (it must pass the benchmark gate, §4, or have a decision in the design spec)
- The design spec needs a new decision
- A new public API surface is added to `blinzel-core` or `blinzel-ui` that the spec does not name
- `unsafe` is added outside a platform shell, FFI binding or SIMD kernel
- A test was disabled or `#[ignore]`d
- Three consecutive `cargo check` failures with the same root cause
- The session has produced ~400 changed lines and a coherent stopping point exists

The state lives in the repository, not in a status file: in a feature spec's `## Evidence`, `## Open questions` and `## Decisions`, otherwise in the commit message.

### 3.4 Resume protocol

1. `git log --oneline -10`
2. Read the open feature specs the SessionStart hook lists (order: `docs/specs/README.md`)
3. Read the last commit message and diff
4. `cargo check && cargo test` must be green before proceeding
5. State the next task, then run the pre-flight checklist (§3.1)

If `cargo check` is red on a fresh checkout, **fix the build first**.

### 3.5 Context discipline

- **Read minimally.** Use `grep` / `rg`; don't load a whole file for one function.
- **Don't re-read what you just wrote.**
- **Don't echo specs into code.** Reference them: `// docs/specs/blinzel.md §5`.

---

## 4. Benchmark gate

Speed is the product, so no library is adopted unchecked (`docs/specs/blinzel.md` §3). For every hot path:

1. `crates/blinzel-bench` measures the library (the yardstick) and the in-house candidate on the deterministic corpus (`blinzel-bench corpus`), with the same pixels.
2. The faster one gets built in; the library stays as the yardstick and pixel reference.
3. The result, with numbers and the machine, goes into the gate table (§3) and the measurements (§15) of the design spec.

Measure alternately in the same process or run (`blinzel-bench jpeg-ab`, `compare`), never against a number from another day or machine: laptops vary by up to 30 % between runs.

---

## 5. Crate conventions

### 5.1 `blinzel-core`

Decoders, EXIF/MPF, layout, selection, scheduler, caches, thumbnails, batch engine, update decisions, CPU reference renderer. **Platform-independent and tested on Linux**: no Win32, AppKit or windowing types. Platform decoders (WIC, ImageIO) live in `blinzel-app`.

### 5.2 `blinzel-jpeg`

Own JPEG parser and parallel decoder. `#![forbid(unsafe_code)]`. Its output is byte-identical with libjpeg-turbo; `blinzel-core/tests/own_jpeg.rs` and `blinzel-bench run --check` prove it.

### 5.3 `blinzel-ui`

The whole interface as one state machine (`App`): menu model, dialogs, filmstrip, grid, batch dialog, screenshot overlay, settings file. It talks only to the traits in `backend.rs` (`Device`, `Canvas`, `Host`, `Services`). Platform-independent and tested on Linux.

### 5.4 `blinzel-app`

The `blinzel` binary: one shell per platform implementing those traits. Windows at the top level of `src/` (`window.rs`, `run.rs`, `gfx/`, `platform/`, `setup/`), Linux in `src/linux/`, macOS in `src/macos/`; shared by Linux and macOS: `selfupdate.rs`, `curl.rs`, `lcms.rs`. A feature that changes behavior belongs in `blinzel-ui` or `blinzel-core`, not in one shell; the shells differ only where the platform does.

### 5.5 Strings

Every user-visible string goes through `blinzel-i18n`: a key in every file under `i18n/` (all languages are kept complete; a missing key falls back to English with a build warning).

---

## 6. Code style

### 6.1 Rust

- Toolchain pinned in `rust-toolchain.toml`; `cargo fmt` with `rustfmt.toml` (`max_width = 100`).
- `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- `unsafe` gets a `// SAFETY:` comment saying why it holds.
- Doc comments (`///`) on public items of the library crates.

### 6.2 Markdown / specs

- English, headings in sentence case.
- Code blocks always with a language tag; tables for structured comparisons.

---

## 7. Testing

### 7.1 Unit tests

Inline `#[cfg(test)] mod tests` next to the code, named after what they prove (`<function>_<scenario>_<expected>` where it reads well).

### 7.2 Integration tests

`tests/` at the crate root for cross-module flows: `blinzel-core/tests/own_jpeg.rs` (byte-identity), `blinzel-app/tests/` (setup, placement, atlas, icon, golden tests).

### 7.3 Golden render tests

The GPU shaders are line-by-line ports of each other and are checked against the CPU reference renderer: `warp_golden.rs` (Direct3D 11 on WARP, Windows CI), `wgpu_golden.rs` (lavapipe, Linux CI), `metal_golden.rs` (macOS CI). `BLINZEL_REQUIRE_GPU=1` turns a missing device into a failure instead of a skip; CI sets it.

### 7.4 Benchmarks and measurements

Performance claims are measured, never estimated (§4): `blinzel-bench` for libraries vs. in-house code, `blinzel --bench` for the app end to end, `--exit-after-first-frame --json` for startup, `blinzel-race` for the comparison with other viewers. A performance acceptance criterion names the command and the machine.

### 7.5 Platform coverage

Locally (Linux) runs everything platform-independent plus the Linux shell. The Windows and macOS shells are built and tested only in their CI jobs. What no automated test can judge (look, feel, a system dialog, Gatekeeper, SmartScreen) is a manual check on that platform, recorded in the feature spec's Evidence as `manual: {who} checked {what}, {date}`.

---

## 8. Branch and PR conventions

- Changes go through PRs to `main`.
- Branch names: `feat/<short-desc>`, `fix/<short-desc>`, `chore/<short-desc>`, `spec/<short-desc>`.
- PR title in the commit convention: `type(scope): description`.
- PR description: the spec section or feature spec it implements, the tests that cover the change, open questions.
- CI must be green before review.

---

## 9. Forbidden patterns

- **Modifying an image file.** The viewer never writes to an image it shows. Batch output and screenshots are new files: an existing name gets a number instead of being replaced. "Save cutout as" goes through the system's save dialog and its overwrite confirmation, and never replaces its own source.
- **Rotating pixels for the EXIF orientation.** Orientation is a transform for the GPU and the CPU reference.
- **Work on the startup or browsing critical path that can wait.** Loading a DLL, reading settings that are not needed for the first frame, synchronous I/O on the UI thread: move it after the first frame or onto a worker (`docs/specs/blinzel.md` §5).
- **A decode outside `catch_unwind`.** A broken file must never take the viewer down.
- **Platform types in `blinzel-core` or `blinzel-ui`.**
- **A user-visible string outside `i18n/`.** Technical reasons that come from a decoder, an encoder or the OS (`DecodeError`, `io::Error`) may stay English inside a translated message.
- **A new dependency without the benchmark gate (§4) or a decision in the design spec.**
- **New suppressions** (`#[allow(…)]`, `#[expect(…)]`) instead of fixing the cause. Where one is truly justified (e.g. `#[allow(deprecated)]` for an AppKit API without a replacement on macOS 14), the reason is a comment next to it, and the user adds it.
- **Disabling a test instead of fixing it.** A test that cannot run in CI says why next to its `#[ignore]`.

---

## 10. Common commands

All from the repository root.

```bash
cargo check                                   # workspace
cargo test -p blinzel-core                    # one crate
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo test --workspace
cargo run --locked -p blinzel-release -- notices --check   # license notices current (after a Cargo.lock change: without --check)
cargo xwin check --target x86_64-pc-windows-msvc           # Windows code from Linux, where cargo-xwin is installed

cargo build --release -p blinzel-app          # target/release/blinzel(.exe)
target/release/blinzel <image> --exit-after-first-frame --json startup.json

cargo run --release -p blinzel-bench -- corpus testdata/generated
cargo run --release -p blinzel-bench -- run testdata/generated --check --json bench.json
cargo run --release -p blinzel-site           # website/dist
```

`.claude/gates.json` holds the gate commands: `fast` (Stop hook) and `full` (`/qa`, the Linux job of `.github/workflows/ci.yml` without its two Xvfb steps). The Xvfb steps (viewer smoke test, X11 display profile) and the Windows and macOS jobs (Direct3D/WARP golden test, benchmarks, Metal golden test, app bundle) run in CI only.

---

## 11. Definition of done

- [ ] The change is minimal and matches the spec
- [ ] `/qa` is green (fmt, clippy, tests, license notices)
- [ ] Code that only Windows or macOS builds is green in CI
- [ ] New behavior has at least one test; performance claims are measured (§7.4)
- [ ] New public items of the library crates have doc comments
- [ ] New strings are in every `i18n/*.toml`
- [ ] The design spec is updated where the design shifted
- [ ] No forbidden pattern (§9)
- [ ] Commit messages follow the convention

For feature-spec work add:

- [ ] `/spec-check` verdict is `READY` and the spec is `status: implemented`
- [ ] `docs/specs/README.md` lists the spec under "Done"

---

## 12. When to stop and ask

Stop and ask when:

- The spec is silent or ambiguous on something that affects the public API or the user-visible behavior
- Two reasonable implementations exist and the choice is not local
- A new dependency is needed
- The task would take more than one session
- A test fails for a reason that suggests the spec is wrong
- A measurement contradicts a decision in the design spec
- Performance work needs an architectural change (a crate boundary, a new thread model)

Do not stop for style choices, local refactors, extra tests, documentation or renames within one module.

---

## 13. Skills

| Skill | Purpose |
| ----- | ------- |
| `/spec <name>` | Write or revise a feature spec with acceptance criteria and a clarification pass |
| `/spec-build <name>` | Implement a feature spec test-first, one commit series per AC block, independent verifier loop |
| `/spec-check <name>` | Independent acceptance against a full gate run; only it sets `status: implemented` |
| `/spec-extract` | A feature spec from existing code, or the current state of an open spec |
| `/qa` | Full gate run (`.claude/gates.json`, the Linux part of CI) |

Rules under `.claude/rules/` load with this file. Hooks (`.claude/settings.json`, they need Node.js): SessionStart lists the open feature specs, a guard refuses new suppressions (§9), and the Stop hook runs the fast gates for the areas a turn changed.

---

## 14. Where things live

```text
docs/specs/blinzel.md         The design spec: goal, decisions (§2), benchmark gate (§3), architecture, platforms, measurements
docs/specs/<name>.md          Feature specs with acceptance criteria (/spec)
docs/specs/README.md          Index: order, done, backlog, open items outside the code
docs/SECRETS.md               The secrets of the workflows
AGENTS.md                     This file (CLAUDE.md only imports it)
.claude/                      Skills, rules, hooks, gates.json

crates/blinzel-core/          Platform-independent core (decoders, batch, thumbnails, update logic)
crates/blinzel-jpeg/          Own parallel JPEG decoder
crates/blinzel-ui/            Platform-independent interface (App, menu, dialogs, grid, snip)
crates/blinzel-i18n/          Translations from i18n/*.toml
crates/blinzel-app/           The binary: Windows shell (src/), src/linux/, src/macos/
crates/blinzel-bench/         Corpus and benchmark gate
crates/blinzel-race/          Comparison with other viewers
crates/blinzel-release/       Release tooling (keys, signatures, notices, macOS bundle and DMG)
crates/blinzel-site/          Website generator (website/ → website/dist)
packaging/                    winget manifests, Linux icon, macOS signing certificate script
```

---

## Gotchas

Pitfalls that code and git history don't show. Short entries; `/spec-build` adds to this list when a loop surprised it.

- **No early `return` in an objc2 `define_class!` method that returns `bool`.** The macro turns the declared `bool` into objc2's `Bool`, and a `return false;` inside the body does not convert (E0308, only the macOS CI job sees it). Write the body as an expression (`if … { false } else { …; true }`).
- **Windows developer machine: the rustup host is `windows-gnu`.** Build the app with `cargo +1.99.1-x86_64-pc-windows-msvc …`. NASM (`%LOCALAPPDATA%\bin\NASM`) and the CMake that comes with Visual Studio (`Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin`) must be in the `PATH`.
- **Laptop measurements vary by up to 30 %** between consecutive runs (throttling). Measure the candidates alternately and run libjpeg-turbo alongside as a control (§4).
- **Git Bash rewrites arguments that look like paths** (`/CN=…` becomes a Windows path). Scripts that pass such arguments set `MSYS_NO_PATHCONV=1`.
- **Shell scripts under `packaging/` are run, not sourced.** Their `set -e` and `exit` would close the terminal; `create-signing-cert.sh` refuses to be sourced.
- **A re-requested thumbnail comes from the disk cache, not the file.** In App tests, counting file reads (`Harness::thumb_reads`) shows only the first request of a thumbnail; whether an evicted one was asked for again shows in the atlas (`Browse::has_thumb`).
- **`App` commands write the settings file, and the UI language is a process global.** In `blinzel-ui` tests, `settings_store` only writes to the thread's `TEST_CONFIG_DIR` (the harness's temp folder), never `~/.config`; App-level tests use `testkit::Harness`, which holds a lock so tests that switch the language cannot race. The harness auto-applies only `Uncloak` (each render returns it until the shell clears `cloaked`); other window feedback goes through `Harness::shell`.
- **`blinzel-core`'s batch tests need its `batch` feature.** `cargo test -p blinzel-core --lib batch::run` alone runs 0 tests and says ok; add `--features batch` (the workspace run gets it through `blinzel-ui`).
- **Two `Harness`es alive in one test deadlock.** Each holds the process-wide harness lock, and shadowing (`let mut h = …` again) does not drop the first: `drop(h)` before making the next one. The symptom is a test that hangs at 0 % CPU, not a failure.
- **`Harness::pump` lets the App cancel a job before it is done.** The App cancels a loader job once the stage it was for arrives, so whatever the job would wrongly do after that (decode again, ask the system decoder) is usually cut short and an assertion that it did not happen passes anyway. Tests that assert what a job did *not* do use `Harness::settle` / `open_settled`, which hold every result back until the loader is idle.
- **Only JPEG decodes smaller for the screen.** A PNG's Screen stage is already full resolution, so zooming never asks for a Full stage; an App test that needs a sharper second result of the same image uses a JPEG larger than the harness's 1920 × 1080 screen box.
- **The harness's window fits its small test images.** App tests that read the batch dialog's drawn rows call `Harness::resize` first; in the tiny window every name is elided to `…name`.
- **Display-bound Linux tests run only on Xvfb.** A test that needs an X server returns early without `BLINZEL_X11_TEST` (so `cargo test --workspace` stays headless) and sets `DISPLAY` itself; the Linux CI job's xvfb-run step lists it by module filter and runs with `--test-threads=1`. Locally: `xvfb-run -a sh -c 'BLINZEL_X11_TEST=$DISPLAY cargo test -p blinzel-app --lib --test startup -- linux::color linux::clipboard startup_order window_class --test-threads=1'`. The Windows target does not `cargo check` here as is (C dependencies such as `libwebp-sys` need MSVC); a type-check and clippy of `blinzel-app` work with `--no-default-features --tests --target x86_64-pc-windows-msvc`, `AR_x86_64_pc_windows_msvc=llvm-lib` and `CC_x86_64_pc_windows_msvc` set to a stand-in `clang-cl` script that runs `clang --driver-mode=cl --target=x86_64-pc-windows-msvc "$@"` and, where that fails for lack of MSVC headers, compiles an empty C file to the `-Fo` output instead.
- **`blinzel-race` needs `turbojpeg-sys` for the cross-target type-check.** Its vendored CMake build fails with the stand-in compilers; for `cargo clippy -p blinzel-race --all-targets --target …` set `<TARGET>_TURBOJPEG_SOURCE=explicit` and `<TARGET>_TURBOJPEG_LIB_DIR` to any existing folder (e.g. `AARCH64_APPLE_DARWIN_…`, `X86_64_PC_WINDOWS_MSVC_…`): a check does not link, and the bindings are pregenerated.
- **Animated JPEG XL fixtures: Ubuntu's `cjxl` (libjxl 0.7) drops the loop count** and writes 1000/1 ticks per second for APNG input (100/1 for GIF). A fixture that needs either is rewritten in its animation header afterwards; the header is not entropy coded, but the bits after it shift, so rebuild up to the end of the image header and pad to the byte where the first frame starts (`docs/specs/animations.md`, Decisions).
- **`profile.test.package."*"` does not cover workspace members.** `blinzel-core` and `blinzel-bench` run unoptimized in `cargo test` (only `blinzel-jpeg` has its own override), so a test that runs `blinzel-bench run` on a small folder still spends about 75 s on the 12 MP own-kernel cases. Keep benchmark tests on small input and test the output logic as pure functions.

---

*Amend in place via PR. A significant change to the workflow gets a decision in `docs/specs/blinzel.md` §2.*
