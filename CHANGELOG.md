# Changelog

All notable changes to **tako** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

How agents and humans maintain this file is described in `AGENTS.md`
(section **Changelog**). Prefer user-facing bullets derived from git history
over dumping raw commit subjects.

## [Unreleased]

## [0.1.0] - 2026-07-18

First public release — multi-service Gradle + Skaffold control plane with
workspace inventory, dependency graph, cascade recipes, jobs console, and
optional Kubernetes deployed-version compare.

### Added

- **Settings tab (4)** — edit all `config.toml` fields in the TUI (scan depth/
  ignore, gradle, skaffold, git, cascade, **kube**, ui). Enter/Space toggles
  bools and opens editors; ←/→ nudges integers. Roots & excludes stay on
  Workspace (3). Enabling **kube.enabled** seeds `namespaces = ["default"]` if
  empty so **K** can probe immediately.
- **`[scan].exclude`** — drop projects from inventory by name, path fragment,
  absolute path, or simple `*` / `**` globs. Workspace (3): **Tab**
  roots/excludes · **n**/**e**/**z**. Projects: **-** excludes cursor (or
  multi-select) and rescans.
- **Phase 2 kube (M6/M7 light):** **K** probes Deployments via `kubectl`
  (read-only). Columns **Local / Deployed / Drift**. Version source default
  **`auto`**: version labels → semver image tags → **`live`** for Skaffold
  content-hash tags. Match by Deployment name, `app` label, `app.kubernetes.io/name`.
  Soft Argo ownership hint; **f** drift-only filter.
- **Cascade recipes:** **B** publish → rebuild dependents (parallel consumers,
  default 5); **P** + skaffold delete/run. Confirm plan overlay; SNAPSHOT
  refresh init script on consumers (`force_latest_snapshots`).
- **Multi-select (M5):** **Space** toggle; bulk **b**/**c**/**G**.
- **Static dependency graph (M2):** catalogs, GAV, `libs.*`, `project(":…")`,
  range matching; browser highlights deps/dependents; full-screen detail
  (**Enter**).
- **Jobs console (2):** live logs; **Tab** list/log; **Esc** cancel job.
- **Exec:** **b**/**c**/**p** gradle; **d**/**D**/**x**/**u** skaffold; **G**
  git pull (`--ff-only`).
- **Workspace (3):** scan roots n/e/z; **r** rescan; inventory cache.
- **Chrome:** theme (**T**), banner (**A**), help (**?**), splash, mouse,
  bottom status bar (not toast).

### Changed

- Project table **Skaffold** column header (was **SK**).
- **B** is publish → dependents (not build upstream deps in parallel).
- Job feedback on bottom status bar instead of covering toast.

### Fixed

- Graph resolves published version catalogs and GAV deps by coordinate or
  artifact name when Maven groups differ.

### Packaging

- RHEL 9 airgap binary (`tako-linux-amd64`) + native macOS (`tako-macos-<arch>`).
- Config path: `~/.config/tako/` on macOS and Linux.
