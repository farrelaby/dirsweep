# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Criterion benchmark harness (`benches/scan.rs`) timing the scan core phase by phase: `pipeline`, `scan`, `build_tree`, `size`, `render`
- Deterministic seeded fixture generator for benchmarks, replacing the non-deterministic `$RANDOM` fixtures in `bench/bench.sh`
- Size-computation benchmark, replicating the worker loop in `main.rs` — previously unmeasured, as the TUI discards it before it completes
- `ScanStats` work counters on `ScanOutput` (directories entered, files seen, subtrees pruned, detection hits) for machine-independent before/after comparison
- CI `Bench` step (`cargo bench --bench scan --no-run`) so the harness cannot silently rot
- Pre-rewrite benchmark baseline recorded in `bench/BASELINE.md`, with machine-readable numbers in `bench/baseline.json`
- Per-phase timing on `ScanStats` (`pass1_us`, `project_build_us`, `pass2_us`) and a benchmark table splitting `scan` across its three phases, so a parallel walker can be aimed at the pass that dominates
- `size_par` benchmark group measuring the parallel size fan-out, kept alongside the sequential `size` group so the speedup is measured within a single run
- Test asserting hidden files inside a target dir count toward its size — guards against walkers that skip dotfiles by default, which would otherwise underreport silently
- Cryptographic SHA-256 verification of downloaded binary archives against `checksums.txt` in npm postinstall (`npm/install.js`)
- Pre-publish version check in CI release workflow ensuring `Cargo.toml` matches git release tag (`release.yml`)

### Changed
- CI clippy now lints all targets (`--all-targets`), covering benches
- `[profile.bench]` sets `opt-level = 3`, overriding the release `-z`, so benchmarks isolate algorithmic change from optimizer behaviour
- `build_tree` indexes target dirs in a `HashMap` once instead of rescanning them per project, taking it from O(projects × targets) to O(projects + targets)
- The size worker fans target walks out across rayon rather than walking every target sequentially on one thread; results still stream per target so the UI fills in progressively
- Benchmark comparison uses `--baseline-lenient` instead of `--baseline`, so adding a benchmark group no longer breaks every subsequent comparison
- Pass 1 walks with `dua-core` across a work-stealing pool instead of one sequential `walkdir` traversal; the predicate still prunes VCS and target dirs, and entries are filtered in the consumer so work counters stay identical to before
- `max_path_width` is cached on `AppState` and refreshed when the tree changes, rather than being rederived from every target on each frame

### Performance
- `pipeline/many-tiny` 34.76 ms → 9.43 ms (−72.9%, 3.7×), in two independent steps: `build_tree` 20.11 ms → 0.44 ms (−97.8%), then pass 1 across dua-core's pool 13.65 ms → 8.57 ms (−37%)
- `pipeline/deep-nesting` 4.50 ms → 3.01 ms (−33.2%)
- Size computation 3.4–6.0× faster depending on target count, measured within a single run against the sequential control
- `render/many-tiny` 366.75 µs → 202.10 µs (−45%) and `render/deep-nesting` 259.25 µs → 221.40 µs (−14%), from caching the maximum path width instead of deriving it from the whole tree on every frame
- Trade-off from the walker: spawning its pool costs a fixed ~350 µs, so small trees regress — `pipeline/dense-targets` 205 µs → 752 µs, `pipeline/node-monorepo` 804 µs → 1236 µs, `pipeline/multi-lang` 821 µs → 1213 µs. Break-even is roughly 50–80 directories and every regression stays sub-millisecond in absolute terms

### Fixed
- Order-by dialog: pressing Up on the last menu item wrapped two rows instead of one (`SizeDesc` mapped to `DateAsc` instead of `SizeAsc`)
- Cursor landed on the project header after reordering the tree (reset `list_index` should skip the header, matching `build_tree`)
- Event loop deadlock when confirming deletion with 0 items selected (`main.rs`)
- Out-of-bounds index panic when navigating an empty tree after deleting all targets (`app.rs`)
- Cursor reset to valid target dir when remaining items shift after deletion (`app.rs`)
- Terminal state (raw mode, alternate screen, cursor) guaranteed restored on exit or panic via RAII guard (`main.rs`)
- Reclaimed size calculation now strictly counts successful deletions, excluding failed targets (`main.rs`)
- Recomputed `total_reclaimable` header metric after target dirs are removed from tree (`app.rs`)
- Prevented spurious `ENOENT` read errors when background size walk races with active deletion (`main.rs`)
- Enforced HTTPS protocol in npm installer, rejecting plaintext HTTP downloads and redirects (`npm/install.js`)
- Hardened Windows PowerShell extraction parameters in npm installer to prevent argument injection (`npm/install.js`)
- Disabled draft releases in GitHub Actions so release assets are immediately downloadable upon npm publish (`release.yml`)

### Changed
- Tree list now shows a blank row between project groups for visual separation
- Last-child glyph (`├`/`└`) is derived during rendering from the tree's ordering instead of being stored as `is_last` on every entry, removing catch-up logic from three mutation paths (`build_tree`, `remove_deleted_from_tree`, `order_tree`)
- `ui::render` takes `&AppState` instead of `&mut AppState` — rendering no longer mutates state

## [0.3.0] - 2026-07-01

### Added
- Language-based project detection with 7 ecosystems (rust, js, python, go, ruby, php, cocoapods)
- Detection via lock files AND config files (pyproject.toml, go.mod, Gemfile, composer.json, etc.)
- Recursive target dir scanning within project subtrees (`__pycache__`, `.venv`, `.tox`)
- Project headers display detected languages (e.g. `my-app (js, rust)`)

### Changed
- Replaced flat `KNOWN_TARGETS` list with per-language `root_target_dirs` and `deep_target_dirs`
- `vendor`, `Pods`, `node_modules` etc. only checked at project root (no false positives from source dirs)
- `build` removed from default target dirs (language-specific now)

### Removed
- Orphan target detection entirely (zero false positive guarantee)

## [0.2.0] - 2026-07-01

### Added
- `dirsweep update` command for self-updating from GitHub releases
- `dirsweep uninstall` command for self-removing the binary
- Cross-platform self-delete/replace via `self-replace` crate
- Retry policy for update downloads (3 attempts on checksum mismatch)
- Release profile optimizations (41% smaller binary: 4.7 MiB → 2.8 MiB)
- Unit tests for commands module (19 tests)
- AGENTS.md for AI-assisted development

### Changed
- Skip `ProjectHeader` entries when navigating with arrow keys
- Walk recursively for orphan target directories

### Fixed
- Orphan target detection for nested directories
- Deduplication logic for overlapping target paths

## [0.1.2] - 2026-06-29

### Changed
- Renamed project from `sweep` to `dirsweep`

### Added
- Multi-channel deployment infrastructure (crates.io, npm)
- Platform-aware install scripts (Linux, macOS, Windows)
- CI checks for format, clippy, and tests

### Fixed
- Scoped secret env vars to jobs with `cicd` environment
- Dry run UI with artificial delay
- Live deletion progress in TUI

### Removed
- Dockerfile and Docker release workflow
- x86_64-apple-darwin target (CI runner hangs)

## [0.1.1] - 2026-06-28

### Fixed
- Include README.md in npm package files
- Regenerate lockfile before publish

### Changed
- Restructured README install section
- Added platform-aware install scripts

## [0.1.0] - 2026-06-27

### Added
- Initial TUI implementation with ratatui + crossterm
- Directory scanning with lock file detection
- Interactive tree view with selection
- Deletion support (trash or permanent)
- Size computation for target directories
- Monorepo detection (pnpm workspaces, Turborepo, Nx, Lerna)
- Support for 12 target directory types
- Support for 9 package managers
