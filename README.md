# tako

Multi-service **Gradle + Skaffold** control plane (ratatui TUI).

**tako** (蛸) = octopus — many arms across microservices, shared libs, and Avro
repos. Same naming spirit as [rakko](../rakko) (ラッコ / otter).

## Status

**Phase 1 (M0–M5) + Phase 2 M6/M7 light** — discovery, workspace roots, static
dependency graph, jobs/logs, cascade publish, multi-select, kube deployed
version / drift (**K** / **f**). Full design: **[SPEC.md](./SPEC.md)**.

`cargo test`: green. Not yet cut as a GitHub Release (`CHANGELOG` still under
`[Unreleased]` / `0.1.0` TBD).

### What works

| Area | Keys / notes |
|------|----------------|
| Splash | `assets/tako.jpeg` truecolor (large); any key dismiss |
| Theme / banner | **T** dark/light · **A** wave / ms / fps / off |
| Help | **?** |
| Workspace (tab **3**) | **Tab** roots/excludes · **n**/**e**/**z** · **r** rescan |
| Settings (tab **4**) | Edit all config (kube enable/ns, gradle, cascade, …) |
| Projects (tab **1**) | scan inventory, graph highlight deps/dependents · **Enter** detail |
| Jobs (tab **2**) | live logs · **Tab** list/log · **Esc** cancel job |
| Exec | **b** build · **c** clean · **p** publish · **d**/**D** skaffold · **x**/**u** delete/run · **G** pull |
| Cascade | **B** publish → rebuild dependents · **P** + skaffold delete/run (confirm plans) |
| Kube | **K** refresh deployed versions · **f** drift-only filter (`[kube]` config) |
| Multi-select | **Space** · bulk **b**/**c**/**G** |

Requires on `PATH`: **`gradle`** (not `./gradlew`), **`skaffold`**, **`git`**.
Phase 2 also needs **`kubectl`** when `[kube] enabled = true`.

## Build & run

```bash
cargo build --release
cargo run

mkdir -p ~/.config/tako
cp config.example.toml ~/.config/tako/config.toml
# edit [scan].roots — or add roots in the TUI (tab 3, n)
```

Config path is always `~/.config/tako/` (macOS + Linux).

## Release assets (same shape as rakko)

```bash
./scripts/build-tui-rhel9.sh   # dist/tako-linux-amd64.tar.gz
./scripts/build-macos.sh       # dist/tako-macos-<arch>.tar.gz + merged SHA256SUMS
```

See `AGENTS.md` for the full release checklist.

## Name

Alternatives (`kumo`, `ari`, `tanuki`, …) are listed at the top of `SPEC.md`.
