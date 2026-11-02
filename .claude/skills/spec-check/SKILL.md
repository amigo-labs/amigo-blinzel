---
name: spec-check
description: Independent acceptance of a feature spec — checks every acceptance criterion against a real test run, gives a verdict (READY / NOT READY) and sets status implemented only on READY. Use this skill when the user wants to know whether a spec is fulfilled, or wants to close a spec after implementing it by hand.
argument-hint: '<spec-name>'
---

# Spec Check: $ARGUMENTS

You check whether the state of the repository fulfils the feature spec `docs/specs/$0.md`. You are the verifier: you derive **everything from the repository anew** — never from checkboxes, never from the Evidence table, never from commit messages. The table is a claim you check.

Without an argument: take the feature spec with `status: todo` from Context; with several, let the user pick via AskUserQuestion; with none, point to `/spec`.

## Procedure

1. **Load:** read the spec in full, plus `git status` and `git log --oneline -20`.
2. **Run the tests:** `node .claude/hooks/gates.mjs full` (like `/qa`, without repair; Bash with `timeout: 600000`, or `run_in_background` and wait for the notification — a cold workspace build takes minutes). If the script is missing, run the entries of `full` in `.claude/gates.json` in order. You repair nothing — a red or skipped gate is a blocker.
3. **Find evidence per AC:** find the test that checks exactly this behavior and look up its result in the run from step 2 — the complete output of every gate is in the log from the table (`cargo test` prints `test <path>::<name> ... ok`).
   - **Evidence is test name + result from this run.** A `file:line` reference is context only, not proof.
   - The test must really check the AC: a test that would also be green without the implementation (no assertion on the behavior, only a call without a check) doesn't count.
   - **Prompt artifacts** (SKILL.md, rules): a structural test against the text counts as evidence if it goes red without the checked statement — counter-check: delete the statement or turn it into its opposite. If you find a rewording that deliberately dodges the test, that is a note, not a blocker: every text-invariant test can be dodged that way, and an acceptance that demands otherwise never ends.
   - Manual ACs (suffix after the form, e.g. `(state, manual: visual design)`): fulfilled only with an entry `manual: {who} checked {what}, {date}` in the Evidence column `Test`. Without it the AC stays open.
   - CI-only ACs (`CI: windows`, `CI: macos`): the test runs only in that CI job. Fulfilled when the test exists, really checks the AC and the job is green for the current commit (GitHub check runs of the branch or `main`); without access to CI the AC stays `unchecked`, a note in the report, not a pass.
   - Dropped ACs (struck through, Evidence status `dropped`) need no evidence and don't count as open. A test that still checks them is a note in the scope check.
4. **Contradictions:** if the code contradicts an AC, an edge case or a decision of the spec — or a design-spec section it references — that is a blocker.
5. **Definition of done** (AGENTS.md §11): new public items of the library crates without doc comments, user-visible strings outside `i18n/` or missing in one of its files, or a forbidden pattern from §9 are blockers.
6. **Scope check:** changed tests and files of the branch (`git diff --stat main...HEAD`) that map to no AC are listed as notes — not a blocker.

## Verdict

| Verdict | When |
| ------- | ---- |
| `READY` | Every AC has evidence, all gates green |
| `READY (manual: n)` | like READY, n ACs via documented manual check |
| `READY (CI: n)` | like READY, n ACs proven by a green Windows or macOS CI job |
| `NOT READY` | at least one blocker: AC without evidence, red gate, contradiction code ↔ spec, definition of done not met, non-empty `## Open questions` |
| `NOT READY — unchecked` | the tests didn't run (build broken, environment missing) — no run, no evidence |

When in doubt `NOT READY`. Rather one more round than an AC waved through.

## Report

In the user's language:

```markdown
## Spec Check: {name}

**Verdict:** {verdict}

### Acceptance criteria

| AC    | Status    | Evidence               |
| ----- | --------- | ---------------------- |
| AC-01 | fulfilled | `{test name}` — passed |
| AC-02 | open      | {what is missing}      |

### Gates

| Gate | Command | Result |
| ---- | ------- | ------ |

### Blockers

- {concrete, actionable — becomes a task in spec-build}

### Notes

- {scope check, anything conspicuous}
```

## Apply the result

**Only on `READY`, `READY (manual: n)` or `READY (CI: n)`** do you change the spec:

1. Front matter: `status: todo` → `status: implemented`
2. All AC checkboxes to `[x]`
3. `## Evidence`: test name and file from your check, status `verified`. Fill the `Commit` column where it is empty: the first commit of the branch that names the AC in its message (`git log --reverse --format=%h --grep='AC-01' main..HEAD`, counting ACs in ranges like `AC-01..AC-11`)
4. Insert `## Summary`:

```markdown
## Summary

> Completed: {YYYY-MM-DD}

- {What was built, 2-5 bullets}
```

5. Remove `## Current state` (if present). `## Decisions` stays — that is long-term project knowledge.

On any other verdict, no change to the spec file — only the report. You don't commit; `spec-build` or the user does.

## Rules

- **Derive, don't believe:** checkboxes, table and commit messages are claims.
- **No run, no evidence:** without a test run in this check the verdict is `NOT READY — unchecked`.
- **Repair nothing:** no code, test or config changes. You are the reviewer, not the developer.

## Context

Spec status:
!`grep -H '^status:' docs/specs/*.md 2>/dev/null || echo "No specs"`
