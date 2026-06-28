use std::fs;
use std::path::Path;

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
fn test_scan_finds_multiple_targets() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");

    let app1 = dir.path().join("project-a");
    create_file(&app1.join("package-lock.json"), 50);
    create_dir(&app1.join("node_modules"));
    create_file(&app1.join("node_modules/pkg/index.js"), 2048);
    create_dir(&app1.join("dist"));
    create_file(&app1.join("dist/bundle.js"), 4096);

    let app2 = dir.path().join("project-b");
    create_file(&app2.join("Cargo.lock"), 50);
    create_dir(&app2.join("target"));
    create_file(&app2.join("target/debug/app"), 8192);

    let output = dirsweep::scanner::scan(dir.path());

    assert_eq!(output.target_dirs.len(), 3, "should find 3 target dirs");
    assert_eq!(output.projects.len(), 2, "should find 2 projects");

    // Fast scan returns size=0; verify sizes via scan_target_size
    for dir in &output.target_dirs {
        let (size, _) = dirsweep::scanner::scan_target_size(&dir.path).unwrap();
        assert!(size > 0, "target {:?} should have non-zero size", dir.path);
    }
}

#[test]
fn test_scan_empty_directory() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");

    let output = dirsweep::scanner::scan(dir.path());

    assert!(output.target_dirs.is_empty(), "no targets in empty dir");
    assert!(output.projects.is_empty(), "no projects in empty dir");
    assert!(output.errors.is_empty(), "no errors in empty dir");
}

#[test]
fn test_scan_ignores_dot_git() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    create_dir(&dir.path().join(".git"));
    create_file(&dir.path().join(".git/objects/pack/pack.pack"), 100000);

    let output = dirsweep::scanner::scan(dir.path());

    assert!(output.target_dirs.is_empty());
}

#[test]
fn test_scan_single_project_without_lockfile() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    create_dir(&dir.path().join("node_modules"));
    create_file(&dir.path().join("node_modules/pkg/index.js"), 500);

    let output = dirsweep::scanner::scan(dir.path());

    // node_modules without a lock file should still be found
    assert_eq!(output.target_dirs.len(), 1, "should find node_modules even without lock file");
    assert!(output.projects.is_empty(), "no project detected without lock file");
}

#[test]
fn test_mock_multi_project_scan() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");

    // project-a: npm with node_modules + .next
    create_file(&dir.path().join("project-a/package-lock.json"), 100);
    create_dir(&dir.path().join("project-a/node_modules"));
    create_file(&dir.path().join("project-a/node_modules/lodash/index.js"), 10_000_000);
    create_dir(&dir.path().join("project-a/.next"));
    create_file(&dir.path().join("project-a/.next/build-manifest.json"), 200_000);

    // project-b: yarn with node_modules + vendor
    create_file(&dir.path().join("project-b/yarn.lock"), 100);
    create_dir(&dir.path().join("project-b/node_modules"));
    create_file(&dir.path().join("project-b/node_modules/react/index.js"), 5_000_000);
    create_dir(&dir.path().join("project-b/vendor"));
    create_file(&dir.path().join("project-b/vendor/jquery.js"), 1_000_000);

    // project-c: cargo with target
    create_file(&dir.path().join("project-c/Cargo.lock"), 200);
    create_dir(&dir.path().join("project-c/target"));
    create_file(&dir.path().join("project-c/target/release/app"), 8_000_000);

    // standalone: venv without lock file parent -> unassigned
    create_dir(&dir.path().join("standalone/venv"));
    create_file(&dir.path().join("standalone/venv/bin/python"), 100_000);

    let output = dirsweep::scanner::scan(dir.path());

    assert_eq!(
        output.target_dirs.len(),
        6,
        "should find 6 target dirs total"
    );
    assert_eq!(output.projects.len(), 3, "should detect 3 projects with lock files");
    assert!(output.errors.is_empty(), "no errors expected");

    // Verify project detection
    let project_names: Vec<&str> = output.projects.iter().map(|p| p.name.as_str()).collect();
    assert!(project_names.contains(&"project-a"));
    assert!(project_names.contains(&"project-b"));
    assert!(project_names.contains(&"project-c"));

    // Verify package managers
    let npm_project = output.projects.iter().find(|p| p.name == "project-a").unwrap();
    assert_eq!(npm_project.package_manager.as_deref(), Some("npm"));

    let yarn_project = output.projects.iter().find(|p| p.name == "project-b").unwrap();
    assert_eq!(yarn_project.package_manager.as_deref(), Some("yarn"));

    let cargo_project = output.projects.iter().find(|p| p.name == "project-c").unwrap();
    assert_eq!(cargo_project.package_manager.as_deref(), Some("cargo"));

    // Verify children per project
    assert_eq!(npm_project.children.len(), 2, "project-a should have 2 children");
    assert_eq!(yarn_project.children.len(), 2, "project-b should have 2 children");
    assert_eq!(cargo_project.children.len(), 1, "project-c should have 1 child");

    // Verify sizes via scan_target_size (fast scan returns size=0)
    let total: u64 = output.target_dirs.iter().map(|d| {
        let (size, _) = dirsweep::scanner::scan_target_size(&d.path).unwrap();
        size
    }).sum();
    assert_eq!(total, 24_300_000, "total size should be sum of all files");
}

#[test]
fn test_mock_tree_building() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");

    create_file(&dir.path().join("app/package-lock.json"), 50);
    create_dir(&dir.path().join("app/node_modules"));
    create_file(&dir.path().join("app/node_modules/pkg/index.js"), 1_000_000);
    create_dir(&dir.path().join("app/.next"));
    create_file(&dir.path().join("app/.next/build.json"), 500_000);

    let output = dirsweep::scanner::scan(dir.path());
    let mut state = dirsweep::app::AppState::new(dir.path().to_path_buf());
    state.build_tree(output);

    // Tree should have: ProjectHeader + 2 TargetDirs = 3 entries
    assert_eq!(state.tree.len(), 3, "tree should have 3 entries");

    let header = &state.tree[0];
    assert!(
        matches!(header, dirsweep::app::TreeEntry::ProjectHeader { name, .. } if name == "app"),
        "first entry should be 'app' header, got {header:?}"
    );

    // Check that exactly 2 TargetDir entries exist for node_modules and .next
    let target_count = state.tree.iter().filter(|e| {
        matches!(e, dirsweep::app::TreeEntry::TargetDir { .. })
    }).count();
    assert_eq!(target_count, 2, "should have 2 target dirs");

    let has_node_modules = state.tree.iter().any(|e| {
        matches!(e, dirsweep::app::TreeEntry::TargetDir { path, .. } if path.file_name() == Some(std::ffi::OsStr::new("node_modules")))
    });
    assert!(has_node_modules, "tree should contain node_modules");

    let has_dot_next = state.tree.iter().any(|e| {
        matches!(e, dirsweep::app::TreeEntry::TargetDir { path, .. } if path.file_name() == Some(std::ffi::OsStr::new(".next")))
    });
    assert!(has_dot_next, "tree should contain .next");
}

#[test]
fn test_mock_select_and_deselect() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");

    create_file(&dir.path().join("app/package-lock.json"), 50);
    create_dir(&dir.path().join("app/node_modules"));
    create_file(&dir.path().join("app/node_modules/pkg/index.js"), 2_000_000);
    create_dir(&dir.path().join("app/dist"));
    create_file(&dir.path().join("app/dist/bundle.js"), 1_000_000);

    let output = dirsweep::scanner::scan(dir.path());
    let mut state = dirsweep::app::AppState::new(dir.path().to_path_buf());
    state.build_tree(output);

    // Find indices of target dirs in the tree
    let indices: Vec<usize> = state.tree.iter().enumerate()
        .filter(|(_, e)| matches!(e, dirsweep::app::TreeEntry::TargetDir { .. }))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(indices.len(), 2, "should find 2 target dirs");

    // Toggle first target
    state.list_index = indices[0];
    state.toggle_selection();
    assert_eq!(state.selection_count(), 1);

    // Toggle second target
    state.list_index = indices[1];
    state.toggle_selection();
    assert_eq!(state.selection_count(), 2);

    // Deselect all
    state.deselect_all();
    assert_eq!(state.selection_count(), 0);

    // Select all
    state.select_all();
    assert_eq!(state.selection_count(), 2);
}
