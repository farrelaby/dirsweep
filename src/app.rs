use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::scanner::{ScanOutput, ScannedDir};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppPhase {
    Scanning,
    Browsing,
    ConfirmDelete,
    Deleting,
    ConfirmQuit,
    OrderDialog,
    Quit
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeletePreference {
    DryRun,
    Trash,
    Permanent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrderBy {
    NameAsc,
    NameDesc,
    DateAsc,
    DateDesc,
    SizeAsc,
    SizeDesc,
}

impl OrderBy {
    pub fn label(&self) -> &str {
        match self {
            OrderBy::NameAsc => "Name (A \u{2192} Z)",
            OrderBy::NameDesc => "Name (Z \u{2192} A)",
            OrderBy::DateAsc => "Oldest first",
            OrderBy::DateDesc => "Newest first",
            OrderBy::SizeAsc => "Smallest first",
            OrderBy::SizeDesc => "Largest first",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            OrderBy::NameAsc => OrderBy::NameDesc,
            OrderBy::NameDesc => OrderBy::DateAsc,
            OrderBy::DateAsc => OrderBy::DateDesc,
            OrderBy::DateDesc => OrderBy::SizeAsc,
            OrderBy::SizeAsc => OrderBy::SizeDesc,
            OrderBy::SizeDesc => OrderBy::NameAsc,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            OrderBy::NameAsc => OrderBy::SizeDesc,
            OrderBy::NameDesc => OrderBy::NameAsc,
            OrderBy::DateAsc => OrderBy::NameDesc,
            OrderBy::DateDesc => OrderBy::DateAsc,
            OrderBy::SizeAsc => OrderBy::DateDesc,
            OrderBy::SizeDesc => OrderBy::SizeAsc,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TreeEntry {
    ProjectHeader {
        name: String,
        package_manager: Option<String>,
    },
    TargetDir {
        path: PathBuf,
        size: u64,
        last_modified: DateTime<Utc>,
        package_manager: Option<String>,
        is_last: bool,
    },
}

#[derive(Debug)]
pub struct AppState {
    pub tree: Vec<TreeEntry>,
    pub selected: HashSet<PathBuf>,
    pub phase: AppPhase,
    pub delete_preference: DeletePreference,
    pub scan_path: PathBuf,
    pub errors: Vec<String>,
    pub list_index: usize,
    pub total_selected_size: u64,
    pub total_deleted_count: u64,
    pub total_deleted_size: u64,
    pub total_reclaimable: u64,
    pub scan_duration_ms: u64,
    pub delete_result_summary: Option<String>,
    pub sizes_found: usize,
    pub sizes_total: usize,
    pub sizes_complete: bool,
    pub ordered_by: OrderBy,
    pub order_cursor: OrderBy,
    pub deleting_paths: Vec<PathBuf>,
    pub deleting_index: usize,
    pub deleting_failed: Vec<PathBuf>,
    target_index: HashMap<PathBuf, usize>,
}

impl AppState {
    pub fn new(scan_path: PathBuf) -> Self {
        Self {
            tree: Vec::new(),
            selected: HashSet::new(),
            phase: AppPhase::Scanning,
            delete_preference: DeletePreference::Trash,
            scan_path,
            errors: Vec::new(),
            list_index: 0,
            total_selected_size: 0,
            total_deleted_count: 0,
            total_deleted_size: 0,
            total_reclaimable: 0,
            scan_duration_ms: 0,
            delete_result_summary: None,
            sizes_found: 0,
            sizes_total: 0,
            sizes_complete: false,
            ordered_by: OrderBy::NameAsc,
            order_cursor: OrderBy::NameAsc,
            deleting_paths: Vec::new(),
            deleting_index: 0,
            deleting_failed: Vec::new(),
            target_index: HashMap::new(),
        }
    }

    pub fn build_tree(&mut self, output: ScanOutput) {
        let mut tree: Vec<TreeEntry> = Vec::new();

        for project in &output.projects {
            let mut subtree: Vec<TreeEntry> = Vec::new();

            let children: Vec<&ScannedDir> = output
                .target_dirs
                .iter()
                .filter(|d| project.children.contains(&d.path))
                .collect();

            if children.is_empty() {
                continue;
            }

            for (i, child) in children.iter().enumerate() {
                let is_last = i == children.len() - 1;
                subtree.push(TreeEntry::TargetDir {
                    path: child.path.clone(),
                    size: child.size,
                    last_modified: child.last_modified,
                    package_manager: child.package_manager.clone(),
                    is_last,
                });
            }

            subtree.insert(
                0,
                TreeEntry::ProjectHeader {
                    name: project.name.clone(),
                    package_manager: project.package_manager.clone(),
                },
            );

            tree.extend(subtree);
        }

        let unassigned: Vec<&ScannedDir> = output
            .target_dirs
            .iter()
            .filter(|d| {
                !output
                    .projects
                    .iter()
                    .any(|p| p.children.contains(&d.path))
            })
            .collect();

        if !unassigned.is_empty() {
            let mut by_parent: HashMap<PathBuf, Vec<&ScannedDir>> = HashMap::new();
            for dir in &unassigned {
                if let Some(parent) = dir.path.parent() {
                    by_parent.entry(parent.to_path_buf()).or_default().push(dir);
                }
            }

            for dirs in by_parent.values() {
                let parent_name = dirs[0]
                    .path
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string());

                tree.push(TreeEntry::ProjectHeader {
                    name: parent_name,
                    package_manager: None,
                });

                for (i, child) in dirs.iter().enumerate() {
                    let is_last = i == dirs.len() - 1;
                    tree.push(TreeEntry::TargetDir {
                        path: child.path.clone(),
                        size: child.size,
                        last_modified: child.last_modified,
                        package_manager: child.package_manager.clone(),
                        is_last,
                    });
                }
            }
        }

        self.sizes_total = tree.iter().filter(|e| matches!(e, TreeEntry::TargetDir { .. })).count();
        self.sizes_found = 0;
        self.sizes_complete = false;
        self.tree = tree;
        self.errors = output.errors;
        self.total_reclaimable = self.tree.iter().filter_map(|e| {
            if let TreeEntry::TargetDir { size, .. } = e { Some(*size) } else { None }
        }).sum();
        self.rebuild_target_index();
        self.clamp_index();
    }

    pub fn toggle_selection(&mut self) {
        let (path, size) = match self.current_entry() {
            Some(TreeEntry::TargetDir { path, size, .. }) => (path.clone(), *size),
            _ => return,
        };

        if self.selected.contains(&path) {
            self.selected.remove(&path);
            self.total_selected_size = self.total_selected_size.saturating_sub(size);
        } else {
            self.selected.insert(path);
            self.total_selected_size += size;
        }
    }

    pub fn select_all(&mut self) {
        self.selected.clear();
        self.total_selected_size = 0;
        for entry in &self.tree {
            if let TreeEntry::TargetDir { path, size, .. } = entry {
                self.selected.insert(path.clone());
                self.total_selected_size += size;
            }
        }
    }

    pub fn deselect_all(&mut self) {
        self.selected.clear();
        self.total_selected_size = 0;
    }

    pub fn current_entry(&self) -> Option<&TreeEntry> {
        self.tree.get(self.list_index)
    }

    pub fn move_up(&mut self) {
        if self.list_index > 0 {
            self.list_index -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.list_index + 1 < self.tree.len() {
            self.list_index += 1;
        }
    }

    pub fn selection_count(&self) -> usize {
        self.selected.len()
    }

    pub fn remove_deleted_from_tree(&mut self) {
        let deleted = std::mem::take(&mut self.selected);
        let old_tree = std::mem::take(&mut self.tree);
        let mut new_tree: Vec<TreeEntry> = Vec::new();
        let mut i = 0;

        while i < old_tree.len() {
            match &old_tree[i] {
                TreeEntry::ProjectHeader { .. } => {
                    let header = old_tree[i].clone();
                    i += 1;
                    let mut surviving: Vec<TreeEntry> = Vec::new();

                    while i < old_tree.len() {
                        match &old_tree[i] {
                            TreeEntry::TargetDir { path, .. } => {
                                if !deleted.contains(path) {
                                    surviving.push(old_tree[i].clone());
                                }
                                i += 1;
                            }
                            TreeEntry::ProjectHeader { .. } => break,
                        }
                    }

                    if !surviving.is_empty() {
                        fix_is_last(&mut surviving);
                        new_tree.push(header);
                        new_tree.extend(surviving);
                    }
                }
                TreeEntry::TargetDir { .. } => {
                    i += 1;
                }
            }
        }

        self.tree = new_tree;
        self.total_selected_size = 0;
        self.rebuild_target_index();
        self.clamp_index();
    }

    fn clamp_index(&mut self) {
        if !self.tree.is_empty() && self.list_index >= self.tree.len() {
            self.list_index = self.tree.len() - 1;
        }
    }

    pub fn accumulate_deletion(&mut self, count: usize, size: u64) {
        self.total_deleted_count += count as u64;
        self.total_deleted_size += size;
    }

    pub fn clear_notification(&mut self) {
        self.delete_result_summary = None;
    }

    fn rebuild_target_index(&mut self) {
        self.target_index.clear();
        for (i, entry) in self.tree.iter().enumerate() {
            if let TreeEntry::TargetDir { path, .. } = entry {
                self.target_index.insert(path.clone(), i);
            }
        }
    }

    pub fn order_tree(&mut self, by: OrderBy) {
        if self.tree.is_empty() {
            self.ordered_by = by;
            return;
        }

        let groups = group_entries(&self.tree);
        let mut ordered_groups: Vec<Vec<TreeEntry>> = Vec::new();

        for group in groups {
            let (header, mut children) = group_split(group);

            match by {
                OrderBy::NameAsc | OrderBy::NameDesc => {
                    children.sort_by(|a, b| {
                        let ka = name_order_key(a);
                        let kb = name_order_key(b);
                        if by == OrderBy::NameAsc { ka.cmp(kb) } else { kb.cmp(ka) }
                    });
                }
                OrderBy::DateAsc | OrderBy::DateDesc => {
                    children.sort_by(|a, b| {
                        let ka = date_order_key(a);
                        let kb = date_order_key(b);
                        if by == OrderBy::DateAsc { ka.cmp(&kb) } else { kb.cmp(&ka) }
                    });
                }
                OrderBy::SizeAsc | OrderBy::SizeDesc => {
                    children.sort_by(|a, b| {
                        let ka = size_order_key(a);
                        let kb = size_order_key(b);
                        if by == OrderBy::SizeAsc { ka.cmp(&kb) } else { kb.cmp(&ka) }
                    });
                }
            }

            fix_is_last(&mut children);

            let mut full = vec![header];
            full.extend(children);
            ordered_groups.push(full);
        }

        match by {
            OrderBy::NameAsc | OrderBy::NameDesc => {
                ordered_groups.sort_by(|a, b| {
                    let ka = group_name_key(&a[0]);
                    let kb = group_name_key(&b[0]);
                    if by == OrderBy::NameAsc { ka.cmp(kb) } else { kb.cmp(ka) }
                });
            }
            OrderBy::DateAsc => {
                ordered_groups.sort_by(|a, b| {
                    group_min_date(a).cmp(&group_min_date(b))
                });
            }
            OrderBy::DateDesc => {
                ordered_groups.sort_by(|a, b| {
                    group_max_date(b).cmp(&group_max_date(a))
                });
            }
            OrderBy::SizeAsc => {
                ordered_groups.sort_by(|a, b| {
                    group_min_size(a).cmp(&group_min_size(b))
                });
            }
            OrderBy::SizeDesc => {
                ordered_groups.sort_by(|a, b| {
                    group_max_size(b).cmp(&group_max_size(a))
                });
            }
        }

        self.tree = ordered_groups.into_iter().flatten().collect();
        self.rebuild_target_index();
        self.ordered_by = by;
        self.list_index = 0;
        self.clamp_index();
    }

    pub fn apply_size_update(&mut self, path: &Path, size: u64, last_modified: DateTime<Utc>) {
        if let Some(&i) = self.target_index.get(path)
            && let Some(TreeEntry::TargetDir {
                size: s,
                last_modified: lm,
                ..
            }) = self.tree.get_mut(i)
        {
            let old_size = *s;
            *s = size;
            *lm = last_modified;
            self.total_reclaimable = self
                .total_reclaimable
                .saturating_add(size.saturating_sub(old_size));

            if self.selected.contains(path) {
                self.total_selected_size = self
                    .total_selected_size
                    .saturating_add(size.saturating_sub(old_size));
            }
        }
    }

    pub fn sizes_status(&self) -> Option<String> {
        if self.sizes_total > 0 && !self.sizes_complete {
            Some(format!(
                "Computing sizes... ({}/{})",
                self.sizes_found, self.sizes_total
            ))
        } else {
            None
        }
    }
}

fn group_entries(tree: &[TreeEntry]) -> Vec<Vec<TreeEntry>> {
    let mut groups = Vec::new();
    let mut current: Vec<TreeEntry> = Vec::new();
    for entry in tree {
        match entry {
            TreeEntry::ProjectHeader { .. } => {
                if !current.is_empty() {
                    groups.push(current);
                }
                current = vec![entry.clone()];
            }
            _ => {
                current.push(entry.clone());
            }
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

fn group_split(group: Vec<TreeEntry>) -> (TreeEntry, Vec<TreeEntry>) {
    let mut iter = group.into_iter();
    let header = iter.next().expect("group must have a ProjectHeader");
    let children = iter.collect();
    (header, children)
}

fn fix_is_last(children: &mut [TreeEntry]) {
    if let Some(TreeEntry::TargetDir { is_last, .. }) = children.last_mut() {
        *is_last = true;
    }
    for entry in children.iter_mut().rev().skip(1) {
        if let TreeEntry::TargetDir { is_last, .. } = entry {
            *is_last = false;
        }
    }
}

fn name_order_key(entry: &TreeEntry) -> &std::path::Path {
    match entry {
        TreeEntry::TargetDir { path, .. } => path.as_path(),
        _ => unreachable!(),
    }
}

fn date_order_key(entry: &TreeEntry) -> DateTime<Utc> {
    match entry {
        TreeEntry::TargetDir { last_modified, .. } => *last_modified,
        _ => DateTime::UNIX_EPOCH,
    }
}

fn group_name_key(header: &TreeEntry) -> &str {
    match header {
        TreeEntry::ProjectHeader { name, .. } => name.as_str(),
        _ => "",
    }
}

fn group_min_date(group: &[TreeEntry]) -> DateTime<Utc> {
    group
        .iter()
        .filter_map(|e| {
            if let TreeEntry::TargetDir { last_modified, .. } = e {
                Some(*last_modified)
            } else {
                None
            }
        })
        .min()
        .unwrap_or(DateTime::UNIX_EPOCH)
}

fn group_max_date(group: &[TreeEntry]) -> DateTime<Utc> {
    group
        .iter()
        .filter_map(|e| {
            if let TreeEntry::TargetDir { last_modified, .. } = e {
                Some(*last_modified)
            } else {
                None
            }
        })
        .max()
        .unwrap_or(DateTime::UNIX_EPOCH)
}

fn size_order_key(entry: &TreeEntry) -> u64 {
    match entry {
        TreeEntry::TargetDir { size, .. } => *size,
        _ => 0,
    }
}

fn group_min_size(group: &[TreeEntry]) -> u64 {
    group
        .iter()
        .filter_map(|e| {
            if let TreeEntry::TargetDir { size, .. } = e {
                Some(*size)
            } else {
                None
            }
        })
        .min()
        .unwrap_or(0)
}

fn group_max_size(group: &[TreeEntry]) -> u64 {
    group
        .iter()
        .filter_map(|e| {
            if let TreeEntry::TargetDir { size, .. } = e {
                Some(*size)
            } else {
                None
            }
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::ProjectInfo;
    use chrono::Utc;

    fn make_scan_output() -> ScanOutput {
        let project = ProjectInfo {
            path: PathBuf::from("/test/my-app"),
            name: "my-app".to_string(),
            package_manager: Some("npm".to_string()),
            children: vec![
                PathBuf::from("/test/my-app/node_modules"),
                PathBuf::from("/test/my-app/.next"),
            ],
        };

        let dirs = vec![
            ScannedDir {
                path: PathBuf::from("/test/my-app/node_modules"),
                size: 1_200_000_000,
                last_modified: Utc::now(),
                package_manager: Some("npm".to_string()),
                error: None,
            },
            ScannedDir {
                path: PathBuf::from("/test/my-app/.next"),
                size: 500_000_000,
                last_modified: Utc::now(),
                package_manager: Some("npm".to_string()),
                error: None,
            },
        ];

        ScanOutput {
            target_dirs: dirs,
            projects: vec![project],
            errors: Vec::new(),
        }
    }

    #[test]
    fn test_build_tree_creates_headers() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(make_scan_output());

        assert!(state.tree.len() >= 3);
        assert!(matches!(state.tree[0], TreeEntry::ProjectHeader { .. }));
        assert!(matches!(state.tree[1], TreeEntry::TargetDir { .. }));
        assert!(matches!(state.tree[2], TreeEntry::TargetDir { .. }));
    }

    #[test]
    fn test_empty_project_is_skipped() {
        let project = ProjectInfo {
            path: PathBuf::from("/test/empty-app"),
            name: "empty-app".to_string(),
            package_manager: Some("npm".to_string()),
            children: vec![],
        };

        let output = ScanOutput {
            target_dirs: vec![],
            projects: vec![project],
            errors: Vec::new(),
        };

        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(output);
        assert_eq!(state.tree.len(), 0, "project with no children should be skipped");
    }

    #[test]
    fn test_toggle_selection() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(make_scan_output());
        state.list_index = 1;

        assert_eq!(state.selection_count(), 0);
        state.toggle_selection();
        assert_eq!(state.selection_count(), 1);
        state.toggle_selection();
        assert_eq!(state.selection_count(), 0);
    }

    #[test]
    fn test_select_all() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(make_scan_output());
        state.select_all();
        assert_eq!(state.selection_count(), 2);
    }

    #[test]
    fn test_deselect_all() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(make_scan_output());
        state.select_all();
        state.deselect_all();
        assert_eq!(state.selection_count(), 0);
    }

    #[test]
    fn test_move_up_down() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(make_scan_output());

        state.move_down();
        assert_eq!(state.list_index, 1);
        state.move_down();
        assert_eq!(state.list_index, 2);
        state.move_up();
        assert_eq!(state.list_index, 1);
    }

    #[test]
    fn test_clamp_index() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(make_scan_output());
        assert!(state.list_index < state.tree.len());
    }

    #[test]
    fn test_cannot_move_past_end() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(make_scan_output());
        let n = state.tree.len();
        for _ in 0..n + 5 {
            state.move_down();
        }
        assert_eq!(state.list_index, state.tree.len() - 1);
    }

    #[test]
    fn test_remove_deleted_removes_target_dirs() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(make_scan_output());
        state.select_all();

        assert_eq!(state.selection_count(), 2);
        state.remove_deleted_from_tree();

        assert_eq!(state.selection_count(), 0);
        assert_eq!(state.tree.len(), 0);
    }

    #[test]
    fn test_remove_deleted_keeps_remaining_children() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(make_scan_output());

        // Select only the first target dir
        state.list_index = 1;
        state.toggle_selection();
        assert_eq!(state.selection_count(), 1);

        state.remove_deleted_from_tree();

        // Project header + 1 remaining target dir
        assert_eq!(state.tree.len(), 2);
        assert!(matches!(state.tree[0], TreeEntry::ProjectHeader { .. }));
        assert!(matches!(state.tree[1], TreeEntry::TargetDir { .. }));
    }

    #[test]
    fn test_total_deleted_starts_at_zero() {
        let state = AppState::new(PathBuf::from("/test"));
        assert_eq!(state.total_deleted_count, 0);
        assert_eq!(state.total_deleted_size, 0);
    }

    #[test]
    fn test_accumulate_deletion() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.accumulate_deletion(3, 1_500_000_000);
        assert_eq!(state.total_deleted_count, 3);
        assert_eq!(state.total_deleted_size, 1_500_000_000);

        state.accumulate_deletion(2, 500_000_000);
        assert_eq!(state.total_deleted_count, 5);
        assert_eq!(state.total_deleted_size, 2_000_000_000);
    }

    #[test]
    fn test_cannot_move_past_start() {
        let mut state = AppState::new(PathBuf::from("/test"));
        state.build_tree(make_scan_output());
        for _ in 0..10 {
            state.move_up();
        }
        assert_eq!(state.list_index, 0);
    }
}
