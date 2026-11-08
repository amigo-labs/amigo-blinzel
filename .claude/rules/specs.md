## Spec-driven features

Features are built spec-driven: first a feature spec `docs/specs/<name>.md` with testable acceptance criteria, then the implementation with tests, then an independent acceptance check. The SessionStart hook lists the open feature specs at the start of every session.

Two kinds of document describe what gets built:

| Kind | Where | Owns |
| ---- | ----- | ---- |
| Design spec | `docs/specs/blinzel.md` (`status: draft`, no acceptance criteria) | Goal, decisions (§2), benchmark gate (§3), architecture, platforms, milestones, measurements (§15) |
| Feature spec | `docs/specs/<name>.md` (`status: todo` / `implemented`, `## Acceptance criteria`) | One feature: what it does, how we prove it |

A feature spec references the `blinzel.md` sections it builds on and never contradicts them. A feature that needs an architectural decision (cross-crate boundary, new platform API, new dependency, file format, public API shape) gets a row in the decisions table in `blinzel.md` §2 in the same commit as the spec (AGENTS.md §2.1). `docs/specs/README.md` indexes the feature specs: order, dependencies, done, and the backlog of gaps not yet written as specs.

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
- **Architecture decisions** → decisions table in `docs/specs/blinzel.md` §2; details in the matching section.
- **Feature-local decisions** → `## Decisions` of the feature spec, with the reason.
- **Progress of a feature** → `## Evidence` of the feature spec (there is no separate status file).
- **Measurements** → `docs/specs/blinzel.md` §15 (and `docs/race/` for the published competitive reports).
- **Never** into auto memory under `~/.claude` — it is switched off for this project because it stays on one machine.
