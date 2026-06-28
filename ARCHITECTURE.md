# Architecture

## Overview

`dirsweep` is a TUI tool for finding and removing bloated project directories. It scans a target directory, identifies dependency/build folders, and presents them in an interactive interface for bulk deletion.

## Project structure

```
src/
├── main.rs       -- Entry point, CLI parsing
├── app.rs        -- Application state, user interaction logic
├── scanner.rs    -- Filesystem scanning, language/package manager detection
├── ui.rs         -- TUI rendering with Ratatui
└── config.rs     -- Target definitions, settings
```

### Module responsibilities

| Module | Responsibility |
|--------|---------------|
| `main.rs` | Initialize CLI args, set up terminal, run app loop |
| `app.rs` | Hold scan results, manage selection state, handle key events |
| `scanner.rs` | Walk directories, detect languages, calculate sizes |
| `ui.rs` | Render panels, tables, confirmation dialogs |
| `config.rs` | Define target directories, detection rules, user preferences |

## Data flow

```
CLI args (target dir)
       ↓
   scanner.rs ──── walkdir scan
       ↓
   HashMap<PathBuf, DirInfo> (results, indexed by path)
       ↓
   Vec<PathBuf> (ordered keys for display)
       ↓
   HashSet<PathBuf> (user selections)
       ↓
   ui.rs ──── render TUI
       ↓
   User interaction (toggle, confirm, delete)
```

Scan results are stored in a `HashMap` for O(1) lookups by path. A separate `Vec<PathBuf>` maintains display order (sorted by size/name/modified). User selections are tracked in a `HashSet` for fast toggle checks.

## Data structures

### `DirInfo`

Stores metadata for each scanned directory:

```rust
pub struct DirInfo {
    pub path: PathBuf,
    pub size: u64,             // bytes
    pub last_modified: DateTime<Utc>,
    pub lang: Option<String>,  // detected language
    pub pkg_manager: Option<String>,  // detected package manager
    pub is_monorepo_root: bool,
    pub error: Option<String>, // permission error, etc.
}
```

### `AppState`

Holds all application state:

```rust
pub struct AppState {
    pub results: HashMap<PathBuf, DirInfo>,  // scan results indexed by path
    pub keys: Vec<PathBuf>,                  // ordered display keys
    pub selected: HashSet<PathBuf>,          // user toggled selections
    pub input_dir: PathBuf,                  // the directory being scanned
    pub phase: AppPhase,                     // current state of the app
    pub total_size: u64,                     // total size of all selected items
    pub errors: Vec<ScannedDirError>,        // permission failures, etc.
}
```

### `AppPhase`

```rust
pub enum AppPhase {
    Scanning,        // scanning in progress
    Browsing,        // normal browsing and selection
    ConfirmDelete,   // confirmation dialog before deletion
    Deleting,        // deletion in progress
    Done,            // deletion completed, show summary
}
```

### `DeletePreference`

Configured or chosen at confirmation time:

```rust
pub enum DeletePreference {
    DryRun,     // show what would be deleted, don't actually delete
    Trash,      // move to system trash (safe, default)
    Permanent,  // permanently delete (with extra warning)
}
```

## Scanner

### Detection strategy

1. Walk the target directory using `walkdir`
2. For each directory, check if its name matches known targets (e.g. `node_modules`, `target`)
3. Detect language/package manager by scanning parent directory for lock files
4. Detect monorepo status by checking for workspace config files
5. Calculate directory size recursively
6. Record last modified timestamp

### Lock file detection

| Lock file | Package manager |
|-----------|----------------|
| `package-lock.json` | npm |
| `pnpm-lock.yaml` | pnpm |
| `bun.lockb` | bun |
| `deno.lock` | deno |
| `yarn.lock` | yarn |
| `Cargo.lock` | Cargo |
| `go.sum` | Go |
| `Gemfile.lock` | Bundler |
| `composer.lock` | Composer |

### Monorepo detection

| File | Monorepo tool |
|------|--------------|
| `pnpm-workspace.yaml` | pnpm workspaces |
| `turbo.json` | Turborepo |
| `nx.json` | Nx |
| `lerna.json` | Lerna |
| `workspaces` in `package.json` | npm/yarn workspaces |
| `Cargo.toml` with `members` | Rust workspace |

## Error handling

### Permission errors

- Visually mark directories that cannot be accessed (e.g. red icon)
- Store errors in `AppState.errors` and show in a dedicated panel
- Continue scanning other directories — a single permission error should not abort the entire scan

### Partial failures during deletion

- If one directory fails to delete, log the error and continue with others
- Report all failures at the end in a summary dialog

## Deletion implementation

### Trash (default)

Use the `trash` crate to move directories to the system trash. This allows users to recover accidentally deleted data.

```rust
trash::delete(path)?;
```

### Permanent delete

Use `std::fs::remove_dir_all` for permanent deletion. Only allowed after explicit user confirmation.

```rust
std::fs::remove_dir_all(path)?;
```

## Safety

### Confirmation flow

```
Browsing → [Enter] → ConfirmDelete phase
  → Show selected items + total size
  → User picks: DryRun | Trash | Permanent | Cancel
  → Execute or return to Browsing
```

### Dry-run

- `--dry-run` flag skips actual deletion
- Runs through the same confirmation flow
- Shows "What would have been deleted" summary
- Useful for testing before real runs

### Deletion warnings

- Trash: one confirmation required
- Permanent: two-step confirmation (select Permanent, then confirm again)

## Concurrency

### Current (v0.1)

- Synchronous scanning with `walkdir`
- Works fine for most use cases

### Future consideration

- Async scanning with `tokio` for large directories (deferred until performance is a real bottleneck)
- Show progress indicator during long scans

## Dependencies

| Crate | Purpose |
|-------|---------|
| `ratatui` | TUI framework |
| `crossterm` | Terminal backend |
| `walkdir` | Recursive directory traversal |
| `clap` | CLI argument parsing |
| `chrono` | Timestamp handling and formatting |
| `humansize` | Human-readable file size formatting |
| `trash` | Move to system trash |

## Testing strategy

- `scanner.rs`: unit tests with synthetic directory trees
- `app.rs`: unit tests for selection logic, phase transitions
- `ui.rs`: snapshot tests or integration tests using `ratatui` backends
- End-to-end: create temp dirs, run scan, toggle, verify selection state

## Config

### Current (v0.1)

Target directories and lock file definitions are compile-time constants in `config.rs`:

```rust
// Per-project directories only (user-level caches excluded)
pub const KNOWN_TARGETS: &[&str] = &[
    "node_modules", "target", ".next", "__pycache__",
    ".venv", "venv", "dist", "build", "vendor",
    "Pods", "bower_components", ".tox", "out",
];
```

### Future

User-defined targets via a config file (e.g. `~/.config/dirsweep/targets.toml`).
