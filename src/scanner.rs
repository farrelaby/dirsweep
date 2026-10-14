use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use walkdir::WalkDir;

use crate::config;

const SKIP_DIRS: &[&str] = &[".git", ".svn", ".hg"];

/// Whether pass 1 drops this entry entirely — neither counted nor descended into.
///
/// VCS names are skipped regardless of entry type because a git worktree has a `.git`
/// *file*, not a directory. Target directories are skipped only as directories: a file
/// that happens to be named `node_modules` is not one, and hiding it would be a bug.
///
/// Applied in two places on purpose. The walker's `descend` predicate is what refuses to
/// traverse, but it runs on directories only and it still emits the entry; reapplying this
/// in the consumer is what reproduces walkdir's `filter_entry`, which dropped the entry.
fn is_pruned(name: &str, is_dir: bool) -> bool {
    SKIP_DIRS.contains(&name) || (is_dir && config::is_any_target(name))
}

#[derive(Debug, Clone)]
pub struct ScannedDir {
    pub path: PathBuf,
    pub size: u64,
    pub last_modified: DateTime<Utc>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub path: PathBuf,
    pub name: String,
    pub languages: Vec<String>,
    pub children: Vec<PathBuf>,
}

/// Counters describing how much work a scan actually performed.
///
/// Wall-clock time on a loaded machine is noisy; the amount of work done is
/// not. Comparing these across a rewrite tells you whether a speedup came
/// from doing genuinely less work or from something incidental.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    /// Directories entered during discovery (pass 1).
    pub discovery_dirs: u64,
    /// Files seen during discovery (pass 1).
    pub discovery_files: u64,
    /// Subtrees pruned by `filter_entry` (VCS dirs and already-known targets).
    pub pruned_subtrees: u64,
    /// Detection files matched (lock files, manifests, etc).
    pub detection_hits: u64,
    /// Directories opened while searching for targets (pass 2).
    pub target_walk_dirs: u64,
    /// Target directories ultimately claimed.
    pub target_dirs_found: u64,
    /// Wall time spent in pass 1 (the recursive discovery walk), microseconds.
    ///
    /// The three timings below bracket the work `scan` performs, so they
    /// should sum to roughly `scan` itself. They exist to say *which* pass
    /// dominates before choosing a parallelism strategy for it — a walker
    /// only affects pass 1, and pass 2 is hand-rolled recursion.
    pub pass1_us: u64,
    /// Wall time turning detection hits into sorted projects, microseconds.
    pub project_build_us: u64,
    /// Wall time in pass 2 (target claiming), microseconds.
    pub pass2_us: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ScanOutput {
    pub target_dirs: Vec<ScannedDir>,
    pub projects: Vec<ProjectInfo>,
    pub errors: Vec<String>,
    pub stats: ScanStats,
}

pub fn scan(base_path: &Path) -> ScanOutput {
    let mut detection_files: HashMap<PathBuf, Vec<&str>> = HashMap::new();
    let mut errors: Vec<String> = Vec::new();
    let mut stats = ScanStats::default();

    let t0 = Instant::now();

    // Pass 1: Walk tree to find detection files.
    //
    // dua-core walks directory reads across a work-stealing pool instead of one thread.
    // skip_metadata is not optional here: pass 1 only uses file_name, file_type and
    // path().parent(), so collecting metadata would add a stat per entry that walkdir
    // never made.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let walk = dua_core::walk(
        base_path,
        threads,
        dua_core::Order::ParentFirst,
        dua_core::Options::default().skip_metadata(),
        |entry: &dua_core::Entry| {
            !is_pruned(&entry.file_name.to_string_lossy(), entry.file_type.is_dir())
        },
    );

    // dua yields a directory whose descend returned false, where walkdir's filter_entry
    // dropped it entirely. Filtering here as well keeps discovery_dirs, discovery_files
    // and pruned_subtrees exactly as they were, which is what makes the benchmark counter
    // table comparable across the rewrite.
    let mut pruned = 0u64;
    for entry in walk {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("Cannot access: {}", e));
                continue;
            }
        };

        let name = entry.file_name.to_string_lossy();
        if is_pruned(&name, entry.file_type.is_dir()) {
            pruned += 1;
            continue;
        }

        if entry.file_type.is_dir() {
            stats.discovery_dirs += 1;
        } else {
            stats.discovery_files += 1;
        }

        if entry.file_type.is_file()
            && let Some(lang) = config::is_detection_file(&name)
            && let Some(parent) = entry.path().parent()
        {
            stats.detection_hits += 1;
            detection_files
                .entry(parent.to_path_buf())
                .or_default()
                .push(lang);
        }
    }

    stats.pruned_subtrees = pruned;
    stats.pass1_us = t0.elapsed().as_micros() as u64;

    // Build projects
    let t1 = Instant::now();
    let mut projects: Vec<ProjectInfo> = detection_files
        .into_iter()
        .map(|(path, languages)| {
            let mut seen = HashSet::new();
            let mut languages: Vec<&str> =
                languages.into_iter().filter(|l| seen.insert(*l)).collect();
            languages.sort();

            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            ProjectInfo {
                path,
                name,
                languages: languages.into_iter().map(|s| s.to_string()).collect(),
                children: Vec::new(),
            }
        })
        .collect();

    // Sort projects deepest first for ancestor-based assignment
    projects.sort_by_key(|p| std::cmp::Reverse(p.path.components().count()));
    stats.project_build_us = t1.elapsed().as_micros() as u64;

    let t2 = Instant::now();

    // Pass 2: Find target dirs in each project's subtree.
    // Process deepest projects first so they claim their targets,
    // and shallower projects skip areas already claimed.
    let mut claimed: HashSet<PathBuf> = HashSet::new();
    let mut claimed_projects: HashSet<PathBuf> = HashSet::new();
    let mut target_dirs: Vec<ScannedDir> = Vec::new();

    for project in &mut projects {
        let lang_names: Vec<&str> = project.languages.iter().map(|s| s.as_str()).collect();
        let root_targets = config::root_target_dirs_for_languages(&lang_names);
        let deep_targets = config::deep_target_dirs_for_languages(&lang_names);

        let mut found: Vec<PathBuf> = Vec::new();

        if !root_targets.is_empty() {
            find_target_dirs(
                &project.path,
                &root_targets,
                &claimed_projects,
                false,
                &mut found,
                &mut errors,
                &mut stats,
            );
        }

        if !deep_targets.is_empty() {
            find_target_dirs(
                &project.path,
                &deep_targets,
                &claimed_projects,
                true,
                &mut found,
                &mut errors,
                &mut stats,
            );
        }

        for target_path in found {
            if claimed.insert(target_path.clone()) {
                target_dirs.push(ScannedDir {
                    path: target_path.clone(),
                    size: 0,
                    last_modified: DateTime::UNIX_EPOCH,
                    error: None,
                });
                project.children.push(target_path);
            }
        }

        claimed_projects.insert(project.path.clone());
    }

    stats.pass2_us = t2.elapsed().as_micros() as u64;
    stats.target_dirs_found = target_dirs.len() as u64;

    ScanOutput {
        target_dirs,
        projects,
        errors,
        stats,
    }
}

/// Walk `dir` looking for directories whose name is in `target_names`.
/// When `recurse` is true, walk the full subtree; otherwise only check direct children.
/// Skips VCS dirs and does not descend into paths already in `skip_projects`.
fn find_target_dirs(
    dir: &Path,
    target_names: &[&str],
    skip_projects: &HashSet<PathBuf>,
    recurse: bool,
    results: &mut Vec<PathBuf>,
    errors: &mut Vec<String>,
    stats: &mut ScanStats,
) {
    stats.target_walk_dirs += 1;

    let top_level = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            errors.push(format!("Cannot read {:?}: {}", dir, e));
            return;
        }
    };

    for entry in top_level.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }

        let entry_path = entry.path();

        if skip_projects.contains(&entry_path) {
            continue;
        }

        if target_names.contains(&name.as_str()) {
            results.push(entry_path);
        } else if recurse {
            find_target_dirs(
                &entry_path,
                target_names,
                skip_projects,
                recurse,
                results,
                errors,
                stats,
            );
        }
    }
}

pub fn scan_target_size(path: &Path) -> Result<(u64, DateTime<Utc>), std::io::Error> {
    let dir = scan_target(path)?;
    Ok((dir.size, dir.last_modified))
}

fn scan_target(path: &Path) -> Result<ScannedDir, std::io::Error> {
    let mut size = 0u64;
    let mut last_modified: Option<std::time::SystemTime> = None;
    let mut errors: Vec<String> = Vec::new();

    let meta = std::fs::metadata(path)?;
    if let Ok(m) = meta.modified() {
        last_modified = Some(m);
    }

    if meta.is_dir() {
        for entry in WalkDir::new(path).into_iter() {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    errors.push(format!("Cannot access {:?}: {}", e.path(), e));
                    continue;
                }
            };

            let file_meta = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    errors.push(format!("Cannot read metadata {:?}: {}", entry.path(), e));
                    continue;
                }
            };

            if file_meta.is_file() {
                size += file_meta.len();
            }

            if let Ok(m) = file_meta.modified()
                && last_modified.is_none_or(|lm| m > lm)
            {
                last_modified = Some(m);
            }
        }
    }

    let dt = last_modified
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| DateTime::from_timestamp(d.as_secs() as i64, d.subsec_nanos()))
        .unwrap_or(DateTime::UNIX_EPOCH);

    let error = if errors.is_empty() {
        None
    } else {
        Some(errors.join("; "))
    };

    Ok(ScannedDir {
        path: path.to_path_buf(),
        size,
        last_modified: dt,
        error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("failed to create temp dir")
    }

    fn create_dir(path: &Path) {
        fs::create_dir_all(path).expect("failed to create dir");
    }

    fn create_file(path: &Path, size: u64) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent");
        }
        let content = vec![0u8; size as usize];
        fs::write(path, content).expect("failed to write file");
    }

    #[test]
    fn test_scan_finds_node_modules() {
        let dir = create_test_dir();
        let project = dir.path().join("my-app");
        create_file(&project.join("package-lock.json"), 100);
        create_dir(&project.join("node_modules"));
        create_file(&project.join("node_modules/lib/index.js"), 1024);

        let output = scan(dir.path());
        assert_eq!(output.target_dirs.len(), 1);
        assert_eq!(output.target_dirs[0].path, project.join("node_modules"));
        assert_eq!(output.target_dirs[0].size, 0);
    }

    #[test]
    fn test_scan_stats_counts_work() {
        let dir = create_test_dir();
        let project = dir.path().join("my-app");
        create_file(&project.join("package-lock.json"), 50);
        create_dir(&project.join("node_modules"));
        create_file(&project.join("node_modules/pkg/index.js"), 1024);

        let output = scan(dir.path());

        assert_eq!(output.stats.detection_hits, 1);
        assert_eq!(output.stats.target_dirs_found, 1);
        // node_modules is pruned, so it is counted as pruned and never walked.
        assert!(output.stats.pruned_subtrees >= 1);
        // Root + my-app are entered; node_modules is not.
        assert_eq!(output.stats.discovery_dirs, 2);
        assert_eq!(output.stats.discovery_files, 1);
    }

    /// A git worktree has a `.git` *file*, not a directory.
    ///
    /// The walker's `descend` predicate only ever sees directories, so a `.git` file is
    /// never offered to it and would otherwise be counted as discovered. walkdir's
    /// `filter_entry` pruned it regardless of type; this pins the consumer filter that
    /// restores that.
    #[test]
    fn test_scan_prunes_git_worktree_file() {
        let dir = create_test_dir();
        let project = dir.path().join("wt");
        create_file(&project.join(".git"), 32);
        create_file(&project.join("package.json"), 10);

        let output = scan(dir.path());

        assert_eq!(output.stats.discovery_dirs, 2, "root + wt");
        assert_eq!(
            output.stats.discovery_files, 1,
            "package.json only — the .git file must be dropped, not discovered"
        );
        assert_eq!(output.stats.pruned_subtrees, 1);
    }

    #[test]
    fn test_scan_phase_timings_cover_scan() {
        let dir = create_test_dir();
        let project = dir.path().join("my-app");
        create_file(&project.join("package-lock.json"), 50);
        create_dir(&project.join("node_modules"));
        create_file(&project.join("node_modules/pkg/index.js"), 1024);

        let t = Instant::now();
        let output = scan(dir.path());
        let total = t.elapsed().as_micros() as u64;

        let s = output.stats;
        let split = s.pass1_us + s.project_build_us + s.pass2_us;

        // The three marks all sit inside `scan`, so together they cannot
        // exceed it — a violation means the marks overlap and the reported
        // split is meaningless. Slack covers coarse clock granularity.
        assert!(
            split <= total + 100,
            "phase sum {split}us exceeds whole scan {total}us"
        );
    }

    #[test]
    fn test_scan_detects_language() {
        let dir = create_test_dir();
        let project = dir.path().join("my-app");
        create_file(&project.join("pnpm-lock.yaml"), 50);
        create_dir(&project.join("node_modules"));
        create_file(&project.join("node_modules/pkg/index.js"), 512);

        let output = scan(dir.path());
        assert_eq!(output.projects.len(), 1);
        assert_eq!(output.projects[0].languages, vec!["js"]);
        assert!(!output.target_dirs.is_empty());
    }

    #[test]
    fn test_scan_ignores_non_targets() {
        let dir = create_test_dir();
        create_dir(&dir.path().join("src"));
        create_file(&dir.path().join("src/main.rs"), 200);
        create_dir(&dir.path().join(".git"));
        create_file(&dir.path().join(".git/config"), 100);

        let output = scan(dir.path());
        assert!(output.target_dirs.is_empty());
    }

    #[test]
    fn test_scan_multiple_projects() {
        let dir = create_test_dir();
        let app1 = dir.path().join("app1");
        let app2 = dir.path().join("app2");
        create_file(&app1.join("package-lock.json"), 50);
        create_dir(&app1.join("node_modules"));
        create_file(&app1.join("node_modules/pkg/index.js"), 1024);
        create_file(&app2.join("Cargo.toml"), 50);
        create_dir(&app2.join("target"));
        create_file(&app2.join("target/debug/app"), 2048);

        let output = scan(dir.path());
        assert_eq!(output.target_dirs.len(), 2);
        assert_eq!(output.projects.len(), 2);
    }

    #[test]
    fn test_scan_multiple_targets_same_project() {
        let dir = create_test_dir();
        let project = dir.path().join("my-app");
        create_file(&project.join("package-lock.json"), 50);
        create_dir(&project.join("node_modules"));
        create_file(&project.join("node_modules/pkg/index.js"), 500);
        create_dir(&project.join(".next"));
        create_file(&project.join(".next/bundle.js"), 1000);

        let output = scan(dir.path());
        assert_eq!(output.target_dirs.len(), 2);
        assert_eq!(output.projects.len(), 1);
        assert_eq!(output.projects[0].children.len(), 2);
    }

    #[test]
    fn test_scan_target_size() {
        let dir = create_test_dir();
        let target = dir.path().join("node_modules");
        create_dir(&target);
        create_file(&target.join("lib/index.js"), 1024);

        let (size, _) = scan_target_size(&target).unwrap();
        assert!(size >= 1024);
    }

    /// Hidden files inside a target dir must contribute to its size.
    ///
    /// Several parallel walkers (jwalk, dua's entry point) skip dotfiles by
    /// default. That failure is silent — sizes come out low but plausible, and
    /// the ordinary size test above still passes because it only checks a
    /// visible file. This pins the behaviour before any walker swap.
    #[test]
    fn test_scan_target_size_counts_hidden_files() {
        let dir = create_test_dir();
        let target = dir.path().join("node_modules");
        create_dir(&target);
        create_file(&target.join("visible.js"), 1_000);
        create_file(&target.join(".cache/hidden.bin"), 500);

        let (size, _) = scan_target_size(&target).unwrap();
        assert!(
            size >= 1_500,
            "hidden files must be counted, got {size} — walker is skipping dotfiles"
        );
    }

    #[test]
    fn test_scan_without_lock_file_finds_nothing() {
        let dir = create_test_dir();
        create_dir(&dir.path().join("vendor"));
        create_file(&dir.path().join("vendor/dep.js"), 512);

        let output = scan(dir.path());
        assert!(
            output.target_dirs.is_empty(),
            "vendor without detection file should not be found"
        );
    }

    #[test]
    fn test_scan_finds_nested_targets_in_subtree() {
        let dir = create_test_dir();
        let project = dir.path().join("my-app");
        create_file(&project.join("pyproject.toml"), 50);
        create_dir(&project.join("src"));
        create_dir(&project.join("src/mypkg/__pycache__"));
        create_file(
            &project.join("src/mypkg/__pycache__/module.cpython-312.pyc"),
            256,
        );
        create_dir(&project.join("tests/__pycache__"));
        create_file(&project.join("tests/__pycache__/test.cpython-312.pyc"), 128);

        let output = scan(dir.path());

        let paths: Vec<_> = output.target_dirs.iter().map(|d| d.path.clone()).collect();
        assert_eq!(output.target_dirs.len(), 2);
        assert!(paths.contains(&project.join("src/mypkg/__pycache__")));
        assert!(paths.contains(&project.join("tests/__pycache__")));
    }

    #[test]
    fn test_scan_does_not_double_count_nested_targets() {
        let dir = create_test_dir();
        create_file(&dir.path().join("package-lock.json"), 50);
        create_dir(&dir.path().join("node_modules/dep/node_modules"));
        create_file(
            &dir.path()
                .join("node_modules/dep/node_modules/deep/index.js"),
            256,
        );

        let output = scan(dir.path());
        assert_eq!(output.target_dirs.len(), 1);
        assert_eq!(output.target_dirs[0].path, dir.path().join("node_modules"));
    }

    #[test]
    fn test_scan_monorepo_nested_projects() {
        let dir = create_test_dir();
        create_file(&dir.path().join("package-lock.json"), 50);
        create_dir(&dir.path().join("node_modules"));
        create_file(&dir.path().join("node_modules/pkg/index.js"), 1024);

        create_dir(&dir.path().join("packages/foo"));
        create_file(&dir.path().join("packages/foo/package-lock.json"), 30);
        create_dir(&dir.path().join("packages/foo/node_modules"));
        create_file(
            &dir.path().join("packages/foo/node_modules/sub/index.js"),
            512,
        );

        let output = scan(dir.path());

        assert_eq!(output.target_dirs.len(), 2);
        assert_eq!(output.projects.len(), 2);

        let foo_project = output.projects.iter().find(|p| p.name == "foo").unwrap();
        assert_eq!(foo_project.children.len(), 1);
    }

    #[test]
    fn test_scan_go_vendor_root_only() {
        let dir = create_test_dir();
        let project = dir.path().join("proj");
        create_file(&project.join("go.mod"), 20);
        create_dir(&project.join("vendor"));
        create_file(&project.join("vendor/dep/index.js"), 1024);
        create_dir(&project.join("src/vendor"));
        create_file(&project.join("src/vendor/own_code.js"), 512);

        let output = scan(dir.path());

        assert_eq!(output.target_dirs.len(), 1);
        assert_eq!(output.target_dirs[0].path, project.join("vendor"));
        assert_eq!(output.projects.len(), 1);
        assert_eq!(output.projects[0].children.len(), 1);
        assert_eq!(output.projects[0].children[0], project.join("vendor"));
    }
}
