---
name: qa
description: Runs the project's full QA pass (rustfmt, clippy with -D warnings, cargo test for the whole workspace, third-party notices current) — independent stages in parallel, with exactly the commands the Linux CI job runs. Use this skill before finishing any coding task, before a commit or PR, and whenever the user wants to know whether "everything is green".
---

# QA

The commands are not here but in `.claude/gates.json` (list `full`). It mirrors the Linux job of `.github/workflows/ci.yml`; the Windows and macOS jobs run in CI only (AGENTS.md §10). Green here means green in the Linux job.

## Procedure

1. From the repo root run `node .claude/hooks/gates.mjs full` (Bash with `timeout: 600000`; a cold build of the workspace takes minutes, so start it with `run_in_background` and wait for the notification). The script runs every entry of `full`, independent stages at the same time (`needs` in `gates.json`), and prints a table, plus for every red stage its error lines, the `fix` command and the path to the full log.
2. Exit 2 means: `gates.json` is missing or broken. Report it and stop.
3. Fix **all red stages** in the code, then start the run **from scratch** — a fix can break another stage. A skipped stage is not green: it waits for a red one and only runs in the next pass. If the error lines aren't enough, read the log.
4. Where a stage has a `fix` command, it may fix what is auto-fixable (formatting). Start over afterwards anyway.
5. The stage "third-party notices current" is red when `Cargo.lock` changed without regenerating the license notices: run its `fix` command (`cargo run -p blinzel-release -- notices`) and commit the regenerated `THIRD-PARTY-NOTICES*.txt` with the change — review them, don't revert them.
6. If the change touches code that only Windows or macOS builds (`crates/blinzel-app/src/` outside `linux/`, `src/macos/`), say so in the report: the local run doesn't compile it. Where `cargo xwin` is installed, `cargo xwin check --target x86_64-pc-windows-msvc` checks the Windows part.

If `.claude/hooks/gates.mjs` is missing, run the entries of `full` **in the given order** one by one, each `command` in its `cwd` (relative to the repo root), stop at the first failure, fix and start over. Don't omit, rephrase or replace a command with an "equivalent" one.

## Forbidden

- Lowering lint levels or suppressing warnings: no new `#[allow(…)]` / `#[expect(…)]`.
- Changing the gate configuration: `.claude/settings.json`, `.claude/hooks/`, `.claude/gates.json`, the CI workflow.
- Deleting, skipping (`#[ignore]`) or weakening tests to make them green.

If a warning really is a false positive after careful review, don't resolve it yourself: name the spot and the reason to the user (AGENTS.md §9).

## Report

Short, as a table (the last run's, without the log column):

| Gate | Command | Result |
| ---- | ------- | ------ |

Plus: what was fixed (file, cause), what stays open, and whether Windows- or macOS-only code is unchecked locally. No "should be green now" — only what the last run actually showed.
