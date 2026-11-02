---
name: spec-extract
description: Reverse engineering — writes feature specs from existing code, or records the current state of the code in an open spec.
argument-hint: '<area or feature-name> | --analyze [<spec-name>]'
---

# Spec Extract

You analyse existing code and turn it into feature specs in the format of `/spec`. Two modes:

| Call | Mode | Function |
| ---- | ---- | -------- |
| `/spec-extract <area>` | Reverse engineering | Derive a spec from code |
| `/spec-extract --analyze [<name>]` | Current state | Enrich an existing spec with the state of the code |

The template and the AC rules are those of `/spec` (`.claude/skills/spec/SKILL.md`) — read them first.

---

## Mode: reverse engineering (`<area>`)

### 1. Explore the code

Use Explore agents to find all code for `$ARGUMENTS`:

- Logic in `blinzel-core` (decoders, batch, thumbnails, update, …) and `blinzel-jpeg`
- The interface in `blinzel-ui`: `App` (`app.rs`), commands (`Cmd`), keys (`App::key`), menu (`menu.rs`), dialogs, settings
- The platform shells in `blinzel-app`: Windows (`src/`), `src/linux/`, `src/macos/`
- Strings in `i18n/*.toml`, benchmarks in `blinzel-bench`, CI steps in `.github/workflows/`
- Tests: inline `#[cfg(test)]`, `tests/`, golden tests; which CI job runs them
- Design-spec sections in `docs/specs/blinzel.md` that describe the area

### 2. Reconstruct

From the code derive:

- **What it does** — summarize the feature from the implementation
- **Acceptance criteria** — testable ACs in the five forms from the actual behavior (not what it should do)
- **Technical notes** — existing data structures, file-format fields, platform APIs, thread model, UI patterns
- **Edge cases** — error handling, guard clauses, validations
- **Evidence** — per AC the existing test that checks exactly this behavior (test name + file). Status `built` if such a test exists, else `open`. A test that only the Windows or macOS CI job runs: the AC carries `CI: windows` / `CI: macos` after its form.

### 3. Present + refine

Show the user:

- "Here is what the code does for `$ARGUMENTS`"
- "These ACs are covered by tests: …"
- "These have no tests: …"
- "These gaps / inconsistencies (also against the design spec) I found: …"

Ask: "Shall I save it like this, or do you want to add planned improvements as new ACs?"

### 4. Write the spec

Write to `docs/specs/$0.md` with the template from `/spec`. If that name belongs to the design spec, pick `$0-<aspect>`.

- Checkboxes stay `[ ]`, status `todo` — even if everything seems covered. Only `/spec-check` decides on `implemented`.
- Tell the user: if all ACs are covered, `/spec-check $0` is enough for acceptance; if tests are missing, `/spec-build $0` closes the gaps.

---

## Mode: current state (`--analyze`)

### 1. Load specs

- Scan `docs/specs/` for feature specs with `status: todo`
- With a name: only that spec
- Without a name and several: process them one after another

### 2. Analyse the code

Per spec:

1. Identify affected areas from Context and Technical notes
2. Read the code and analyse the current state
3. Match ACs against the code — are implementation approaches visible?
4. Identify relevant types, commands (`Cmd`), platform code, tests

### 3. Write the current state

Update or add the `## Current state` section in the spec:

```markdown
## Current state

> Last updated: {YYYY-MM-DD}

### Existing implementation

- `crates/blinzel-core/src/batch/…` — resize engine with name planning exists

### AC coverage (estimated)

| AC    | Status      | Reason                              |
| ----- | ----------- | ----------------------------------- |
| AC-01 | Partial     | Engine exists, no dialog yet        |
| AC-02 | Not started | No code                             |
```

**An existing `## Current state` is replaced completely** — the code is the truth.

---

## Commit

Whether you commit the written files without asking is decided by the **commit mode** in Context (`SPEC_AUTOCOMMIT`, per developer):

- **on** (default — also when the Context line is missing or says anything other than `ask`): `spec-build` only starts on a clean working tree and reads the committed spec — so commit yourself:
  1. Only the affected files, by path: `git add docs/specs/<name>.md` — never `git add -A`, no other files.
  2. Commit `docs(spec): extract <name>` or `docs(spec): analyze <name>`. Add the new spec to `docs/specs/README.md` in the same commit.
  3. On `main` (changes go through PRs, AGENTS.md §8) first switch to `feat/<name>`, where `spec-build` continues.
- **ask**: ask via AskUserQuestion before the commit — do not end the turn without this question. The question names the files and the commit message; the options are "Commit now" and "Continue without commit":
  - "Commit now": commit as with **on**.
  - "Continue without commit": you don't commit. Name the `git add` command with exactly these paths and the commit message, and point out that `spec-build` only starts once the specs are committed.

In both modes no `git push` — pushing and merging is the user's job.

## Rules

- **Document what IS, not what SHOULD be** — reverse engineering is a snapshot of the current state
- **No spec changes** except `## Current state` in analyze mode
- **Don't change the front-matter status** of an existing spec — only `/spec-check` sets `implemented`
- **Take types and code verbatim** — don't paraphrase
- Specs are **English**; talk to the user in their language

## Context

Existing specs:
!`grep -L '^## Acceptance criteria' docs/specs/*.md 2>/dev/null | sed 's/$/ (design spec)/'; grep -l '^## Acceptance criteria' docs/specs/*.md 2>/dev/null || echo "No feature specs yet"`

Commit mode:
!`node .claude/hooks/commit-mode.mjs 2>/dev/null || echo "on"`
