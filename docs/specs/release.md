---
status: todo
created: 2026-11-02
size: M
platforms: all
design: [blinzel.md §2, blinzel.md §9, blinzel.md §10, blinzel.md §12 (M7), blinzel.md §13]
---

# Release automation

## Context

Releases are cut without manual steps: release-please turns the Conventional Commits on `main` into a release PR; merging it tags the version and creates a draft release, and `release.yml` builds every platform, writes and signs `SHA256SUMS`, attaches the files and publishes the release. Only then do the self-updaters see it (`docs/specs/updates.md`). Nothing carries a code signing certificate from a CA (`blinzel.md` §2, "Signature"); the project's own Ed25519 key signs the checksums. The secrets are listed in `docs/SECRETS.md`. This spec records what the workflows and `blinzel-release` do today (reverse engineered on 2026-11-02).

## Scope

### In scope

- `.github/workflows/release-please.yml`, `release-please-config.json`, `.release-please-manifest.json`
- `.github/workflows/release.yml`: builds, assets and their names, checksums, signing, publishing
- `blinzel-release keygen | sign | verify | notices` (the `macos-bundle` / `macos-dmg` commands are in `docs/specs/installation.md`)
- The third-party notices per platform and their CI check
- How the secrets `RELEASE_SIGNING_KEY`, `MACOS_SIGNING_CERT`, `MACOS_SIGNING_CERT_PASSWORD` and `RELEASE_PLEASE_TOKEN` are used

### Out of scope

- The winget submission — because `packaging/winget/` holds templates that a person fills and submits after a release (`packaging/winget/README.md`); no workflow does it
- Notarization and Authenticode — because releases deliberately carry no CA certificate (`blinzel.md` §2)
- The website deployment (`website.yml`, Cloudflare secrets) — because the website has its own spec
- Creating the Mac signing certificate (`packaging/macos/create-signing-cert.sh`) — because it is a one-time operator task documented in `docs/SECRETS.md`

## Acceptance criteria

### Release PR and tag

- [ ] AC-01 (event, manual: runs only on GitHub): When commits land on `main`, then release-please opens or updates one release PR titled `chore: release amigo-blinzel <version>` that sets the workspace version in `Cargo.toml` and adds the `CHANGELOG.md` entry (sections Features, Bug Fixes, Performance Improvements, Reverts; `docs`, `refactor`, `test`, `build`, `ci`, `chore` hidden), and the workflow pushes a matching `Cargo.lock` to that PR when it changed.
- [ ] AC-02 (event, manual: runs only on GitHub): When the release PR is merged, then release-please tags `v<version>`, creates the GitHub release as a draft with the changelog, and calls `release.yml` with that tag; `release.yml` can also be started by hand for an existing tag.
- [ ] AC-03 (error, CI: release): If the tag differs from `v` plus the workspace version in `Cargo.toml`, then the Windows build jobs fail with `tag <tag> != v<version>` and nothing is published.

### Assets

- [ ] AC-04 (always, CI: release): The published release carries `amigo-blinzel-setup-x64.exe`, `amigo-blinzel-setup-arm64.exe`, `amigo-blinzel-linux-x64`, `amigo-blinzel-linux-arm64`, `amigo-blinzel-linux-x64.tar.gz`, `amigo-blinzel-linux-arm64.tar.gz`, `amigo-blinzel-macos.dmg`, `amigo-blinzel-macos.zip`, `SHA256SUMS`, `SHA256SUMS.sig`, `THIRD-PARTY-NOTICES.txt`, `THIRD-PARTY-NOTICES-linux.txt` and `THIRD-PARTY-NOTICES-macos.txt`.
- [ ] AC-05 (always, CI: release): Each setup executable is the viewer `blinzel.exe` built for that architecture, renamed; each `amigo-blinzel-linux-<arch>` is the bare viewer executable, and its `.tar.gz` holds a folder `amigo-blinzel/` with that executable, `LICENSE` and `THIRD-PARTY-NOTICES-linux.txt`.
- [ ] AC-06 (always, CI: release): The macOS disk image and zip both hold the same universal `amigo-blinzel.app`; the zip stays in every release because installed copies update from that name.
- [ ] AC-07 (always, CI: release): `SHA256SUMS` lists, in `sha256sum` format, the SHA-256 of every release file whose name starts with `amigo-blinzel-`.

### Signing

- [ ] AC-08 (error, CI: release): If the secret `RELEASE_SIGNING_KEY` is empty, then the step "Sign SHA256SUMS" fails with `RELEASE_SIGNING_KEY secret is missing` and nothing is published.
- [ ] AC-09 (event, CI: release): When `blinzel-release sign <file> <out.sig>` runs with `RELEASE_SIGNING_KEY` set to a 64-digit hex seed, then it writes the Ed25519 signature as 128 hex digits plus a newline, after checking that the signature verifies, and prints the public key.
- [ ] AC-10 (error): If `RELEASE_SIGNING_KEY` is unset or not a 64-digit hex seed, then `blinzel-release sign` exits with 1 and writes no signature.
- [ ] AC-11 (event): When `blinzel-release verify <file> <sig> <public-key-hex>` runs, then it prints `valid` and exits with 0 for a matching signature, and prints `INVALID signature` and exits with 1 otherwise.
- [ ] AC-12 (event): When `blinzel-release keygen <file>` runs, then it writes a new seed as hex to `<file>`, refuses to overwrite an existing file, and prints the public key as hex and as the `RELEASE_KEY` line for `blinzel-core/src/update.rs`.
- [ ] AC-13 (option, CI: release): Where the secret `MACOS_SIGNING_CERT` is set, the macOS job imports the `.p12` into a temporary keychain and signs the app with the certificate it contains (by SHA-1 hash, whatever its name); otherwise it prints `MACOS_SIGNING_CERT is not set: signing ad hoc` and signs ad hoc; either way `codesign --verify` must accept the app.
- [ ] AC-14 (state, manual: needs the organization secret and a release run): While `MACOS_SIGNING_CERT` holds the amigo-labs certificate, the released app's signature shows `Authority=amigo-labs`.

### Publishing

- [ ] AC-15 (event, CI: release): When all build jobs succeeded, then the files are uploaded to the tag's release first (a draft named `amigo-blinzel <version>` is created if the tag has none), and only then is the release published as the latest; files of an earlier run for the same tag are replaced.

### Third-party notices

- [ ] AC-16 (always): `blinzel-release notices --check` fails with `<file> is out of date: run \`cargo run -p blinzel-release -- notices\`` when any of `THIRD-PARTY-NOTICES.txt` (Windows), `THIRD-PARTY-NOTICES-linux.txt` or `THIRD-PARTY-NOTICES-macos.txt` differs from what the generator writes for the locked dependencies (line endings ignored).
- [ ] AC-17 (error): If a linked crate's licence expression has no option among MIT, ISC, Zlib, BSD-2-Clause, BSD-3-Clause, Apache-2.0, Unlicense, 0BSD, Unicode-3.0 and CC0-1.0 (for example GPL-only, or only a `WITH` exception), then the generator stops; of an `OR` choice it takes the most preferred option in that order, and `AND` keeps all parts.
- [ ] AC-18 (always): Each platform's viewer shows its platform's notices file under the menu entry "Third-party notices".

## Technical notes

- Runners: Windows jobs on `windows-2025` (x64 native, ARM64 as target `aarch64-pc-windows-msvc`), Linux on `ubuntu-22.04` and `ubuntu-22.04-arm` (glibc 2.35 is the minimum), macOS on `macos-14` with `MACOSX_DEPLOYMENT_TARGET=14.0`, building `aarch64-apple-darwin` and `x86_64-apple-darwin` and joining them with `blinzel-release macos-bundle`. All builds use `cargo build --release --locked`; NASM is installed for the x86-64 assembly.
- The macOS job zips the bundle with `ditto -c -k --keepParent`, builds the DMG with `blinzel-release macos-dmg` and runs `hdiutil verify`; it prints `lipo -info` and the `Authority=` / `Signature=` lines of `codesign --display`.
- The publish job runs on `windows-2025` with `contents: write`; every other job has `contents: read`. Third-party actions handling the signing key are pinned to commits (`ilammy/setup-nasm`). The release uses `github.token` (`GH_TOKEN`) for `gh release create | upload --clobber | edit --draft=false --latest`.
- release-please: `release-type: simple`, `include-v-in-tag`, `draft: true`, `force-tag-creation`, `bump-minor-pre-major: true`, `bump-patch-for-minor-pre-major: false`, the version in `Cargo.toml` via `extra-files` (`$.workspace.package.version`), `bootstrap-sha` 5eb2dcb (merge of PR #22). The manifest is at 1.3.0. Token: `RELEASE_PLEASE_TOKEN` if set, else `github.token` (then no CI runs on the release PR).
- Notices: `crates/blinzel-release/src/notices.rs`. The crate list comes from `cargo tree --locked -p blinzel-app -e normal,no-proc-macro` for both targets of a platform (default features, so `heic` is not covered); the texts from the crates' sources in Cargo's registry, plus libjpeg-turbo, libwebp and Little CMS from their `-sys` crates and the Rust standard library. The viewer embeds the file with `include_str!` (`crates/blinzel-app/src/notices_text.rs`). `ci.yml`'s Linux job runs `cargo run --locked -p blinzel-release -- notices --check` ("Third-party notices up to date").
- The public key built into the app is `RELEASE_KEY` in `crates/blinzel-core/src/update.rs`; the seed lives only in the repository secret `RELEASE_SIGNING_KEY` (`docs/SECRETS.md`).

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| A push to `main` with only `docs`/`ci`/`chore` commits | No release PR change that bumps the version |
| `release.yml` run by hand for a tag that has no release | It creates a draft release with generated notes, uploads, publishes |
| `release.yml` run again for a published tag | Files are replaced (`--clobber`), the release stays published and latest |
| A dependency update without regenerated notices | The Linux CI job fails at "Third-party notices up to date" |
| `blinzel-release keygen` pointed at an existing file | Fails, the existing key stays |
| `MACOS_SIGNING_CERT` set but holds no certificate | The macOS job fails with `no certificate in MACOS_SIGNING_CERT` |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 |      | open   |        |
| AC-02 |      | open   |        |
| AC-03 | `release.yml` job "Windows" step "Tag matches the crate version" | built |        |
| AC-04 | `release.yml` "Attach the files and publish" uploads them; no step asserts the list (a missing `amigo-blinzel-*` file goes unnoticed) | open   |        |
| AC-05 |      | open   |        |
| AC-06 |      | open   |        |
| AC-07 |      | open   |        |
| AC-08 | `release.yml` job "Checksums and release" step "Sign SHA256SUMS" (`test -n "$RELEASE_SIGNING_KEY"`) | built |        |
| AC-09 | `release.yml` step "Sign SHA256SUMS" (`sign` fails when its signature does not verify); the output format is not asserted | open   |        |
| AC-10 |      | open   |        |
| AC-11 |      | open   |        |
| AC-12 |      | open   |        |
| AC-13 | `release.yml` macOS step "App bundle and disk image" (`codesign --verify`); `signs_ad_hoc_without_an_identity` (`crates/blinzel-release/src/macos.rs`) for the identity choice | built |        |
| AC-14 |      | open   |        |
| AC-15 |      | open   |        |
| AC-16 | `ci.yml` Linux job step "Third-party notices up to date" (`notices --check`); a stale file is not provoked by a test | built |        |
| AC-17 | `alternatives_prefer_mit_and_reject_unknown` (`crates/blinzel-release/src/notices.rs`), Linux CI | built |        |
| AC-18 |      | open   |        |

## Decisions

