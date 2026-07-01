use std::path::Path;

pub struct Language {
    pub name: &'static str,
    pub lock_files: &'static [&'static str],
    pub config_files: &'static [&'static str],
    /// Target dirs checked only at the project root (no recursion).
    pub root_target_dirs: &'static [&'static str],
    /// Target dirs checked recursively within the project subtree.
    pub deep_target_dirs: &'static [&'static str],
}

pub const LANGUAGES: &[Language] = &[
    Language {
        name: "rust",
        lock_files: &["Cargo.lock"],
        config_files: &["Cargo.toml"],
        root_target_dirs: &["target"],
        deep_target_dirs: &[],
    },
    Language {
        name: "js",
        lock_files: &[
            "package-lock.json",
            "pnpm-lock.yaml",
            "bun.lock",
            "deno.lock",
            "yarn.lock",
        ],
        config_files: &[],
        root_target_dirs: &["node_modules", ".next", ".nuxt"],
        deep_target_dirs: &[],
    },
    Language {
        name: "python",
        lock_files: &["uv.lock"],
        config_files: &["pyproject.toml", "requirements.txt", "Pipfile"],
        root_target_dirs: &[],
        deep_target_dirs: &["__pycache__", ".venv", "venv", ".tox"],
    },
    Language {
        name: "go",
        lock_files: &[],
        config_files: &["go.mod", "go.sum"],
        root_target_dirs: &["vendor"],
        deep_target_dirs: &[],
    },
    Language {
        name: "ruby",
        lock_files: &["Gemfile.lock"],
        config_files: &["Gemfile"],
        root_target_dirs: &["vendor"],
        deep_target_dirs: &[],
    },
    Language {
        name: "php",
        lock_files: &["composer.lock"],
        config_files: &["composer.json"],
        root_target_dirs: &["vendor"],
        deep_target_dirs: &[],
    },
    Language {
        name: "cocoapods",
        lock_files: &["Podfile.lock"],
        config_files: &["Podfile"],
        root_target_dirs: &["Pods"],
        deep_target_dirs: &[],
    },
];

pub fn is_detection_file(file_name: &str) -> Option<&'static str> {
    for lang in LANGUAGES {
        for f in lang.lock_files {
            if *f == file_name {
                return Some(lang.name);
            }
        }
        for f in lang.config_files {
            if *f == file_name {
                return Some(lang.name);
            }
        }
    }
    None
}

pub fn is_any_target(dir_name: &str) -> bool {
    LANGUAGES.iter().any(|lang| {
        lang.root_target_dirs.contains(&dir_name) || lang.deep_target_dirs.contains(&dir_name)
    })
}

pub fn root_target_dirs_for_languages(languages: &[&str]) -> Vec<&'static str> {
    let mut dirs: Vec<&str> = Vec::new();
    for lang in LANGUAGES {
        if languages.contains(&lang.name) {
            for d in lang.root_target_dirs {
                if !dirs.contains(d) {
                    dirs.push(d);
                }
            }
        }
    }
    dirs
}

pub fn deep_target_dirs_for_languages(languages: &[&str]) -> Vec<&'static str> {
    let mut dirs: Vec<&str> = Vec::new();
    for lang in LANGUAGES {
        if languages.contains(&lang.name) {
            for d in lang.deep_target_dirs {
                if !dirs.contains(d) {
                    dirs.push(d);
                }
            }
        }
    }
    dirs
}

pub fn detect_languages(dir: &Path) -> Vec<&'static str> {
    let mut languages: Vec<&str> = Vec::new();
    let read_dir = match dir.read_dir() {
        Ok(rd) => rd,
        Err(_) => return languages,
    };

    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.file_type().is_ok_and(|ft| ft.is_file())
            && let Some(lang) = is_detection_file(&name)
            && !languages.contains(&lang)
        {
            languages.push(lang);
        }
    }

    languages.sort();
    languages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_languages_have_names() {
        for lang in LANGUAGES {
            assert!(!lang.name.is_empty());
        }
    }

    #[test]
    fn test_is_detection_file_works() {
        assert_eq!(is_detection_file("Cargo.lock"), Some("rust"));
        assert_eq!(is_detection_file("package-lock.json"), Some("js"));
        assert_eq!(is_detection_file("pyproject.toml"), Some("python"));
        assert_eq!(is_detection_file("go.mod"), Some("go"));
        assert_eq!(is_detection_file("Gemfile.lock"), Some("ruby"));
        assert_eq!(is_detection_file("composer.json"), Some("php"));
        assert_eq!(is_detection_file("Podfile.lock"), Some("cocoapods"));
    }

    #[test]
    fn test_is_detection_file_unknown() {
        assert!(is_detection_file("unknown.lock").is_none());
        assert!(is_detection_file("random.txt").is_none());
        assert!(is_detection_file(".gitignore").is_none());
    }

    #[test]
    fn test_is_any_target() {
        assert!(is_any_target("node_modules"));
        assert!(is_any_target("target"));
        assert!(is_any_target("__pycache__"));
        assert!(!is_any_target("random_folder"));
        assert!(!is_any_target(".git"));
        assert!(!is_any_target("src"));
    }

    #[test]
    fn test_root_target_dirs_for_languages() {
        let dirs = root_target_dirs_for_languages(&["rust"]);
        assert_eq!(dirs, vec!["target"]);

        let dirs = root_target_dirs_for_languages(&["js"]);
        assert!(dirs.contains(&"node_modules"));
        assert!(dirs.contains(&".next"));

        let dirs = root_target_dirs_for_languages(&["python"]);
        assert!(dirs.is_empty(), "python has no root-only targets");
    }

    #[test]
    fn test_deep_target_dirs_for_languages() {
        let dirs = deep_target_dirs_for_languages(&["python"]);
        assert!(dirs.contains(&"__pycache__"));

        let dirs = deep_target_dirs_for_languages(&["go"]);
        assert!(dirs.is_empty(), "go has no deep targets");
    }

    #[test]
    fn test_target_dirs_dedup_shared_targets() {
        // go, ruby, php all have "vendor" in root_target_dirs — should appear once
        let dirs = root_target_dirs_for_languages(&["go", "ruby", "php"]);
        assert_eq!(dirs.iter().filter(|&&d| d == "vendor").count(), 1);
    }

    #[test]
    fn test_detect_languages_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let langs = detect_languages(dir.path());
        assert!(langs.is_empty());
    }

    #[test]
    fn test_detect_languages_with_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.path().join("package.json"), "").unwrap();
        let langs = detect_languages(dir.path());
        assert_eq!(langs, vec!["rust"]);
    }

    #[test]
    fn test_detect_languages_multiple() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.path().join("package-lock.json"), "").unwrap();
        let langs = detect_languages(dir.path());
        assert_eq!(langs, vec!["js", "rust"]);
    }
}
