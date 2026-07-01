use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Utc};
use walkdir::WalkDir;

use crate::config;

const SKIP_DIRS: &[&str] = &[".git", ".svn", ".hg"];

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

#[derive(Debug, Default)]
pub struct ScanOutput {
    pub target_dirs: Vec<ScannedDir>,
    pub projects: Vec<ProjectInfo>,
    pub errors: Vec<String>,
}

pub fn scan(base_path: &Path) -> ScanOutput {
    let mut detection_files: HashMap<PathBuf, Vec<&str>> = HashMap::new();
    let mut errors: Vec<String> = Vec::new();

    // Pass 1: Walk tree to find detection files.
    // filter_entry skips entering known target dirs and VCS dirs entirely.
    let walk = WalkDir::new(base_path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !(SKIP_DIRS.contains(&name.as_ref())
                || e.file_type().is_dir() && config::is_any_target(name.as_ref()))
        });

    for entry in walk {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("Cannot access: {}", e));
                continue;
            }
        };

        let file_name = entry.file_name().to_string_lossy().to_string();

        if entry.file_type().is_file()
            && let Some(lang) = config::is_detection_file(&file_name)
            && let Some(parent) = entry.path().parent()
        {
            detection_files
                .entry(parent.to_path_buf())
                .or_default()
                .push(lang);
        }
    }

    // Build projects
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

    ScanOutput {
        target_dirs,
        projects,
        errors,
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
) {
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
