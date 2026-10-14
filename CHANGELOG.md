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

### Changed
- CI clippy now lints all targets (`--all-targets`), covering benches
- `[profile.bench]` sets `opt-level = 3`, overriding the release `-z`, so benchmarks isolate algorithmic change from optimizer behaviour

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
