# dirsweep

A TUI tool for finding and removing bloated project directories that waste disk space. Scan projects, see exactly what's eating your disk, and delete with a single keystroke.

## Install

### npm

Requires Node.js or pnpm installed on your system.

```bash
# for npm user
npx dirsweep
```

```bash
# for pnpm user
pnpx dirsweep
```

### cargo

Requires the Rust toolchain installed via rustup.

```bash
cargo install dirsweep
```

### Shell script

#### Linux

```bash
curl -sSfL https://github.com/farrelaby/dirsweep/raw/main/install.sh | sh
```

Installs to `~/.local/bin` (no sudo needed).

#### macOS

```bash
curl -sSfL https://github.com/farrelaby/dirsweep/raw/main/install.sh | sh
```

Installs to `/usr/local/bin` (may prompt for sudo).

### Windows

```powershell
irm https://github.com/farrelaby/dirsweep/raw/main/install.ps1 | iex
```

Installs to `%USERPROFILE%\.local\bin` and adds to user PATH.

## Usage

```bash
# Scan current directory
dirsweep

# Scan a specific directory
dirsweep --dir /path/to/projects

# Update to latest version
dirsweep update

# Uninstall
dirsweep uninstall
dirsweep uninstall --force  # skip confirmation
```

## TUI preview

```
 dirsweep — /home/user/projects  |  6.8 GiB reclaimable  |  scan 0.6s

   my-app (pnpm)
 ▶ └ [●] node_modules    1.2 GiB   3 days ago
     [●] dist            800 MiB   2 days ago
   rust-project (cargo)
   └ [●] target          4.5 GiB   10 days ago
   my-other-app (npm)
     [ ] .next           350 MiB   just now

 [Space] toggle  [a] all  [d] none  [Enter] delete  [q] quit  3 selected | 6.50 GiB | 1/6
```

## Supported targets

| Directory | Language / Framework |
|-----------|-------------------|
| `node_modules` | Node.js |
| `target` | Rust, Maven (Java) |
| `.next` | Next.js |
| `__pycache__` | Python |
| `.venv` / `venv` | Python virtual environments |
| `dist` / `build` | JS/TS bundlers, Gradle (Java/Kotlin) |
| `vendor` | Go, PHP (Composer) |
| `Pods` | iOS (CocoaPods) |
| `bower_components` | Legacy JavaScript |
| `.tox` | Python (tox) |
| `out` | Kotlin/Java |

### Lock file detection

`dirsweep` detects the language/package manager by scanning for lock files and manifests in the parent directory:

| Lock file | Package manager |
|-----------|----------------|
| `package-lock.json` | npm |
| `pnpm-lock.yaml` | pnpm |
| `bun.lockb` | bun |
| `deno.lock` | deno |
| `yarn.lock` | yarn |
| `Cargo.lock` | Cargo (Rust) |
| `go.sum` | Go |
| `Gemfile.lock` | Bundler (Ruby) |
| `composer.lock` | Composer (PHP) |

### Monorepo detection

`dirsweep` identifies monorepo structures to help you understand what you're deleting:

| File | Monorepo tool |
|------|--------------|
| `pnpm-workspace.yaml` | pnpm workspaces |
| `turbo.json` | Turborepo |
| `nx.json` | Nx |
| `lerna.json` | Lerna |
| `workspaces` in `package.json` | npm/yarn workspaces |
| `Cargo.toml` with `members` | Rust workspace |

Root-level `node_modules` in monorepos are highlighted as they may be shared across packages — deleting them could break the entire workspace.

## Benchmark

`dirsweep` is benchmarked against [`npkill`](https://github.com/voidcosmos/npkill) — the most popular CLI tool for finding and removing `node_modules` directories. Both tools run in TUI mode through a PTY, measuring wall time and peak RSS.

### Results

| Scenario | ds (s) | np (s) | ds (MB) | np (MB) | Result |
|---|---|---|---|---|---|
| Node monorepo | 0.064 | 0.272 | 7 | 171 | **dirsweep 4.2× faster** |
| Multi-language | 0.062 | 0.136 | 7 | 170 | **dirsweep 2.2× faster** |
| 500 tiny projs | 0.098 | 4.430 | 7 | 188 | **dirsweep 45.1× faster** |
| Deep nesting | 0.067 | 0.973 | 7 | 176 | **dirsweep 14.5× faster** |
| Large targets | 0.061 | 0.112 | 7 | 169 | **dirsweep 1.8× faster** |

*Averages of 10 runs on synthetic fixtures (low variance). ds = dirsweep v0.3.0, np = npkill v0.12.2.*

### Scenarios

| # | Name | Description |
|---|------|-------------|
| 1 | Node monorepo | Root project + 20 JS sub-packages, `.next` build output |
| 2 | Multi-language | 5 JS, 5 Rust, 5 Python projects with their respective junk dirs |
| 3 | 500 tiny projs | 500 minimal JS projects, each with 1 KB `node_modules` |
| 4 | Deep nesting | 10 nested projects + 100 random-depth tiny projects |
| 5 | Large targets | 3 JS projects with 100, 200, 300 MB `node_modules` |

### Run it yourself

Requirements:

- **dirsweep** — build from source: `cargo build --release`
- **npkill** — install globally: `npm install -g npkill` (or via `pnpm`/`yarn`)
- **Python 3** — used for PTY automation
- **bash**, `find`, `truncate`, `awk` — standard POSIX tools

```bash
# From the repo root:
bash bench/bench.sh

# Only create fixtures (skip the benchmark):
bash bench/bench.sh --prepare

# Customise:
DIRSWEEP=target/debug/dirsweep RUNS=5 SCENARIOS="1 3" bash bench/bench.sh
```

Fixtures are created in `bench/fixtures/` and left after the run for inspection.
Remove them with `rm -rf bench/fixtures`.

### Benchmarking the core

`bench.sh` measures dirsweep end-to-end against npkill — one wall-clock number.
That answers "is dirsweep fast?", but not "which phase got slower?". For
algorithmic changes to the scan core there is a separate Criterion harness that
times each phase in isolation, plus size computation, which `bench.sh` never
measures because the TUI discards it before it finishes.

```bash
# Build without running (used in CI to stop benches rotting):
cargo bench --bench scan --no-run

# Run everything:
cargo bench --bench scan

# One phase, one scenario:
cargo bench --bench scan -- "scan/many-tiny"
```

Five groups, each over five deterministic scenarios:

| Group | Measures |
|---|---|
| `pipeline/` | `scan` → `build_tree` → first rendered frame. The headline number |
| `scan/` | `scanner::scan` alone — discovery and target assignment |
| `build_tree/` | `AppState::build_tree` — flattening scan output into the render tree |
| `size/` | `scan_target_size` across every target, replicating the worker in `main.rs` |
| `render/` | `ui::render` into a fixed 120x40 `TestBackend`, tree pre-built |

Scenarios: `node-monorepo`, `multi-lang`, `many-tiny` (500 projects),
`deep-nesting`, `dense-targets` (few targets, many files — the one that
stresses `size/`).

Unlike `bench.sh`, fixtures are **deterministic**: a seeded PRNG replaces
`$RANDOM`, so the same scenario always produces the same tree. That is what
makes a before/after comparison meaningful rather than partly noise.

Before starting a rewrite, save a baseline; afterwards, compare against it:

```bash
git checkout main      && cargo bench --bench scan -- --save-baseline before
git checkout <rewrite> && cargo bench --bench scan -- --baseline before
```

Run the two back to back. The same code measured **1.87× apart** across two
sessions on this laptop — a uniform shift from CPU clock and thermal state, not
an algorithmic change. A stale baseline will report a false speedup in every
benchmark. `bench/BASELINE.md` has the measurements and the full procedure.

Criterion reports a significance test per benchmark (regressed / improved /
no change) rather than a bare percentage. HTML reports land in
`target/criterion/`.

Each run also prints a **work counters** table — directories entered, files
seen, subtrees pruned, detection files matched:

```
scenario          targets    files   disc_d   disc_f   pruned   detect    tgt_d
many-tiny             500     1500      501      500      500      500      500
dense-targets           3      753        4        3        3        3        3
```

Wall-clock time on a loaded machine is noisy; work done is not. If a rewrite
cuts `disc_d` by a third, that is a real result even when the timer cannot
resolve it. Note `dense-targets`: 753 files exist but discovery only enters
three, because `node_modules` is pruned rather than walked.

Benches build under `[profile.bench]` with `opt-level = 3`, while the shipped
binary is `opt-level = "z"`. Both sides of a comparison use the bench profile,
so the result isolates algorithmic change from optimizer behaviour — but it is
not a measurement of the binary you ship.
