# dirsweep

A TUI tool for finding and removing bloated project directories that waste disk space.

## What it does

`dirsweep` scans directories for large project dependency/build folders and presents them in an interactive TUI where you can:

- See exactly how much space each directory consumes
- Check when each directory was last modified
- Toggle directories for batch removal
- Delete with a single keystroke

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

## Usage

```bash
# Scan current directory
dirsweep

# Scan a specific directory
dirsweep --dir /path/to/projects
```

## Installation

Choose your method:

```bash
# via cargo (if you have Rust installed)
cargo install dirsweep

# via npm/pnpm (no Rust needed)
npx dirsweep

# via shell script (Linux/macOS)
curl -sSfL https://github.com/farrelaby/dirsweep/raw/main/install.sh | sh

# via Docker (interactive TUI requires -it)
docker run -it ghcr.io/farrelaby/dirsweep [--dir /path]
```
