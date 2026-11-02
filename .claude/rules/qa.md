## Quality gates

- **Run `/qa` before finishing any coding task** and report the result. After every answer with code changes the Stop hook runs only the fast gates for the areas that changed (rustfmt and clippy for `crates/`, `i18n/`, `Cargo.toml`); tests and the license-notice check run in `/qa` and in CI.
- **The gates are Linux's share of CI.** The Windows (Direct3D/WARP, benchmarks) and macOS (Metal, app bundle) jobs run in CI only. Platform code under `crates/blinzel-app/src/windows/` or `macos/` is green locally only once it is green in CI too; at least `cargo xwin check --target x86_64-pc-windows-msvc` checks Windows code from Linux where it is installed.
- **Fix the cause, never lower the gates.** No new suppressions (`#[allow(…)]`, `#[expect(…)]`), no weakened, skipped or `#[ignore]`d tests (AGENTS.md §9). The guard hook refuses new suppressions; a justified one is added by the user.
- **Don't loosen the gate configuration**: `.claude/settings.json`, `.claude/hooks/`, `.claude/gates.json` change only on the user's explicit request (editing them asks for permission).
- When the Stop hook blocks, the error is in its message. Where a gate names a `fix` command (`cargo fmt --all`, regenerating the license notices), that fix is allowed; everything else is fixed in the code.
- `STOP_GATE=off` in the environment switches the Stop hook off, e.g. for a bulk migration — only on the user's request.
