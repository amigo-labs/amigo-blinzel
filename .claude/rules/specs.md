## Spec-driven features

Features are built spec-driven: first a feature spec `docs/specs/<name>.md` with testable acceptance criteria, then the implementation with tests, then an independent acceptance check. The SessionStart hook lists the open feature specs at the start of every session.

Two kinds of document describe what gets built:

| Kind | Where | Owns |
| ---- | ----- | ---- |
| Plan | `docs/PLAN.md` (sections, status per item, no acceptance criteria) | Goal, architecture, platforms, measurements, the decisions table (`PLAN.md` §2) |
| Feature spec | `docs/specs/<name>.md` (`status: todo` / `implemented`, `## Acceptance criteria`) | One feature: what it does, how we prove it |

A feature spec references the `PLAN.md` sections it builds on and never contradicts them. A feature that needs an architectural decision (cross-crate boundary, new platform API, new dependency, file format, public API shape) gets a row in the decisions table in `PLAN.md` §2 in the same commit as the spec (AGENTS.md §2.1). `docs/specs/README.md` indexes the feature specs: order, dependencies, done, and the backlog of gaps not yet written as specs.

## Triage

Classify every new input before touching code:

| Input | Approach |
| ----- | -------- |
| New feature or changed behavior ("I want …", "it should …") | Start `/spec`, no code before the spec. If the user says "just do it", implement directly. |
| Request touches an existing feature spec | Revise that spec in `/spec`'s revision mode, never create a second one |
| Unclear whether small change or feature | Exactly one question (AskUserQuestion): "Capture as spec" / "Implement directly" |
| Bug fix | Directly, always with a regression test that is red before the fix. If the fix changes specified behavior, revise the spec too. |
| Refactoring, tooling, docs, questions | Directly, no spec |
| "Carry on", "build X", "implement the spec" | `/spec-build <name>` |
| "Is X done?", acceptance after manual implementation | `/spec-check <name>` |
| Document existing behavior | `/spec-extract <area>` |

Only `/spec-check` sets a spec to `status: implemented` — never by hand.

## Keeping knowledge

Project knowledge belongs in the repository, not on one machine:

- **Pitfalls and non-obvious knowledge** → `## Gotchas` in `AGENTS.md`. Only what cannot be read from code or git history; no noise.
- **Architecture decisions** → decisions table in `docs/PLAN.md` §2; details in the matching `PLAN.md` section.
- **Feature-local decisions** → `## Decisions` of the feature spec, with the reason.
- **Progress of a feature** → `## Evidence` of the feature spec (there is no separate status file).
- **Measurements** → `docs/PLAN.md` §15 (and `website/data/` for the published ones).
- **Never** into auto memory under `~/.claude` — it is switched off for this project because it stays on one machine.
