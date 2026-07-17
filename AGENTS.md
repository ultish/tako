# AGENTS.md

Project guidance for coding agents (Grok, Claude Code, etc.) working in this repo.
Claude Code loads this via `CLAUDE.md` (`@AGENTS.md`); edit **this file only**.

## What this is

**tako** (蛸 / octopus) — a terminal UI (ratatui) multi-service **Gradle + Skaffold**
control plane: discover projects under scan roots, parse the dependency graph
(catalog + build files), run coordinated `gradle` / `skaffold` / `git` actions,
and cascade publish → rebuild → redeploy across consumers. Complements **rakko**
(Kafka TUI): tako ships code; rakko inspects the bus.

Read `README.md` for a short human overview and **`SPEC.md` for the design-of-record**
(architecture, monorepo multi-skaffold, git rules, cascade recipes, milestones,
UI kit requirements). Update `SPEC.md` when architecture decisions change — don't
let it silently drift from the code.

## Layout

- `src/config/` — load/save under **`~/.config/tako/`** (constructed manually,
  not via a platform-native path). Workspace roots, scan ignores, gradle/skaffold
  defaults, `[ui]` theme / banner prefs.
- `src/scan/` — walk roots; detect projects (skaffold entrypoints + publishable
  Gradle modules); monorepo multi-skaffold → one inventory row per entrypoint.
- `src/gradle/` — catalog TOML, light `build.gradle.kts` extraction, version
  ranges, coordinates / edges.
- `src/git/` — branch / dirty (per `git_root`); ff-only pull (no merge UI).
- `src/skaffold/` — detect `skaffold.yaml` / `.yml`; actions always use that
  file's cwd / `-f`.
- `src/kube/` — phase 2: deployed-version probes via `kubectl` (read-only);
  drift + soft Argo ownership hint.
- `src/graph/` — assemble graph; dependents / cascade query.
- `src/exec/` — invoke `gradle` / `skaffold` / `git`; recipes → plan → runner
  (job logs, cancel). Prefer **`gradle` on PATH**, never require `./gradlew`.
- `src/app/` / event loop — Elm-style `App` / `Action` / `AppEvent` / `Command`
  (mirror rakko). Background I/O never blocks the render loop.
- `src/ui/` — **ported from rakko** (theme, banner, splash, help, toasts, mouse,
  widgets), then tako domain screens (browser, detail, plan, console, workspace).
- `assets/tako.jpeg` — splash art (truecolor, scaled large; braille fallback).
- `Dockerfile.rhel9` + `scripts/build-tui-rhel9.sh` — airgap Linux/amd64 release
  build (Rocky 9 builder; plain Rust binary — no cmake/OpenSSL/rdkafka). Output:
  `dist/tako-linux-amd64.tar.gz`, `dist/tako`, `dist/SHA256SUMS`, `dist/ldd.txt`.
- `scripts/build-macos.sh` — native macOS release build (`cargo build --release`
  + packaging). Output: `dist/tako-macos-<arch>.tar.gz`, `dist/tako-macos-<arch>`,
  `dist/SHA256SUMS` (merged, not clobbered), `dist/otool-macos-<arch>.txt`.
- `config.example.toml` — sample config for `~/.config/tako/config.toml`.
- `docker/nexus/` — optional local Nexus for publish experiments (not required
  for the TUI itself).
- `tests/fixtures/` — tiny fake repos (catalog ranges, multi-skaffold monorepo).

## Hard constraints (don't break these)

- **Config path is `~/.config/tako/`** on both macOS and Linux, not
  `~/Library/Application Support` — deliberate, same as rakko; don't "fix" it
  to be more macOS-native.
- **Gradle is `gradle` on PATH**, not `./gradlew`. Air-gapped machines use a
  preinstalled Gradle; do not download wrappers or assume the wrapper exists.
- **UI comes from rakko** — theme, banner (`A`), splash, toasts, mouse, help
  (`?`), table_nav, footer, confirm_dialog, editor_pane, view_switcher. Do not
  invent a second visual language; port then rebrand. SPEC § *UI kit — reuse
  rakko* is authoritative.
- **No in-process Kubernetes control plane for v1.** tako invokes CLIs (`skaffold`,
  later `kubectl` for phase 2). User owns kube context / port-forwards. Phase 2
  cluster version compare is optional and separate.
- **Background I/O never blocks the render loop.** Long `gradle` / `skaffold dev`
  / `git pull` work runs off the UI thread with cooperative cancel and piped logs.
- **Git pull is ff-only / clean merge only** — on conflict, mark failed and hand
  back to the user; no conflict-resolution UI.
- **Do not commit `dist/`** — release binaries attach to GitHub Releases only.

## Before you finish a change

- `cargo test` — pure-logic tests (config, graph, ranges, catalog fixtures);
  no live cluster / Nexus required for the default suite.
- `cargo build` / `cargo clippy` — should stay warning-clean modulo expected
  dead-code on not-yet-wired pieces.
- Optional later: `#[ignore]`d integration tests that need `gradle` / `skaffold`
  on PATH — keep them opt-in so plain `cargo test` stays offline-friendly.
- **User-facing changes:** update `CHANGELOG.md` under `[Unreleased]` (see
  **Changelog** below). Skip pure refactors, test-only, and docs-only work unless
  the user-facing surface changed.

## Changelog

`CHANGELOG.md` is the human-readable history of **user-visible** changes. Format:
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) sections under SemVer
headings. Git is the source of *what happened*; the changelog is the curated
summary — **not** a paste of commit subjects.

### Day-to-day (feature / fix work)

1. **After** the change is implemented (or as part of the same commit), open
   `CHANGELOG.md` and add bullets under **`## [Unreleased]`**.
2. Pick a section (add the heading if missing):
   - `### Added` — new capability or UI surface
   - `### Changed` — behavior change users will notice
   - `### Fixed` — bug fix
   - `### Removed` — removed feature / keybind / config key
   - `### Deprecated` / `### Security` — as needed
3. Write **user-facing** bullets (what a user would care about), not implementation
   notes. Good: `Cascade plan shows consumer set before confirm.`
   Bad: `Wire Action::ConfirmPlan in apply_event().`
4. Derive bullets from the work just done **and** from git when catching up:
   ```bash
   # Since last tagged release (or since main diverged):
   git log vX.Y.Z..HEAD --oneline
   # Or unreleased commits on this branch:
   git log --oneline -20
   ```
   Skim commits/diffs, then **rewrite** into short product language. Do not dump
   `git log` verbatim into the changelog.
5. Keep `[Unreleased]` ordered roughly newest-first within each subsection is fine;
   merge related bullets instead of one line per commit.
6. Prefer updating the changelog **in the same commit** as the feature/fix so
   `git log -p -- CHANGELOG.md` stays aligned with history. If you forget, a
   follow-up commit that only edits `CHANGELOG.md` is OK — mention the related
   change in the commit message.

### When cutting a release

1. Bump `version` in `Cargo.toml` (see release checklist).
2. In `CHANGELOG.md`:
   - Rename `## [Unreleased]` content into
     `## [X.Y.Z] - YYYY-MM-DD` (today’s date, ISO).
   - Leave a fresh empty `## [Unreleased]` at the top (no bullets yet).
3. Use the new section (or a short headline distilled from it) as
   `gh release create --notes`. Prefer the changelog body over inventing notes
   from scratch:
   ```bash
   gh release create vX.Y.Z \
     --title "vX.Y.Z — <headline>" \
     --notes-file <(sed -n '/## \[X.Y.Z\]/,/## \[/p' CHANGELOG.md | sed '$d') \
     dist/tako-linux-amd64.tar.gz \
     dist/tako-macos-<arch>.tar.gz \
     dist/SHA256SUMS
   ```
4. Commit version bump + changelog together when possible
   (`Release vX.Y.Z` or similar).

### What not to changelog

- Internal refactors with no user-visible behavior change
- Test-only or CI-only changes
- Typo fixes in comments / agent docs (`AGENTS.md`) unless they document a
  product change

## Release checklist (when cutting a version)

**Trigger:** a version bump in `Cargo.toml`'s `git diff` — whether you wrote it or
not — means a release is being cut. Run this checklist before committing. Ownership
of the commit implies ownership of the checklist; "someone else bumped it" is not an
exception.

1. **Bump `version` in `Cargo.toml`.** SemVer: bug fixes → patch, backward-compatible
   features → minor, breaking changes to the config format / CLI / architecture →
   major. Run `cargo build` once after bumping so `Cargo.lock` picks it up.
2. **Update `CHANGELOG.md`:** move `[Unreleased]` bullets into
   `## [X.Y.Z] - YYYY-MM-DD` and reset `[Unreleased]` (see **Changelog** above).
3. **RHEL 9 / airgap Linux release asset (do not skip).** Air-gapped users install
   the prebuilt binary; a version cut without it leaves them stranded.
   ```bash
   ./scripts/build-tui-rhel9.sh
   ```
   Prefer `DOCKER=docker` when the daemon is up; otherwise `DOCKER=container` on
   Apple Silicon. First build downloads crates under qemu if on Apple Silicon.
   - Confirm artifacts exist and look right:
     - `dist/tako-linux-amd64.tar.gz` — **primary GitHub Release asset**
     - `dist/SHA256SUMS`
     - `dist/ldd.txt` — expect glibc / libgcc_s only (no surprise dynamic deps)
     - `file dist/tako-linux-amd64` → ELF 64-bit **x86-64** (not arm64)
   - Do **not** commit `dist/` (gitignored) — it's attached to the Release only, in
     step 5.
4. **macOS release asset (do not skip).** Generate it and attach it to every release,
   same as the RHEL 9 asset.
   ```bash
   ./scripts/build-macos.sh
   ```
   Native build (no container) — runs on whatever Mac you're on, producing that
   Mac's architecture (`arm64` or `x86_64`).
   - Confirm artifacts exist and look right:
     - `dist/tako-macos-<arch>.tar.gz`
     - `dist/SHA256SUMS` — merged with the RHEL 9 entries, not clobbered
     - `dist/otool-macos-<arch>.txt` — system frameworks + libSystem only
5. **Commit, tag, and cut the GitHub Release** — no CI; releases are 100% manual.
   Committing and pushing does **not** create a release; a Release only exists once
   `gh release create` runs. Every version bump gets a matching git tag *and* a
   GitHub Release.
   - Commit the release (include `Cargo.toml`, `Cargo.lock` if changed,
     `CHANGELOG.md`), push to `main`.
   - Tag the release commit and push the tag:
     ```bash
     git tag -a v<X.Y.Z> -m "Release <X.Y.Z>: <headline>"
     git push origin v<X.Y.Z>
     ```
   - Create the Release, attaching the dist assets; **prefer notes from
     `CHANGELOG.md`** for that version (see Changelog → When cutting a release):
     ```bash
     gh release create v<X.Y.Z> \
       --title "v<X.Y.Z> — <headline>" \
       --notes "<from CHANGELOG.md [X.Y.Z] section>" \
       dist/tako-linux-amd64.tar.gz \
       dist/tako-macos-<arch>.tar.gz \
       dist/SHA256SUMS
     ```
   - Verify: `gh release view v<X.Y.Z>` shows all three assets and `isDraft: false`.
