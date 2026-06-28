# Design

## Layout

```
┌───────────────────────────────────────────────────────────┐
│  dirsweep — scanning /path/to/projects      Ctrl+C quit   │
├───────────────────────────────────────────────────────────┤
│                                                           │
│  Project                         Size      Modified       │
│  ───────────────────────────────────────────────────────── │
│  my-app (pnpm)                                            │
│    ├─ node_modules          [●]  1.2 GB    2 days ago     │
│    └─ .next                 [ ]  500 MB    1 week ago     │
│  blog (npm)                                               │
│    └─ node_modules          [●]  800 MB    5 days ago     │
│  rust-cli (cargo)                                         │
│    └─ target                [ ]  3.2 GB    3 hours ago    │
│                                                           │
├───────────────────────────────────────────────────────────┤
│  [Space] toggle  [a] all  [d] none  [Enter] delete       │
│  2 selected  ●  2.0 GB reclaimable                       │
└───────────────────────────────────────────────────────────┘
```

### Sections

| Section | Content |
|---------|---------|
| Header bar | App name, scan path, quit hint |
| Tree (main) | Project-grouped directory list with toggles |
| Status bar | Keybinding hints, selection summary |

## Colors

| Element | Color | Usage |
|---------|-------|-------|
| Normal row | Default | Non-selected directories |
| Selected row | Green (bold) | Toggled for deletion |
| Parent row | Cyan (dim) | Project name, not selectable |
| Warning | Yellow | Permission errors, warnings |
| Danger | Red (bold) | Error messages, permanent delete warning |
| Accent | Blue | Focus highlight, keybinding hints |

## Keybindings

| Key | Action |
|-----|--------|
| `j` / `↓` | Move cursor down |
| `k` / `↑` | Move cursor up |
| `Space` | Toggle selection of current item |
| `a` | Select all |
| `d` | Deselect all |
| `Enter` | Confirm and proceed to deletion |
| `q` / `Esc` | Quit (browsing) or go back (confirm dialog) |

## Tree structure

### Grouping logic

Directories with a common parent project are grouped together. Two directories belong to the same project if they share the same parent directory containing a recognized lock/manifest file.

```
my-app/                        ← project root (detected via lock file)
  ├── package.json
  ├── pnpm-lock.yaml
  ├── node_modules/            ← target
  └── .next/                   ← target
```

### Row types

| Row | Togglable | Description |
|-----|-----------|-------------|
| Project name | No | Cyan, dim, shows project name and package manager in parens |
| Target leaf | Yes | Toggled with Space, size and date shown inline |

### Partial selection state

When a parent project has some children selected and others not, the parent row shows a partial indicator. This is purely visual — parent rows themselves are never togglable.

## Symbols

| Symbol | Meaning |
|--------|---------|
| `[●]` | Selected |
| `[ ]` | Not selected |
| `├──` | Intermediate child |
| `└──` | Last child |
| ` ⚠ ` | Warning (permission error, etc.) |
| ` ✗ ` | Error (failed to scan) |

## Phases

### 1. Scanning

```
dirsweep — scanning /home/projects
────────────────────────────────
  Scanning...  Found 12 directories ████████░░ 85%
```

- Spinner animation while scanning
- Counter showing directories found so far
- Progress bar for large scans (if feasible)

### 2. Browsing

The main tree view as described above. Default ordering is by project name (A → Z).

### 3. Confirm delete

```
┌──────────────────────────────────────────────────┐
│  Delete 3 items?                                  │
│                                                   │
│  node_modules  (my-app, 1.2 GB)                  │
│  node_modules  (blog,  800 MB)                    │
│  target        (rust-cli, 3.2 GB)                 │
│  ────────────────────────────────────────         │
│  Total: 5.2 GB                                    │
│                                                   │
│  Mode: [●] Trash    [ ] Permanent                 │
│                                                   │
│  [Enter] confirm    [Esc] cancel                  │
└──────────────────────────────────────────────────┘
```

- Overlays the browsing view (dimmed background)
- Lists each selected item with project context
- Shows total reclaimable size
- Toggle between Trash (default) and Permanent
- Permanent adds a second confirmation step

### 4. Deleting

```
Deleting...  ██████░░░░  2/3  (target — rust-cli)
```

- Shows current item being deleted
- Progress counter
- On failure: shows error and continues to next item

### 5. Done

```
Done!  ✔  3 deleted,  5.2 GB reclaimed
        ⚠  1 failed (target — rust-cli: Permission denied)
```

- Quick summary (auto-dismiss after 3 seconds, or press any key to return)
- Success and failure counts

## Interaction feedback

- **Selection change**: status bar updates immediately ("3 selected ● 4.1 GB reclaimable")
- **Permission errors**: marked with `⚠` icon, shown inline with the directory name
- **Empty state**: "No sweepable directories found" message when scan returns nothing
- **Ordering**: press `o` to reorder by name, date, or size (ascending/descending)
