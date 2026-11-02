---
status: todo
created: 2026-11-02
size: M
platforms: all
design: [blinzel.md §2, blinzel.md §11, blinzel.md §12 (M8)]
---

# Website

## Context

amigo-blinzel has to prove on a public website that it is faster than other viewers (`blinzel.md` §1). Decided in `blinzel.md` §2: the website is static assets on Cloudflare Workers, with the competitive comparison and benchmarks. `crates/blinzel-site` generates the site from `website/src` (one layout, one HTML fragment per page and language, stylesheet, two small scripts) and the competitive benchmark reports `website/data/race.json` (Windows) and `website/data/race-macos.json` (macOS) written by `blinzel-race` (see `competitive-benchmark.md`). Charts are inline SVG generated at build time; the browser runs no chart library.

The site is a one-page site per language (German, English) plus a privacy page. The separate pages the design spec lists (features, comparison, benchmarks, methodology, download, changelog) are sections or collapsible blocks of the start page; their old addresses redirect there, and "changelog" points to the GitHub releases page instead of a page generated from `CHANGELOG.md`.

## Scope

### In scope

- The generator `blinzel-site`: pages, placeholders, charts and tables from the race reports, sitemap, robots.txt, copied static files.
- The page sources in `website/src`: layout, start page and privacy page in German and English, redirects, headers, the root language redirect, the download-button script.
- Deployment: `wrangler.toml`, `website/build.sh`, the optional `.github/workflows/website.yml`.

### Out of scope

- The measurement that produces `race.json` / `race-macos.json` — because `blinzel-race` has its own spec (`competitive-benchmark.md`).
- The content of the hand-written feature comparison tables (Windows viewers, macOS viewers, batch tools) — because they are prose with sources that a person proofreads, not generated from data.
- A legal notice (Impressum) page — because only the operator can provide its details (`README.md`, open items outside the code); it does not exist yet.
- Charts from `blinzel-bench` or `blinzel --bench` JSON — because the generator only reads the two race reports.

## Acceptance criteria

- [ ] AC-01 (always): A build produces, for German and English each, the start page and the privacy page with every `{{…}}` placeholder of layout and fragment filled.
- [ ] AC-02 (always): Every generated page has exactly one `<h1>` (a fragment without its own gets the page title as heading), one meta description, a canonical link to its own absolute URL, `hreflang` links to itself in both languages, and a link to the privacy page of its language.
- [ ] AC-03 (always): Every generated page links the GitHub releases page as the changelog ("Änderungen" / "Changes") and the same page in the other language.
- [ ] AC-04 (always): The start page of each language contains the sections `features`, `measured` and `download` that the navigation bar links to.
- [ ] AC-05 (always): Every redirect of an old page address (`/de/features`, `/en/benchmarks`, …) that points to an anchor points to an anchor that exists on the start page of that language.
- [ ] AC-06 (always): No generated page contains an inline script; every `<script>` loads a file, so the site works under its Content-Security-Policy `script-src 'self'`.
- [ ] AC-07 (state): While a race report file is absent, each of its placeholders shows the notice that the reference measurement is still to come, and the report is not published.
- [ ] AC-08 (event): When a race report file exists, its chart placeholders become inline SVG bar charts, its table placeholder a table of every viewer with version and median (p95) for open warm, open cold and next image that links the raw report, and the report is published at `/race.json` or `/race-macos.json`.
- [ ] AC-09 (always): Time charts list the fastest viewer first, the key-repeat chart the viewer with the highest share of images shown first ("shown / presses"); no viewer is highlighted; times of 1000 ms and more read as seconds with two decimals and the language's decimal separator, shorter ones as whole milliseconds.
- [ ] AC-10 (state): While no viewer in a report has a value for a metric, that chart reads "Nicht gemessen." / "Not measured."
- [ ] AC-11 (always): The machine block lists every entry of the report's `machine` object, labelled in the page language where the key is known and with the raw key otherwise.
- [ ] AC-12 (error): If a page uses an unknown placeholder, an unknown race chart kind or an unclosed `{{`, then the build fails.
- [ ] AC-13 (error): If a page fragment does not start with the `<!-- title: … -->` and `<!-- description: … -->` lines, then the build fails.
- [ ] AC-14 (error): If a race report file exists but is not valid JSON, then the build fails naming the file.
- [ ] AC-15 (always): The download placeholders resolve to the stable `releases/latest/download/` URLs of `amigo-blinzel-setup-x64.exe`, `amigo-blinzel-setup-arm64.exe`, `amigo-blinzel-macos.dmg`, `amigo-blinzel-linux-x64.tar.gz`, `amigo-blinzel-linux-arm64.tar.gz` and `SHA256SUMS`.
- [ ] AC-16 (event): When the start page is opened in a desktop browser on Windows (x64 or ARM64), macOS or Linux (x64 or ARM64), then the main download button links that platform's file with a matching label; on phones, tablets, ChromeOS and unknown systems it keeps linking the download section.
- [ ] AC-17 (event): When the site root `/` is opened, then the browser goes to `/de/` if its language starts with "de", otherwise to `/en/`.
- [ ] AC-18 (always): The build writes a `sitemap.xml` with one entry per page and language, each listing both language versions, and a `robots.txt` that names the sitemap.
- [ ] AC-19 (event, manual: runs on Cloudflare): When Cloudflare Workers Builds deploys the repository, then it runs `website/build.sh`, which installs the toolchain pinned in `rust-toolchain.toml` if missing and generates `website/dist`, and serves it with the 404 page for unknown paths.
- [ ] AC-20 (option, manual: needs the Cloudflare secrets): Where `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` are set, the website workflow deploys the built site on a push to `main` that touches the website, `blinzel-site`, `CHANGELOG.md` or `wrangler.toml`, and on a published release; without them it builds and ends successfully without deploying.
- [ ] AC-21 (always, manual: visual design): The pages, charts and tables are legible on a phone and a desktop.

## Technical notes

- Generator: `crates/blinzel-site/src/main.rs` (`build`, `expand`, `placeholder`, `render`, `sitemap`), charts in `crates/blinzel-site/src/charts.rs` (`race`, `bars`). Usage `cargo run -p blinzel-site [-- <website-dir>]`, default `website`; `website/dist` is deleted and rebuilt on every run.
- Pages: `PAGES = ["index", "privacy"]`, navigation `SECTIONS = ["features", "measured", "download"]`. Fragments are `website/src/{de,en}/{slug}.html`; URLs `/{lang}/` and `/{lang}/privacy`. Canonical base `SITE = "https://blinzel.amigo-labs.dev"`.
- Placeholders: `{{version}}` (crate version), `{{download:x64|arm64|macos|linux-x64|linux-arm64|sums}}`, `{{race:<kind>}}` and `{{race-macos:<kind>}}` with kind `open_warm`, `open_cold`, `next`, `burst`, `table`, `machine`.
- Report fields read: `machine` (object), `viewers[].name`, `.version`, `.open_warm|open_cold|next.median_ms|p95_ms`, `.burst.presses`, `.burst.shown.median_ms`.
- Static files copied when present: `style.css`, `site.js`, `lang.js`, `favicon.svg`, `og.png`, `index.html`, `404.html`, `_headers`, `_redirects`.
- `wrangler.toml`: `[build] command` runs `website/build.sh` via `git rev-parse --show-toplevel`; `[assets] directory = "./website/dist"`, `not_found_handling = "404-page"`, `html_handling = "auto-trailing-slash"`; empty `[previews]` for branch builds.
- Start page content: hero, `#features` (viewing, batch resizing, screenshots, own decoders, system decoders), `#measured` (Windows charts, macOS in `<details>`, all values and machines, `#method` "How we measure", `#compare` Windows and macOS feature tables, `#batch-compare` batch tool table), `#download` (Windows, macOS, Linux, first start, checksums, updates).

## Edge cases

| Scenario | Expected behavior |
| -------- | ----------------- |
| Neither race report exists | Every race placeholder shows the "measurement still to come" notice; the build succeeds |
| Only `race.json` exists | Windows charts render; macOS placeholders show the notice; only `race.json` is published |
| A viewer has `open_cold: null` (no `--cold` run) | It is left out of the cold chart; its table cell reads "–" |
| A viewer's burst has `presses: 0` or no `shown` | It is left out of the key-repeat chart |
| A viewer name contains `<`, `&` or `"` | It is escaped in SVG and table |
| An old address such as `/en/changelog` | 301 to the GitHub releases page |

## Open questions

<!-- One bullet per open question; empty means: everything settled, spec-build may start -->

## Evidence

| AC    | Test | Status | Commit |
| ----- | ---- | ------ | ------ |
| AC-01 | `builds_the_site` (`crates/blinzel-site/src/main.rs`, Linux CI) | built | |
| AC-02 | `builds_the_site` (`crates/blinzel-site/src/main.rs`, Linux CI) | built | |
| AC-03 | — (no test checks the changelog link or the language switch link) | open | |
| AC-04 | `builds_the_site` (`crates/blinzel-site/src/main.rs`, Linux CI) | built | |
| AC-05 | `builds_the_site` (`crates/blinzel-site/src/main.rs`, Linux CI) | built | |
| AC-06 | `builds_the_site` (`crates/blinzel-site/src/main.rs`, Linux CI) | built | |
| AC-07 | `builds_the_site`, `expands_placeholders` (`crates/blinzel-site/src/main.rs`, Linux CI) | built | |
| AC-08 | `builds_the_site` (`crates/blinzel-site/src/main.rs`, Linux CI); table columns: `charts_sort_without_highlight` (`crates/blinzel-site/src/charts.rs`) | built | |
| AC-09 | `charts_sort_without_highlight` (`crates/blinzel-site/src/charts.rs`, Linux CI) | built | |
| AC-10 | `charts_sort_without_highlight` (`crates/blinzel-site/src/charts.rs`, Linux CI) | built | |
| AC-11 | `charts_sort_without_highlight` (`crates/blinzel-site/src/charts.rs`, Linux CI) | built | |
| AC-12 | `expands_placeholders` (`main.rs`), `charts_sort_without_highlight` (`charts.rs`), Linux CI | built | |
| AC-13 | `pages_need_title_and_description` (`crates/blinzel-site/src/main.rs`, Linux CI) | built | |
| AC-14 | — | open | |
| AC-15 | `expands_placeholders` checks only the x64 URL | open | |
| AC-16 | — (`website/src/site.js`, no browser test) | open | |
| AC-17 | — (`website/src/lang.js`, no browser test) | open | |
| AC-18 | `builds_the_site` (entry count and file presence; `robots.txt` content not checked) | open | |
| AC-19 | manual | open | |
| AC-20 | manual | open | |
| AC-21 | manual | open | |

## Decisions

- 2026-11-02: Extracted from the code. The design spec's separate pages (features, comparison, benchmarks, methodology, download, changelog) are sections of the start page; the changelog is the GitHub releases page, not a page generated from `CHANGELOG.md`.
