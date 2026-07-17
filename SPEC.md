# tako — multi-service build & skaffold control plane

**tako** (蛸, octopus): many arms, one body — one TUI that reaches across a
forest of Gradle microservices, shared libs, Avro schemas, and Skaffold
dev loops.

Japanese short animal name, same vibe as **rakko** (ラッコ / sea otter).

---

## Name alternatives (if tako doesn't stick)

| Name | Animal | Why it fits | Why not |
|------|--------|-------------|---------|
| **tako** ★ | octopus | Multi-arm control of many services; short; memorable | “Taco” pun in English |
| **kumo** | spider | Dependency *web* is the product | Easy to confuse with “cloud” (雲) |
| **ari** | ant | Colony of services, industrious builds | Less distinctive |
| **tanuki** | raccoon dog | Clever multi-shape (lib vs service vs avro) | Longer; folklore baggage |
| **risu** | squirrel | Gathers deps / caches | Weaker deploy metaphor |
| **karasu** | crow | Watches many nests | Darker tone |

**Recommendation: `tako`.** Rename the folder if you pick another; nothing else
depends on the name yet except `~/.config/tako/`.

---

## Problem

You run a **microservices** estate:

- Many Gradle projects (services + shared libs + Avro schema repos).
- **Kafka + Avro** as the event log between services.
- **Skaffold** for day-to-day `dev` / `debug` on Kubernetes.
- **Argo CD** for services you rarely touch (out of scope for the control loop,
  but projects may still appear in the inventory).
- **Nexus** for published artifacts (not `mavenLocal`). Version alignment via a
  **Gradle version catalog**, often with **ranges** like `[1.0.0, 2.0.0)`.

Pain today:

1. Changing something that touches **N services** means `cd` + `skaffold dev`
   (or rebuild) in **N folders**.
2. Changing an **Avro** repo means rebuild/redeploy every service that depends
   on that schema artifact (and sometimes downstream of those).
3. Releasing a **shared lib** means: build → publish to Nexus → then rebuild
   (and often skaffold delete/run) every consumer.
4. Dependency truth lives in `build.gradle.kts` + catalog TOML, not in your head.

**tako** is a ratatui TUI that **discovers** that graph, **remembers** it, and
runs **coordinated Gradle + Skaffold** actions across the right subset of
projects.

---

## Goals

1. Point at one or more **root folders**; discover projects that look like
   deployable/buildable apps (Skaffold + Gradle).
2. Parse **Gradle** metadata (`build.gradle.kts`, settings, **version catalog**)
   to infer **edges** between projects and published libs (including range
   consumers).
3. Show a browsable inventory **grouped by parent folder** (and later by type:
   service / lib / avro), including each project’s **Gradle version** and
   **current git branch**.
4. Persist discovery + graph + UI prefs under **`~/.config/tako/`** (same
   deliberate path style as rakko — not macOS Application Support).
5. From the TUI: **Gradle** `clean` / `build` / `publish` (and related tasks);
   **Skaffold** `dev` / `debug` / `delete` / `run` (and optionally `build`);
   **git pull** (fast-forward / clean merge only — see Git).
6. On selection: **highlight dependents / dependencies** in the graph.
7. **Cascade actions**: e.g. publish lib X → build all services that depend on
   X → skaffold delete + run (or dev) those services.
8. Stay a **plain external tool**: invoke CLIs you already use
   (`gradle` on `PATH`, `skaffold`, `git`, and later `kubectl`). You own
   kube context / port-forwards. **Do not require `./gradlew`** — air-gapped
   machines use a preinstalled Gradle, not wrapper downloads.
9. **Phase 2:** read **currently deployed versions** from the cluster and
   compare them to local Gradle versions so drift is obvious and updates are
   intentional.

## Non-goals (v1)

- Replacing Argo CD / GitOps promotion to higher environments.
- Being a full Gradle IDE (source editing, debugger UI).
- Being a full git client (commit, rebase, conflict resolution UI) — on
  conflict, **abort and hand back to the user**.
- Resolving every possible Gradle multi-module edge case on day one
  (composite builds, included builds, exotic plugins) — start with the
  patterns *you* use, expand with fixtures.
- Implementing Nexus; only **publish** via Gradle and **consume** versions
  already expressed in catalogs / dependency declarations.
- Auto-bumping catalog versions in git (may be a later “propose PR” feature).
- Kafka runtime management (that’s rakko’s job).
- **Cluster inventory / deployed-version compare** (that’s **phase 2**, not v1).

---

## Personas & primary flows

### Flow A — “I changed service Foo”

1. Open tako → select Foo (or multi-select).
2. Run **build** and/or **skaffold dev** / **debug** without leaving the TUI.
3. Optionally stop/delete when done.

### Flow B — “I published shared-lib 1.4.0”

1. Select shared-lib → **clean / build / publish**.
2. Tako computes **consumers** (catalog range match + direct project deps).
3. Confirm cascade: **gradle build** each consumer → **skaffold delete** →
   **skaffold run** (or **dev** for a focused subset).
4. Progress / logs visible per project; failures don’t silently skip the rest
   without a summary.

### Flow C — “I changed avro-payments schemas”

1. Select avro-payments → publish (or build+publish).
2. Cascade to every service whose catalog/coords depend on that artifact
   (and optionally second-hop if another lib re-exports — v2).
3. Same build + skaffold path as Flow B.

### Flow D — “Cold start on a new machine”

1. Point at `~/Developer` (or a monorepo root list).
2. Scan → review discovered projects → save workspace.
3. Next launch: load cache, optional re-scan / refresh.

### Flow E — “Pull latest before I thrash the cluster”

1. Multi-select services (or a folder group).
2. **git pull** each (see Git rules).
3. On conflict / non-ff failure: mark that row failed, **do not** attempt merge
   UI; user fixes in their normal git tooling, then retries.

### Flow F — Phase 2: “What’s live vs what I’m about to ship?”

1. With kube context selected, refresh **deployed versions** for services that
   map to cluster workloads.
2. Browser shows **local Gradle version** vs **deployed version** (and branch).
3. Filter “behind / ahead / unknown”; act (build, skaffold, or just go fix git).

---

## Concepts

| Term | Meaning |
|------|---------|
| **Workspace** | One or more root paths + scan settings + cached graph |
| **Git repo** | Directory containing `.git` (almost always 1 product git root) |
| **Project** | A discoverable unit inside a repo: path, name, kind, version, skaffold file(s), gradle identity |
| **Kind** | `service` \| `library` \| `avro` \| `unknown` (heuristic + overrides) |
| **Coordinate** | Maven-style `group:name` (+ version or range when relevant) |
| **Edge** | `Project → Coordinate` (depends on) or `Project produces Coordinate` |
| **Consumer set** | Projects that depend on a selected project’s published coordinate(s), including range matches against the **currently selected/published version** |
| **Action** | A runnable verb: gradle task(s), skaffold command, git command, or a **recipe** |
| **Recipe** | Named cascade, e.g. `publish-and-redeploy-consumers` |
| **Deployed version** (phase 2) | Version string observed in the cluster for a mapped workload |

---

## Repo layout (authoritative)

### Default: one git repo ≈ one product tree

- **Most projects are their own git repository** (one `.git` at the repo root).
- Scan still walks **filesystem roots** you configure (e.g. `~/Developer`);
  each discovered project records `git_root` = nearest ancestor with `.git`.

### Exception: monorepos with multiple Skaffold entrypoints

Some git repos are **monorepos** and may contain **multiple `skaffold.yaml` /
`skaffold.yml` files** in different subdirectories (multiple deployable
“apps” inside one clone).

Implications:

| Concern | Behavior |
|---------|----------|
| **Inventory unit** | One **project row per skaffold entrypoint** (and/or per Gradle module that publishes), not one row per git repo only |
| **Git metadata** | Shared across rows that share the same `git_root` (branch, pull, dirty) |
| **Gradle** | May share one `settings.gradle.kts` root with several modules, or several mini-builds — detect per path |
| **Skaffold actions** | Always run with `-f` / cwd of **that** skaffold file, never assume repo-root only |
| **Grouping** | UI can group by **folder**, and optionally nest under **git repo name** when a monorepo has multiple projects |

### Discovery algorithm (sketch)

1. Walk scan roots (respect ignore + max depth).
2. Find all `skaffold.yaml` / `skaffold.yml` (and optional `skaffold.*.yaml` if
   you use them) → each becomes a **project** candidate of kind `service`
   (unless overridden).
3. Find Gradle roots / modules for **libraries and avro** without skaffold
   (publishable artifacts still need cascade sources).
4. For every project path, resolve:
   - `git_root` = nearest `.git` ancestor (or “not a git repo”).
   - `gradle_root` / module for version + deps.
5. Dedup: if monorepo has both a root skaffold and nested skaffolds, **prefer
   explicit nested entrypoints**; don’t double-count the same path.

### Project detection (v1 heuristics)

A directory is a **project candidate** if any of:

1. Contains `skaffold.yaml` or `skaffold.yml` (primary for services; monorepos
   may have **several** of these under one git root).
2. Contains `build.gradle.kts` or `build.gradle` and looks like a publishable
   lib/avro (no skaffold required).
3. Is a Gradle root (`settings.gradle.kts`) used only as an anchor for modules
   — modules themselves become rows when they publish or have skaffold.

**Skaffold-backed service (primary UI focus):** has at least one skaffold file.

**Library / avro:** often no skaffold; detected via Gradle + naming / tags /
manual kind override (`tako.project.toml` or UI mark).

### Gradle multi-project (within a git repo)

- Prefer **Gradle root** (`settings.gradle.kts`) as the build entrypoint
  (`gradle -p <root>` / cwd = that root).
- Invoke **`gradle` from `PATH`** only (see Execution model) — not the wrapper.
- Modules under `include(...)` map module path → filesystem path → optional
  skaffold beside the module.
- **Repo-per-service** remains the common case (git root == gradle root ==
  single skaffold). Monorepo is the important secondary case.

### Kind heuristics (overridable)

| Signal | Kind |
|--------|------|
| path/name contains `avro`, `schema`, `*-avro*` | `avro` |
| has skaffold + application plugin / boot jar / jib / etc. | `service` |
| `java-library` / `maven-publish` without skaffold | `library` |
| user override | wins |

### Project version (local)

Shown in the browser for every project row.

**Source of truth (v1):** static parse of the owning module’s
`build.gradle.kts` / `build.gradle`, then fallbacks:

1. `version = "…"` in the module build file  
2. `version` in `gradle.properties` (project or root)  
3. Catalog / `build.gradle.kts` `version = libs.versions.…` if you use that  
4. Else `—` / unknown  

Display as **local version** (e.g. `1.4.2` or `0.0.1-SNAPSHOT`). This is the
version you are about to build/publish, not necessarily what is in Nexus or
the cluster (cluster = phase 2).

---

## Dependency graph

### Produces (outbound coordinates)

From each project’s Gradle model, extract **published** coordinates:

- `group`, `archivesName` / project name, publishing block
- Version: from project version, catalog, or `gradle.properties`

Store as: `project_id → [coordinate]`.

### Depends (inbound)

From:

1. `build.gradle.kts` / `.gradle`:
   - **Direct GAV strings** (Nexus/Maven Central at *build* time — tako only
     parses the string for the graph, does **not** download):  
     `implementation("group:name:1.2.3")`,  
     `implementation("util-libs:util-core:[1.0,2.0)")`,  
     `implementation(group = "g", name = "n", version = "v")`
   - project deps `project(":foo")`
   - platform BOMs (not expanded in v1)
2. **Version catalog** (`libs.xxx` / published `from("g:catalog:v")` under scan
   roots — see catalog registry):
   - aliases → `group`, `name`, `version` or **version ref**
   - version refs may be **strict**, **prefer**, or **range** strings

**Graph linking:** prefer exact `group:name` match to a scanned producer’s
`produces` coordinate; if groups differ but **artifact name** matches (common
when a script uses a short/module group vs the library’s Maven `group`), still
link for highlight/cascade.

### Range matching (critical)

Catalog / deps often use ranges: `[1.0.0, 2.0.0)`.

When cascading from a **publish** of version `1.4.2`:

1. Resolve producer coordinate `g:n` at version `1.4.2`.
2. Find all projects that depend on `g:n` with:
   - exact version equal, or
   - Ivy/Maven **range** that includes `1.4.2`, or
   - catalog alias pointing at a range that includes `1.4.2`.
3. Optional: if catalog still pins an old range that **excludes** the new
   version, **warn** (“consumers won’t pick this up until catalog bump”)
   instead of false-positive cascade.

**v1:** range inclusion via a small semver/ivy subset you actually use; no need
for full Maven range edge cases on day one. Cover:

- exact `1.2.3`
- `[1.0.0, 2.0.0)`, `(1.0, 2.0]`, `1.0.+` if you use it

### Project-to-project (same settings)

`implementation(project(":shared"))` → hard edge without Nexus.

### Refresh

- Full rescan (slow) vs incremental (watch mtimes of gradle/catalog/skaffold).
- Command: **r** refresh selected / workspace.
- Cache invalidation when roots or ignore rules change.

### Accuracy honesty

Gradle is a Turing-complete build system. v1 is **static parse + catalog**,
not full `gradle dependencies` for every project on every frame.

Later optional **deep resolve**: shell out to
`gradle :module:dependencies --configuration runtimeClasspath` and parse
(accurate, slow, cache aggressively).

---

## Persistence (`~/.config/tako/`)

Mirror rakko’s deliberate `~/.config/<app>/` layout (Linux + macOS).

```
~/.config/tako/
  config.toml           # roots, ignores, defaults, UI prefs
  workspaces/
    default.json        # or named workspaces
  cache/
    graph.json          # projects, edges, last scan time
    project-fingerprints.json
  history.jsonl         # optional command history
```

### `config.toml` (sketch)

```toml
# ~/.config/tako/config.toml

[scan]
roots = ["/Users/you/Developer/services", "/Users/you/Developer/libs"]
max_depth = 6
ignore = ["**/.git/**", "**/build/**", "**/node_modules/**"]

[gradle]
# Always use `gradle` on PATH (air-gap friendly). Do not download via wrapper.
# command = "gradle"          # override if needed (absolute path ok)
default_tasks_build = ["build"]
default_tasks_publish = ["publish"]

[skaffold]
default_profile = ""          # optional
dev_args = []                 # extra args
debug_args = ["--port-forward"]  # example; tune to your setup

[git]
pull_ff_only = true           # never invent merge commits; fail → user fixes
# pull_args = []              # extra args if needed

# Phase 2 — optional until M6
# [kube]
# context = "docker-desktop"
# namespaces = ["dev"]
# version_source = "label:app.kubernetes.io/version"  # or image_tag, env:APP_VERSION

[ui]
theme = "dark"
banner_mode = "off"

[cascade]
# After publishing a lib, what to do to consumers
default_consumer_gradle = ["build"]
default_consumer_skaffold = ["delete", "run"]   # or ["dev"] for interactive
```

### Cache schema (sketch)

```json
{
  "scanned_at": "2026-07-17T12:00:00Z",
  "projects": [
    {
      "id": "payments-api",
      "path": "/Users/you/Developer/services/payments-api",
      "git_root": "/Users/you/Developer/services/payments-api",
      "kind": "service",
      "version": "1.4.2",
      "git_branch": "main",
      "git_dirty": false,
      "skaffold": ["skaffold.yaml"],
      "gradle_root": "/Users/you/Developer/services/payments-api",
      "gradle_module": ":",
      "produces": [{ "group": "com.example", "name": "payments-api", "version": "1.4.2" }],
      "depends": [
        { "group": "com.example", "name": "common-lib", "version_req": "[1.0.0,2.0.0)" },
        { "group": "com.example", "name": "avro-payments", "version_req": "1.3.0" }
      ],
      "folder_group": "services",
      "deployed_version": null
    },
    {
      "id": "platform-monorepo/orders",
      "path": "/Users/you/Developer/platform/orders",
      "git_root": "/Users/you/Developer/platform",
      "kind": "service",
      "version": "2.1.0",
      "git_branch": "feature/foo",
      "git_dirty": true,
      "skaffold": ["skaffold.yaml"],
      "gradle_root": "/Users/you/Developer/platform",
      "gradle_module": ":orders",
      "folder_group": "platform",
      "deployed_version": null
    }
  ]
}
```

---

## Git (v1)

### Display

Every project row (and detail pane) shows:

| Field | Source |
|-------|--------|
| **Branch** | `git -C <git_root> rev-parse --abbrev-ref HEAD` |
| **Dirty** (optional column / glyph) | `git status --porcelain` non-empty |
| **Upstream divergence** (optional later) | `git rev-list --left-right --count` |

Projects that share a **git_root** (monorepo multi-skaffold) share the same
branch/dirty state — refresh once per root, fan out to rows.

### `git pull`

- Action on selection or multi-select (key draft: `g` then `p`, or `G` pull).
- Implementation: `git -C <git_root> pull --ff-only` **or** plain `git pull`
  with **no** tako-driven merge/rebase UI.
- **On success:** refresh branch tip; ephemeral toast optional.
- **On failure** (including merge conflicts, divergent branches, auth errors):
  1. Mark project/repo status `pull_failed` with short stderr summary.
  2. **Do not** open a merge tool, resolve conflicts, or continue a cascade
     that depended on a clean pull.
  3. User fixes outside tako (or in another terminal), then retries.
- Prefer **`--ff-only`** as default so tako never creates merge commits by
  surprise; config override if you want unrestricted pull later.
- Deduplicate: multi-select of five monorepo services under one git_root →
  **one** pull for that root.

### Non-goals (git)

- commit / push / checkout / rebase / stash UI  
- interactive conflict resolution  
- credential helpers beyond what `git` already uses  

---

## TUI (ratatui)

### Stack

- **ratatui + crossterm**
- **Elm-style** `App` / `Action` / `AppEvent` / `Command` reducer (same shape
  as rakko’s `src/app/` + `src/main.rs` event loop)
- Background work: **never block the render loop** (`spawn_blocking` /
  child processes with piped logs + cooperative cancel)
- Config path: `~/.config/tako/` only

### UI kit — reuse rakko (hard requirement)

**tako must take its UI elements from rakko and reuse them**, not invent a
second visual language. Look and feel, interaction patterns, and widget APIs
should match so both tools feel like one family (GrokNight-ish dark + light,
scarce purple, cyan chrome, green success / red error).

#### First-class chrome (must ship in phase 1 — port from rakko)

These are **product features**, not optional polish. Implement by porting the
rakko modules named below; only rebrand and domain-hook.

##### 1. Mouse control

Full parity with rakko’s mouse model:

| Behavior | Rakko reference | tako |
|----------|-----------------|------|
| Enable mouse capture | `main.rs` `EnableMouseCapture` | same |
| Hover row tint | `table_nav` + `theme.hover_row` | project list, plan steps, job list |
| Click to select row | `App::register_click` / `action_at` | same |
| Double-click → confirm / open | `check_double_click` in main | open project detail / confirm plan step |
| Click dialogs / tabs / panels | click regions | detail panes, view switcher, help close |
| Scroll wheel | list scroll / pane scroll | project list, log pane, plan table |
| Mouse position tracking | `App::set_mouse_pos` / `is_hovered` | same |

Keyboard remains primary; mouse is a first-class second path, not an afterthought.

##### 2. Toasts (ephemeral status)

Port rakko’s **toast** pattern (topic-detail copy feedback +
`set_ephemeral_status` / auto-dismiss timer):

- Top-centered (or equivalent) **toast** for short feedback, not a sticky
  footer that never clears.
- **Success** band: `theme.success` (greenish-yellow solid / bold) — same
  family as banner fps strip.
- **Error** band: `theme.error`.
- Auto-dismiss after ~2–3s (`status_clear_at` + tick in the event loop).
- **Do use for:** git pull ok/fail summary, build/publish finished, cascade
  done/failed, skaffold started/stopped, scan finished.
- **Do not use for:** banner **A** / theme **T** cycles (mode is visible on
  the chrome itself — lesson from rakko 0.13.2).
- Sticky `status_message` only for ongoing work if needed (“scanning…”,
  “publishing…”) and clear when the job ends (or convert to a toast).

##### 3. Theme

Port `theme.rs` wholesale:

- **Dark** (default) + **Light** palettes, same role table as rakko.
- **`T`** cycles theme; persist `[ui].theme` under `~/.config/tako/config.toml`.
- All screens/widgets take styles from `App.theme` — **no hardcoded colors**.
- Full-frame `root_style()` paint so the terminal default bg never shows
  through (rakko draw shell).

##### 4. Banner + wave / ms / fps / off

Port `banner.rs` + banner tick in the main loop:

| Mode | Label | Meaning |
|------|-------|---------|
| **wave** | `stream` | Decorative braille wave (`theme.secondary` cyan) |
| **ms** | `N ms/frame` | Paint wall time of `terminal.draw` |
| **fps** | `N fps` | Paint capacity `1000 / ms` (not screen refresh rate) |
| **off** | `paused` | Frozen strip; no banner tick redraw tax |

- Brand word **tako** (accent purple); cycle with **`A`**; save
  `banner_mode` in config (`wave` \| `ms` \| `fps` \| `off`).
- Samples timed around `terminal.draw` only (same honest diagnostic as rakko).
- Banner tick ~200ms when mode ≠ off.

##### 5. Splash screen (tako art, truecolor)

Port `splash.rs` structure from rakko; art is the repo asset:

| | |
|--|--|
| **Asset** | **`assets/tako.jpeg`** (square ~447×447; embed with `include_bytes!`) |
| **Path in code** | `include_bytes!("../../assets/tako.jpeg")` from `src/ui/splash.rs` |

**Truecolor half-block** when the terminal supports it (`supports_truecolor()` /
`COLORTERM` / override env e.g. `TAKO_TRUECOLOR`). **Braille / monochrome
fallback** when truecolor is off or `NO_COLOR` is set.

**Scale it up (important):** do **not** keep rakko’s modest preferred width
(`TRUECOLOR_COLS = 72`) as a hard comfort size. For tako:

- Prefer filling the **splash art pane**: target width ≈ **min(terminal art
  width, ~100–120 cols)** or simply **`max_cols` of the inner art block** so
  the octopus is large on a typical full-screen terminal.
- Still respect aspect ratio and available height (half-block rows); scale
  down only when the terminal is small.
- **No otter-style tight face crop** — `tako.jpeg` is already a composed square;
  use the **full image** (or a light center pad), not rakko’s 8%/12%/84%/60%
  face crop.

Show once at startup (`show_splash`); **any key / click dismisses**. Layout:
title line (`tako` / 蛸) + large art + “press any key” footer.

#### How to reuse (implementation order)

1. **M0:** Copy rakko’s UI primitives into `tako/src/ui/` (theme, banner,
   splash, help, widgets, mouse/click plumbing in `App` + `main`) and rebrand
   (`rakko` → `tako`, config paths). Splash loads **`assets/tako.jpeg`**, full
   frame, **scaled up** to the art pane (see Splash above).
2. **Prefer API-compatible ports** of `render_*` helpers so screens can call
   the same shapes (`render_selectable_list`, `render_confirm_dialog`, …).
3. **Later (optional):** extract a shared crate (e.g. `tui-kit` / `goro-ui`)
   used by both rakko and tako — **not** a v1 blocker. Until then, **copy +
   adapt**, and when either app improves a widget, port the fix across.

Do **not** hardcode one-off colors in tako screens; everything goes through
`Theme` slots like rakko.

#### Concrete inventory to port from `rakko/src/ui/`

| Rakko module | What it is | Use in tako |
|--------------|------------|-------------|
| **`theme.rs`** | `Theme` / `ThemeName`, dark+light, roles (accent purple, secondary cyan, success, error, border, dim, selected/hover row, …), `focus_title` / `focus_border`, `root_style` / `panel_style` | **Required** — see Theme above |
| **`banner.rs`** | Top strip: brand + wave / ms / fps / off | **Required** — brand **tako** |
| **`splash.rs`** | Truecolor half-block splash + braille fallback | **Required** — **`assets/tako.jpeg`**, scaled large |
| **`help.rs`** | `?` overlay, `KeybindSection` / `KeybindEntry` | Global + per-screen keybind help |
| **`widgets/table_nav.rs`** | `render_selectable_list`, column auto-width, **hover**, **click** rows, viewport helpers | Project browser, plan tables, job lists |
| **`widgets/footer.rs`** | `render_keybind_footer`, `split_with_footer` | Every screen’s keybind line |
| **`widgets/confirm_dialog.rs`** | `render_confirm_dialog`, `centered_rect`, … | Destructive confirms, quit |
| **`widgets/editor_pane.rs`** | Scrollable multi-line pane | Log viewer / path edit |
| **`widgets/view_switcher.rs`** | Numbered top-level tabs + click | Projects / Jobs / Workspace (if used) |
| **`mod.rs` draw shell** | clear → root bg → splash **or** banner → switcher → screen → help → quit | Same shell; splash until dismissed |
| **`App` click/hover + main mouse** | `register_click`, `action_at`, mouse stream, double-click | **Required** mouse control |
| **Ephemeral status** | `set_ephemeral_status`, dismiss tick, toast render | **Required** toasts |

#### Patterns to reuse (not separate files, but mandatory behavior)

| Pattern | Rakko source of truth | tako |
|---------|----------------------|------|
| **Selectable table lists** | topic list, profile picker, message list | project browser, cascade plan, job list |
| **List title embeds status** | `"Topics — loading…"` | `"Projects — scanning…"` / pull failures |
| **Ephemeral status toast** | copy toast + TTL | pull / build / cascade — see Toasts above |
| **Filter input line** | `/` on lists, reversed active field | project filter |
| **Quit confirm** | centered dialog | same |
| **Click regions + hover** | full mouse path | full mouse path — see Mouse above |
| **Config UI prefs** | `[ui] theme`, `banner_mode` | same under `~/.config/tako/` |
| **Color discipline** | purple = active only; cyan = chrome; green/red = semantic | deps tint via existing slots only |

#### Visual mapping for tako-specific states

| State | Theme slot |
|-------|------------|
| Selected project | `selected_row` |
| Hover | `hover_row` |
| Dependency (uses) | `secondary` (cyan) — or dim+secondary |
| Dependent (used by) | `warning` or `success` (pick one and stick; document in theme comment) |
| Skaffold running | `success` / status chip |
| Git dirty / pull fail | `warning` / `error` |
| Drift local ahead (phase 2) | `success` or `warning` |
| Drift cluster ahead | `warning` |
| Match | `dim` |

#### What not to invent

- A second palette or “tako-only” neon scheme  
- Ad-hoc list widgets that bypass `table_nav`  
- Footers that don’t use `render_keybind_footer`  
- Confirms that aren’t `render_confirm_dialog`  
- Blocking “loading” that freezes the UI (rakko’s async `AppEvent` model)

### Screens (v1)

1. **Workspace / roots** — add roots, rescan, open last workspace  
2. **Project browser** — tree or grouped list by **folder_group** (parent under root)
   - Columns (v1): **name**, **kind**, **version** (Gradle), **branch**,  
     skaffold?, git dirty?, last job status  
   - Phase 2 adds: **deployed** version, drift indicator  
   - Filter `/`, kind filter, monorepo sub-rows nested or flat with path suffix  
3. **Project detail** — path, git root, branch, version, produces, depends,
   skaffold file(s) / profiles  
4. **Graph focus** — select project → highlight **deps** / **dependents**  
5. **Run plan** — review cascade steps before execute (y/n)  
6. **Run console** — multiplexed or tabbed logs per project; status chips  

### Wireframes (rough — not final chrome)

Widths are illustrative (~100 cols). `>` = cursor, `*` = multi-selected,
`●` = skaffold present, `✗` = dirty git, colors called out in words only.

#### 1) Project browser (main home)

```
┌─ tako ───────────────────────────────────────────────────────────── workspace: default ─┐
│ / filter…                          kind: [all ▾]   roots: ~/Developer   r:rescan         │
├──────────────────────────────────────────────────────────────────────────────────────────┤
│ GROUP / NAME              KIND     VER      BRANCH        SK  GIT   STATUS               │
├──────────────────────────────────────────────────────────────────────────────────────────┤
│ ▼ avro                                                                                   │
│     avro-payments         avro     1.3.0    main          ·   ·     ok                   │
│     avro-orders           avro     2.0.1    main          ·   ·     ok                   │
│ ▼ libs                                                                                   │
│   > common-lib            library  1.4.2    main          ·   ·     idle                 │
│     logging-lib           library  0.9.0    release/0.9   ·   ✗     idle                 │
│ ▼ services                                                                               │
│   * payments-api          service  3.2.0    feature/x     ●   ·     skaffold dev ●       │
│     orders-api            service  1.1.0    main          ●   ·     idle                 │
│     notify-worker         service  0.4.0    main          ●   ·     idle                 │
│ ▼ platform  (monorepo: ~/Developer/platform)                                             │
│     platform/orders       service  2.1.0    feature/foo   ●   ✗     idle                 │
│     platform/billing      service  2.0.3    feature/foo   ●   ✗     idle                 │
├──────────────────────────────────────────────────────────────────────────────────────────┤
│ 3 selected · deps/dependents tint on highlight · common-lib: 4 dependents                │
│ b build  c clean  p publish  G pull  d dev  D debug  x delete  u run  P cascade  ?:help  │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```

Selection highlight (same screen, conceptual):

```
│   > common-lib            library  1.4.2    main     ·  ·   idle     │  ← selected (strong)
│     payments-api          service  3.2.0    …        ●  ·   idle     │  ← dependent (amber)
│     orders-api            service  1.1.0    …        ●  ·   idle     │  ← dependent (amber)
│     notify-worker         service  0.4.0    …        ●  ·   idle     │  ← dependent (amber)
│     some-other-lib        library  0.2.0    …        ·  ·   idle     │  ← depends ON common? (cyan)
```

#### 2) Project browser — phase 2 columns (deployed)

```
│ NAME            KIND     LOCAL    DEPLOYED  DRIFT      BRANCH     STATUS   │
│ payments-api    service  3.2.0    3.1.0     local▲     feature/x  idle     │
│ orders-api      service  1.1.0    1.1.0     match      main       idle     │
│ notify-worker   service  0.4.0    —         unknown    main       idle     │
│ common-lib      library  1.4.2    —         —          main       idle     │
```

`local▲` = local version newer than cluster · `cluster▲` = cluster newer ·
`match` / `unknown` / `—` for libs without a kube mapping.

#### 3) Project detail + graph side pane (Enter)

```
┌─ tako · payments-api ───────────────────────────────────────────────────────┐
│ path:  ~/Developer/services/payments-api                                     │
│ git:   ~/Developer/services/payments-api  branch: feature/x  dirty: no       │
│ ver:   3.2.0   kind: service   skaffold: ./skaffold.yaml                     │
│ gradle root: .   module: :                                                   │
├───────────────────────────┬─────────────────────────────────────────────────┤
│ PRODUCES                  │ DEPENDS ON                                      │
│ com.ex:payments-api:3.2.0 │ com.ex:common-lib     [1.0.0, 2.0.0)            │
│                           │ com.ex:avro-payments  1.3.0                     │
│                           │ (catalog: libs.common, libs.avro.payments)      │
├───────────────────────────┴─────────────────────────────────────────────────┤
│ DEPENDENTS (who uses this artifact)                                          │
│   (none — this is a leaf service)                                            │
│                                                                              │
│ For a library row, this pane fills with services; those rows also tint       │
│ amber in the browser behind this overlay / split.                            │
├──────────────────────────────────────────────────────────────────────────────┤
│ Esc back   b build   p publish   d skaffold dev   G pull   P cascade if lib  │
└──────────────────────────────────────────────────────────────────────────────┘
```

Split alternative (no modal): browser left ~55%, detail right ~45%.

```
┌─ list (groups) ────────────┬─ focus: common-lib ────────────────────────────┐
│ > common-lib     lib 1.4.2 │ version 1.4.2   branch main                    │
│   payments-api   svc …     │ produces: com.ex:common-lib:1.4.2              │
│   orders-api     svc …     │ ─────────────────────────────────────────────── │
│   notify-worker  svc …     │ DEPENDENTS                                     │
│   avro-payments  avro…     │  • payments-api  (range ok)                    │
│                            │  • orders-api    (range ok)                    │
│                            │  • notify-worker (range ok)                    │
│                            │ DEPENDS ON                                     │
│                            │  • (none / only third-party)                   │
└────────────────────────────┴────────────────────────────────────────────────┘
```

#### 4) Cascade plan confirm (P on a lib / avro)

```
┌─ plan: publish_and_redeploy_consumers ───────────────────────────────────────┐
│ source: common-lib @ 1.4.2  →  publish to Nexus                              │
│                                                                              │
│ #  STEP                         PROJECT         TASK                         │
│ 1  gradle clean build publish   common-lib      :clean :build :publish       │
│ 2  gradle build                 payments-api    :build                       │
│ 3  skaffold delete              payments-api    -f skaffold.yaml             │
│ 4  skaffold run                 payments-api    -f skaffold.yaml             │
│ 5  gradle build                 orders-api      :build                       │
│ 6  skaffold delete              orders-api      …                            │
│ 7  skaffold run                 orders-api      …                            │
│ …                                                                            │
│ warn: catalog range for foo-svc is [1.0,1.2) — will NOT pick up 1.4.2        │
├──────────────────────────────────────────────────────────────────────────────┤
│ y/Enter run plan    n/Esc cancel    j/k scroll                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

#### 5) Run console (jobs live)

```
┌─ jobs ────────────────────────────────┬─ log: payments-api · skaffold run ──┐
│ ● common-lib      publish    ok  12s  │ …                                  │
│ ● payments-api    build      ok   4s  │ Starting deploy…                   │
│ ▶ payments-api    skaffold   run …    │ - deployment/payments-api  ok      │
│ · orders-api      build      pend     │ Watching for changes? (run: no)    │
│ · orders-api      skaffold   pend     │                                    │
│                                       │                                    │
├───────────────────────────────────────┤                                    │
│ Esc cancel focused job · Tab focus    │                                    │
│ list vs log · q only if idle          │                                    │
└───────────────────────────────────────┴────────────────────────────────────┘
```

#### 6) Git pull multi-select result toast / status strip

```
│ pull: platform (monorepo) ok · payments-api ok · logging-lib FAIL (not ff)   │
│ → fix conflicts/diverged branch in a terminal, then G again                  │
```

No merge editor inside tako.

#### 7) First-run / workspace roots (minimal)

```
┌─ tako · workspace ──────────────────────────────────────────────────────────┐
│ Scan roots                                                                   │
│   ~/Developer/services                                                       │
│   ~/Developer/libs                                                           │
│   ~/Developer/avro                                                           │
│   [+] add path                                                               │
│                                                                              │
│ last scan: 2m ago   projects: 42   git repos: 38   skaffolds: 21             │
│ Enter open browser   r rescan   e edit roots                                 │
└──────────────────────────────────────────────────────────────────────────────┘
```

#### Layout chrome (global) — same shell as rakko

Matches rakko’s `ui::draw`: full-frame clear + `theme.root_style()`, then:

```
┌─ splash (first paint): assets/tako.jpeg half-blocks, LARGE (fill art pane) ──┐
│ any key or click → dismiss                                                   │
└──────────────────────────────────────────────────────────────────────────────┘

┌─ banner: “tako” + wave | N ms/frame | N fps | off   [A cycles, saved] ───────┐
│ optional view_switcher (1 Projects  2 Jobs  3 Workspace)  ← clickable tabs   │
│ content — table_nav lists (hover + click + wheel), confirm_dialog, logs    │
│ …                                                                            │
├─ toast (ephemeral, ~2.5s): success green / error red ────────────────────────┤
└─ keybind footer (render_keybind_footer) ─────────────────────────────────────┘
```

`?` → help overlay. `q` → quit confirm. **T** theme. **A** banner mode.
Mouse works on lists, dialogs, and splash dismiss.

These are **targets for implementation**, not a pixel-perfect contract. Columns
can compress (branch truncated, VER short) on narrow terminals; monorepo
nesting can flip to flat `platform/orders` labels if the tree feels noisy.
**Chrome and widgets stay rakko-identical**; only domain columns/copy change.

### Keybinds (draft)

| Key | Action |
|-----|--------|
| `r` | Refresh scan / project (gradle version + git metadata) |
| `b` | Gradle build (selected) |
| `c` | Gradle clean |
| `p` | Gradle publish |
| `G` | Git pull (selected / multi; dedupe by git_root; ff-only default) |
| `d` | Skaffold dev |
| `D` | Skaffold debug |
| `x` | Skaffold delete |
| `u` | Skaffold run (Up) |
| `Enter` | Detail / graph focus |
| `Space` | Multi-select |
| `B` | Recipe: build selection |
| `P` | Recipe: publish then cascade consumers (confirm) |
| `?` | Help |
| `q` | Quit |

Phase 2 additions: e.g. `K` refresh deployed versions from kube; filter
`drift only`.

Exact keys can shift once it feels cramped; keep **destructive** actions
behind confirm (delete, mass redeploy).

### Grouping

- Primary: **folder group** = first path segment under each scan root
  (e.g. `services/`, `libs/`, `avro/`).
- When a git monorepo yields multiple skaffold projects, either:
  - nest under repo name, or  
  - flat list with `repo/subpath` labels  
- Secondary sort: kind, then name.

### Selection highlighting

- Selected: strong highlight.
- Direct dependencies: one style (e.g. cyan).
- Direct dependents: another (e.g. amber/green).
- Transitive (optional toggle): dimmer.

---

## Execution model

### Process runner

- Spawn **`gradle`** (from `PATH`, or absolute path in config) with cwd =
  gradle root; module tasks as `:module:build` when needed.
  - **Always `gradle`, never `./gradlew`.** Wrappers pull distributions from
    the network; air-gapped / managed desktops already put Gradle on PATH.
  - Startup / first-build check: `gradle --version` must succeed or show a
    clear “`gradle` not on PATH” error.
- Spawn `skaffold` with cwd = directory of the **selected skaffold file**
  (or `-f` absolute path) — critical for monorepos with multiple skaffolds.
- Spawn `git -C <git_root> …` for branch/status/pull.
- Capture stdout/stderr with ring buffers per job.
- **Cancel:** kill process group on Esc/confirm cancel (document skaffold’s
  behavior with child builds). Git pull: usually short; still killable.
- Parallelism: cap concurrent Gradle/skaffold/git (config `max_jobs`), never
  unbounded.

### Recipes (v1)

#### `publish_and_redeploy_consumers`

1. Preconditions: selected project produces coordinates; network for Nexus.  
2. `gradle clean build publish` (configurable tasks).  
3. Compute consumer set for published version.  
4. Show plan table: consumer × actions.  
5. On confirm: for each consumer (ordered: topological if project-deps exist,
   else alphabetical):
   - gradle build  
   - skaffold delete (if skaffold present)  
   - skaffold run **or** attach to a multi-dev supervisor (see below)  
6. Summary: ok / fail per step.

#### `dev_many`

1. Multi-select services.  
2. Start **N** `skaffold dev` processes **or** one wrapper — v1 = N processes
   with a process list UI (simple, matches “I used to open N terminals”).  
3. Later: optional tmux/wezterm integration — non-goal for v1.

### Skaffold dev vs run

| Command | Use |
|---------|-----|
| `skaffold dev` | Active coding; file sync / rebuild loop |
| `skaffold debug` | Debug ports / debug mode |
| `skaffold run` | One-shot deploy after lib cascade |
| `skaffold delete` | Tear down before clean redeploy |

Cascades after lib publish default to **delete + run** (non-interactive).
Interactive **dev** stays user-triggered on a focused set.

---

## Architecture (implementation sketch)

```
tako/
  Cargo.toml
  src/
    main.rs              # event loop (rakko-shaped), child process supervisor
    config/              # load/save ~/.config/tako  ([ui] theme/banner like rakko)
    scan/                # walk roots, detect projects + monorepo multi-skaffold
    gradle/
      parse_kts.rs       # light parse: version, group, deps (v1)
      catalog.rs         # libs.versions.toml
      ranges.rs          # version range inclusion
      model.rs           # coordinates, edges
    git/
      status.rs          # branch, dirty (per git_root, cached)
      pull.rs            # ff-only pull; map conflicts → user-facing error
    skaffold/
      detect.rs
    kube/                # phase 2: deployed version probes
      discover.rs
      versions.rs
    graph/
      build.rs           # assemble graph from scan + gradle
      query.rs           # dependents, deps, cascade set
    exec/
      gradle.rs
      skaffold.rs
      git.rs
      plan.rs            # recipes → steps
      runner.rs          # jobs, logs, cancel
    app/                 # Elm reducer (mirror rakko app/ layout)
    ui/                  # *** ported from rakko, then domain screens ***
      theme.rs           # from rakko
      banner.rs          # from rakko (brand tako)
      help.rs            # from rakko
      splash.rs          # from rakko (art later)
      mod.rs             # draw shell like rakko
      widgets/           # from rakko: table_nav, footer, confirm_dialog,
      #                  #   editor_pane, view_switcher
      screens/           # tako-only: browser, detail, plan, console, workspace
  tests/
    fixtures/            # tiny fake repos: catalog ranges, multi-skaffold monorepo
```

### Parsing strategy (pragmatic)

**v1 — static:**

- TOML catalog: full parse (easy).
- `build.gradle.kts`: targeted extraction (regex / simple AST via `kotlin`
  not required) for:
  - `group = "…"`
  - `version = "…"`  ← **browser column**
  - `libs.xxx` catalog refs
  - `"group:name:version"` strings
  - `project(":…")`
- Optional: read `gradle.properties` for `version=`.

**v1 git:** shell out to `git`; no libgit2 required initially (can switch later).

**v2 — deep Gradle:**

- Cached `gradle … dependencies` per project.

**Phase 2 — kube versions:**

- `kubectl` / kube API (TBD) to read labels/annotations/image tags that encode
  app version for mapped workloads.

Ship v1 with **fixture tests** from sanitized real patterns (no secrets).

---

## Security & safety

- Never run arbitrary shell from project files without showing the command.
- Confirm mass **delete** / **publish** / multi-redeploy.
- Don’t store Nexus passwords; rely on Gradle’s existing
  `~/.gradle/gradle.properties` / env / credential helpers.
- Log redaction optional later.

---

## Observability

- Job status: pending / running / ok / failed.
- Timings per step.
- Last error line promoted to status bar.
- Optional export of last plan as text.

---

## Testing strategy

| Layer | What |
|-------|------|
| Unit | version ranges, catalog parse, cascade set selection |
| Fixture scan | temp dirs with fake skaffold + gradle + catalog |
| Exec dry-run | plan builder without spawning |
| Ignored integration | real `gradle` on PATH + tiny sample (optional CI) |

No live cluster required for core graph tests.

---

## Milestone plan

### Phase 1 — Local control plane

#### M0 — Skeleton + rakko UI kit — **done**

- Cargo app, config dir (`~/.config/tako/`).
- **Port rakko UI kit** end-to-end:
  - **theme** (dark/light, **T**, persist)
  - **banner** (wave / **ms** / **fps** / off, **A**, persist)
  - **splash** (`assets/tako.jpeg` truecolor, **scaled up** to fill art pane;
    braille fallback; any key/click dismiss)
  - **toasts** (ephemeral success/error, auto-dismiss)
  - **mouse** (capture, hover, click, double-click, wheel)
  - help, footer, table_nav, confirm_dialog, editor_pane, view_switcher, draw shell
- Empty project browser using `render_selectable_list` + keybind footer so the
  first screen already *looks and feels* like rakko (keyboard **and** mouse).
- Scaffold **release packaging** early (can be thin): `Dockerfile.rhel9`,
  `scripts/build-tui-rhel9.sh`, `scripts/build-macos.sh`, `dist/` gitignore,
  CHANGELOG + AGENTS release checklist (same assets as rakko — see Release
  packaging).
- AGENTS.md / README for humans (point at this SPEC + “UI from rakko”).

#### M1 — Discovery + persistence — **done**

- Scan roots; **one row per skaffold** (monorepo multi-skaffold supported).
- Git root association; group by folder; save/load workspace cache.
- Parse + display **Gradle version**; **git branch** (+ dirty glyph).
- Manual kind override.
- Workspace tab lists roots (edit via config until M1.5).

#### M1.5 — In-TUI workspace roots (rakko profiles-shaped) — **done**

- **n** add root, **e** edit selected, **z**/delete with confirm — path text field
  (expand `~/`), save to `~/.config/tako/config.toml` `[scan].roots`.
- Reuse rakko text-field / form patterns (`text_field`, focus chrome).
- After save: optional auto-rescan (`Command::ScanWorkspace`).
- First-run: empty roots → prompt to add rather than only “edit the TOML”.

#### M2 — Graph (static) — **done**

- Catalog parse + dependency extraction.
- Produces/depends model; dependents query.
- Highlight deps/dependents in UI.

#### M3 — Single-project exec — **done**

- Gradle build/clean/publish.
- Skaffold dev/debug/delete/run (`-f` / correct cwd for monorepos).
- **Git pull** (ff-only default; abort on conflict).
- Log pane + cancel.

#### M4 — Cascade recipes — **done**

- `publish_and_redeploy_consumers` with confirm plan.
- Range-aware consumer selection + catalog mismatch warnings.

#### M5 — Polish — **done**

- Multi-select bulk actions (incl. bulk pull, bulk build).
- Refresh ergonomics, fingerprints.
- Help overlay polish; toast copy for every long job; splash art finalization
  if placeholder was used in M0.

### Phase 2 — Cluster version awareness

Not required for day-1 usefulness; ships after phase 1 is solid.

#### M6 — Deployed version inventory

- Config: kube context(s), namespace(s), how a **project maps** to a workload
  (label selector, deployment name pattern, Argo application name, etc.).
- Probe live versions (prefer a single conventional signal — see open
  questions), cache with TTL, refresh on demand (`K` or similar).
- Browser columns: **local version** | **deployed version** | **drift**
  (`match` / `local_ahead` / `cluster_ahead` / `unknown`).

#### M7 — Drift-driven workflows

- Filter “only drift”.
- Optional recipe: “update these to match local” → build + skaffold run
  (still confirm; never silent prod mutations).
- Read-only mode: compare only, no deploy actions.

---

## Phase 2 detail — deployed versions from Kubernetes

### Intent

Answer quickly: *“What version is running in this cluster vs what my checkout
claims?”* so you know what to rebuild/redeploy after lib/avro cascades or
missed skaffold runs.

### Mapping project → workload

Each service project may declare (auto-guess + override):

```toml
# optional per-project or inferred
[kube]
namespace = "dev"
# one of:
deployment = "payments-api"
# label_selector = "app.kubernetes.io/name=payments-api"
# argo_app = "payments-api"
```

Defaults: derive name from project folder / skaffold metadata when possible.

### Version signal (pick one primary in M6)

Prefer **one conventional source** so compare stays reliable:

| Source | Example | Notes |
|--------|---------|--------|
| Pod/Deployment **label** | `app.kubernetes.io/version=1.4.2` | Clean if you set it in manifests/skaffold |
| **Annotation** | `tako/version` or build-info | Flexible |
| **Image tag** | `…/payments-api:1.4.2` | Common; messy with `latest` / digests-only |
| **Env var** | `APP_VERSION` on container | Works if standardized |

Config: `version_source = "label:app.kubernetes.io/version" | "image_tag" | …`

### UX

- Show deployed version only for projects with a kube mapping + successful probe.
- Libs/avro rows: usually **no** deployed version (unless you deploy schema
  registries as apps — rare); leave `—`.
- Stale cache: dim “as of 3m ago”; force refresh.

### Safety

- Phase 2 **read path** is default; write path reuses phase 1 skaffold/gradle
  with the same confirms.
- Never kubectl-apply arbitrary YAML from tako in v1 of phase 2.

---

## Open questions (resolve during build)

1. ~~**Repo layout**~~ → **Resolved:** mostly **1 git repo per project**; some
   monorepos with **multiple skaffold files**.
2. **Publish task names:** always `publish`, or `publishToNexus`,
   `publishMavenPublicationTo…`?
3. **Catalog ownership:** one shared catalog repo vs per-service catalog copy?
   (Affects “who must bump versions” warnings.)
4. After lib publish, do consumers need a **catalog version bump + refresh
   dependencies** before build, or do ranges make that automatic?
5. Preferred cascade deploy: **run** vs restart existing **dev** sessions?
6. Naming of folder groups: automatic from path only, or
   `tako.workspace.toml` labels?
7. Should tako understand **Skaffold profiles** / multiple kube contexts per
   project?
8. **Git pull:** confirm `--ff-only` as hard default, or allow merge pulls?
9. **Phase 2 version signal:** labels vs image tags vs env — what do your
   manifests already set consistently?
10. **Phase 2 scope:** only dev cluster, or multi-context (dev/stage) with a
    context picker?

---

## Success metrics

- Cold scan of your Developer tree completes in acceptable time (target:
  under ~30s for hundreds of repos with cache warm thereafter).
- Monorepos with multiple skaffolds show **one controllable row per
  skaffold**, sharing git branch/pull correctly.
- Browser always shows **Gradle version** + **git branch** without opening a
  terminal.
- Publish-lib → consumer list is **correct for your catalog ranges** (spot
  check vs mental model / `gradle dependencies`).
- “Change avro X → rebuild/redeploy all users” is **one confirm**, not N
  terminal tabs.
- Day-to-day `skaffold dev` on 1–3 services is faster than juggling directories.
- **Pull** either succeeds or fails loudly with “fix conflicts yourself” —
  never leaves a half-merged tree driven by tako.
- **Phase 2:** drift between local and cluster version is visible in one
  screen for the services you care about.

---

## Relationship to rakko

| | **rakko** | **tako** |
|--|-----------|----------|
| Domain | Kafka clusters / messages | Gradle + Skaffold multi-repo |
| Metaphor | Otter | Octopus |
| Config | `~/.config/rakko/` | `~/.config/tako/` |
| UI | ratatui Elm-style + **source UI kit** | **reuses that kit** (copy first, shared crate later) |
| Theme / chrome | GrokNight roles, banner A, help `?` | same |
| **Release assets** | RHEL9 linux/amd64 + macOS + `SHA256SUMS` | **same shape** (see below) |

They complement: tako ships code; rakko inspects the bus between services.

**UI ownership:** rakko remains the reference implementation of widgets/theme.
tako tracks it. Improvements should ideally land in a way that both can adopt
(shared crate when the cost of dual maintenance shows up).

### Port from rakko — beyond the UI kit

First-class chrome (theme, banner, splash, toasts, mouse, help, widgets) is
already required above. This section covers **engine, ops, and quality** to
steal so tako feels like “rakko for builds,” not a greenfield TUI with similar
colors.

#### Strongly recommended (bring over)

| From rakko | Why for tako |
|------------|--------------|
| **Elm loop** — `App` / `Action` / `AppEvent` / `Command` | Same event-driven shape; keeps gradle/skaffold off the render thread |
| **Terminal bootstrap** | raw mode, mouse capture, panic hook restore, Ctrl-c force quit |
| **Background I/O rules** | `spawn` / `spawn_blocking` + cooperative cancel — critical for long `skaffold dev` / `gradle build` |
| **`~/.config/<app>/` + `config.example.toml`** | Already planned; keep load/save + `[ui]` prefs pattern |
| **Release scripts + AGENTS / CHANGELOG discipline** | RHEL9 + macOS assets, Keep a Changelog, agent checklist (see Release packaging) |
| **`RingBuffer` (or equivalent)** | Job logs / last N lines per process without unbounded memory |
| **Pure-logic tests + optional `#[ignore]` integration** | Graph/range/catalog tests offline; real `gradle` on PATH later if wanted |
| **Click-region + double-click plumbing on `App`** | Copy the mechanism (`register_click`, `action_at`, double-click timer in `main`), not only “support mouse” |
| **Ephemeral status TTL tick** | Toasts that actually clear (`status_clear_at` + loop wake) — avoid sticky chrome toasts |

**Behavioral lessons to keep explicit:**

- **Don’t toast** pure chrome changes (banner **A** / theme **T**) — mode is already on screen.
- **Do toast** job outcomes (pull / build / publish / cascade / scan done).

#### Nice to have

| From rakko | Use in tako |
|------------|-------------|
| **Clipboard (copy)** | Copy project path, failed command line, log snippet |
| **Text-field helpers** (`text_field`) | Filter `/`, edit scan roots, paste paths (Ctrl/Cmd+V) |
| **View switcher** | Top-level modes once you have 2–3 (e.g. Projects / Jobs / Workspace) |
| **Tracing + quiet defaults** | Debug long job orchestration without spamming the TUI |
| **`AppError` / `AppResult` style** | Consistent status toasts for failures |

#### Skip (rakko-specific / low value)

- Kafka / rdkafka / schema registry / Avro wire decode  
- Consumer seek, replay, export JSONL  
- Profile TLS/mTLS cluster connectivity  
- Vendoring OpenSSL/librdkafka in the RHEL image (keep tako’s image thin unless deps require it)  
- Splash otter crop ratios (use full `assets/tako.jpeg`, scaled large)

---

## Release packaging (match rakko)

tako uses the **same release discipline and asset layout as rakko**, so cutting
a version always produces installable binaries for air-gapped Linux and macOS
developers. Adapt rakko’s scripts (simpler deps: no librdkafka/OpenSSL vendoring
unless we add similar native deps later).

### Artifacts (every version)

| Asset | Source | Notes |
|-------|--------|--------|
| `dist/tako-linux-amd64.tar.gz` | RHEL 9 / Rocky 9 container build | **Primary** GitHub Release asset for Linux airgap; ELF **x86-64** |
| `dist/tako-linux-amd64` | same | Bare binary (optional attach) |
| `dist/tako` | same | Short name copy of linux binary (rakko convention) |
| `dist/ldd.txt` | builder `ldd` audit | Expect glibc + libgcc_s only (no surprise dynamic deps) |
| `dist/tako-macos-<arch>.tar.gz` | native `cargo build --release` on Mac | `arm64` or `x86_64` matching the build machine |
| `dist/tako-macos-<arch>` | same | Bare Mach-O |
| `dist/otool-macos-<arch>.txt` | `otool -L` audit | System frameworks + libSystem only |
| `dist/SHA256SUMS` | both scripts | **Merged** (macOS script must not clobber Linux entries) |

Do **not** commit `dist/` (gitignored). Attach archives + `SHA256SUMS` to the
GitHub Release only.

### Scripts / Docker (port from rakko)

| Rakko | tako equivalent |
|-------|-----------------|
| `Dockerfile.rhel9` | `Dockerfile.rhel9` — Rocky/RHEL 9, `linux/amd64`, cargo release build |
| `scripts/build-tui-rhel9.sh` | `scripts/build-tui-rhel9.sh` — docker/container/podman, extract to `dist/` |
| `scripts/build-macos.sh` | `scripts/build-macos.sh` — native package + merge `SHA256SUMS` |

tako’s Dockerfile can be **lighter** than rakko’s (no cmake/perl/OpenSSL
vendoring unless Cargo deps require it). Keep the same **platform pin**
(`linux/amd64`), extract path, and audit checks.

### Release checklist (when `Cargo.toml` version bumps)

Same ownership rule as rakko: whoever cuts the version owns the checklist.

1. **Bump** `version` in `Cargo.toml` (SemVer). `cargo build` so `Cargo.lock` updates.
2. **Changelog:** move `[Unreleased]` → `## [X.Y.Z] - YYYY-MM-DD`; leave empty
   `[Unreleased]`. Prefer Keep a Changelog user-facing bullets.
3. **RHEL 9 asset (do not skip):**
   ```bash
   ./scripts/build-tui-rhel9.sh
   # Prefer DOCKER=docker; else DOCKER=container on Apple Silicon
   ```
   Confirm: `file dist/tako-linux-amd64` → ELF 64-bit **x86-64**; `ldd.txt` clean.
4. **macOS asset (do not skip):**
   ```bash
   ./scripts/build-macos.sh
   ```
   Confirm tarball + `otool-macos-<arch>.txt`; `SHA256SUMS` merged with Linux.
5. **Commit, tag, GitHub Release** (manual — no CI requirement unless added later):
   ```bash
   git tag -a vX.Y.Z -m "Release X.Y.Z: <headline>"
   git push origin main   # or master
   git push origin vX.Y.Z
   gh release create vX.Y.Z \
     --title "vX.Y.Z — <headline>" \
     --notes "<from CHANGELOG.md [X.Y.Z] section>" \
     dist/tako-linux-amd64.tar.gz \
     dist/tako-macos-<arch>.tar.gz \
     dist/SHA256SUMS
   ```
6. Verify: `gh release view vX.Y.Z` shows all three assets, `isDraft: false`.

### Changelog / AGENTS.md

Mirror rakko’s agent docs:

- `CHANGELOG.md` Keep a Changelog + `[Unreleased]`
- `AGENTS.md` (or `Agents.md`) documents layout, hard constraints, **release
  checklist**, and “don’t commit dist/”

### Install story (user-facing README)

Document both:

```bash
# Linux airgap (RHEL9-compatible x86_64)
tar -xzf tako-linux-amd64.tar.gz && ./tako

# macOS
tar -xzf tako-macos-arm64.tar.gz && ./tako-macos-arm64
# or x86_64 archive when built on Intel
```

Optional: `cargo install --path .` for contributors; releases are for people
who should not need a Rust toolchain.

---

## Next steps

1. Confirm name **tako** (or pick an alternative above).  
2. Answer open questions 1–4 (layout, publish tasks, catalog, ranges).  
3. Drop 1–2 **anonymized fixtures** (catalog snippet + two fake services + one
   lib) into `fixtures/` for parser TDD.  
4. Scaffold Cargo project (M0) when ready to implement.
