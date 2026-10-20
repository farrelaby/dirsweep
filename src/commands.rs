use sha2::Digest;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::Command;
const GITHUB_REPO: &str = "farrelaby/dirsweep";
const MAX_RETRIES: u32 = 3;

pub fn detect_target() -> Result<&'static str, &'static str> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        Ok("x86_64-unknown-linux-gnu")
    }

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        Ok("aarch64-unknown-linux-gnu")
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Ok("aarch64-apple-darwin")
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Ok("x86_64-pc-windows-msvc")
    }

    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    {
        Err("Unsupported operating system or architecture")
    }
}

pub fn parse_version(tag: &str) -> Option<String> {
    tag.strip_prefix('v').map(|s| s.to_string())
}

pub fn version_is_newer(current: &str, new: &str) -> bool {
    let current_parts: Vec<u32> = current.split('.').filter_map(|s| s.parse().ok()).collect();
    let new_parts: Vec<u32> = new.split('.').filter_map(|s| s.parse().ok()).collect();

    for (a, b) in current_parts.iter().zip(new_parts.iter()) {
        if a < b {
            return true;
        }
        if a > b {
            return false;
        }
    }
    new_parts.len() > current_parts.len()
}

pub fn parse_checksums(content: &str) -> HashMap<String, String> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (hash, filename) = if let Some((h, f)) = line.split_once("  ") {
                (h.trim(), f.trim())
            } else if let Some((h, f)) = line.split_once(" *") {
                (h.trim(), f.trim())
            } else {
                return None;
            };
            let filename = filename.strip_prefix('*').unwrap_or(filename);
            if hash.is_empty() || filename.is_empty() {
                None
            } else {
                Some((filename.to_string(), hash.to_string()))
            }
        })
        .collect()
}

pub fn find_asset_url(assets: &[serde_json::Value], target: &str) -> Option<String> {
    for asset in assets {
        if let Some(name) = asset["name"].as_str()
            && name.contains(target)
            && !name.ends_with(".txt")
        {
            return asset["browser_download_url"]
                .as_str()
                .map(|s| s.to_string());
        }
    }
    None
}

pub fn find_checksums_url(assets: &[serde_json::Value]) -> Option<String> {
    for asset in assets {
        if let Some(name) = asset["name"].as_str()
            && name == "checksums.txt"
        {
            return asset["browser_download_url"]
                .as_str()
                .map(|s| s.to_string());
        }
    }
    None
}

fn download_file(url: &str, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let response = ureq::get(url).set("User-Agent", "dirsweep").call()?;

    let mut file = std::fs::File::create(dest)?;
    let mut reader = response.into_reader();
    io::copy(&mut reader, &mut file)?;
    Ok(())
}

fn verify_checksum(file_path: &Path, expected_hash: &str) -> bool {
    let mut file = match std::fs::File::open(file_path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(_) => return false,
        }
    }

    let actual_hash = format!("{:x}", hasher.finalize());
    actual_hash.eq_ignore_ascii_case(expected_hash)
}

fn extract_archive(
    archive_path: &Path,
    dest_dir: &Path,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    if cfg!(target_os = "windows") {
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                    archive_path.display(),
                    dest_dir.display()
                ),
            ])
            .output()?;
    } else {
        Command::new("tar")
            .args([
                "-xzf",
                &archive_path.to_string_lossy(),
                "-C",
                &dest_dir.to_string_lossy(),
            ])
            .output()?;
    }

    let binary_name = if cfg!(target_os = "windows") {
        "dirsweep.exe"
    } else {
        "dirsweep"
    };
    let binary_path = dest_dir.join(binary_name);

    if binary_path.exists() {
        Ok(binary_path)
    } else {
        Err("Extracted binary not found".into())
    }
}

fn prompt_confirm(message: &str, force: bool) -> bool {
    if force {
        return true;
    }

    print!("{} [y/N] ", message);
    io::stdout().flush().ok();

    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();

    input.trim().to_lowercase() == "y"
}

pub fn cmd_update() -> Result<(), Box<dyn std::error::Error>> {
    let current_version = env!("CARGO_PKG_VERSION");
    let target = detect_target()?;

    println!("Checking for updates...");

    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );
    let response: serde_json::Value = ureq::get(&url)
        .set("User-Agent", "dirsweep")
        .call()?
        .into_json()?;

    let tag = response["tag_name"]
        .as_str()
        .ok_or("Failed to parse release tag")?;

    let latest_version = parse_version(tag).ok_or("Failed to parse version from tag")?;

    if !version_is_newer(current_version, &latest_version) {
        println!("dirsweep is already up to date (v{})", current_version);
        return Ok(());
    }

    println!("Updating to v{}...", latest_version);

    let assets = response["assets"]
        .as_array()
        .ok_or("No assets found in release")?;

    let asset_url = find_asset_url(assets, target).ok_or("No download found for your platform")?;

    let checksums_url = find_checksums_url(assets).ok_or("No checksums found in release")?;

    let temp_dir = tempfile::tempdir()?;

    let mut success = false;
    for attempt in 1..=MAX_RETRIES {
        let checksums_path = temp_dir.path().join("checksums.txt");
        if download_file(&checksums_url, &checksums_path).is_err() {
            if attempt == MAX_RETRIES {
                println!("Update failed. Please try again later.");
                return Err("Download failed".into());
            }
            println!("Download failed. Retrying... ({}/{})", attempt, MAX_RETRIES);
            continue;
        }

        let archive_name = asset_url.split('/').next_back().unwrap_or("archive");
        let archive_path = temp_dir.path().join(archive_name);
        if download_file(&asset_url, &archive_path).is_err() {
            if attempt == MAX_RETRIES {
                println!("Update failed. Please try again later.");
                return Err("Download failed".into());
            }
            println!("Download failed. Retrying... ({}/{})", attempt, MAX_RETRIES);
            continue;
        }

        let checksums_content = std::fs::read_to_string(&checksums_path)?;
        let checksums = parse_checksums(&checksums_content);

        let expected_hash = match checksums.get(archive_name) {
            Some(hash) => hash,
            None => {
                println!("Update failed. Archive checksum missing.");
                return Err("Archive checksum missing from checksums.txt".into());
            }
        };

        if !verify_checksum(&archive_path, expected_hash) {
            if attempt == MAX_RETRIES {
                println!("Update failed. Please try again later.");
                return Err("Checksum mismatch".into());
            }
            println!(
                "Download corrupted. Retrying... ({}/{})",
                attempt, MAX_RETRIES
            );
            continue;
        }

        let extract_dir = temp_dir.path().join("extract");
        std::fs::create_dir_all(&extract_dir)?;

        let new_binary = extract_archive(&archive_path, &extract_dir)?;

        self_replace::self_replace(&new_binary)?;

        std::fs::remove_file(&new_binary).ok();

        println!("Updated to v{}", latest_version);
        success = true;
        break;
    }

    if !success {
        println!("Update failed. Please try again later.");
        return Err("Update failed after retries".into());
    }

    Ok(())
}

pub fn cmd_uninstall(force: bool) -> Result<(), Box<dyn std::error::Error>> {
    let exe_path = std::env::current_exe()?;

    if !prompt_confirm("This will remove dirsweep from your system.", force) {
        println!("Uninstall cancelled.");
        return Ok(());
    }

    println!("Uninstalling from {}...", exe_path.display());

    self_replace::self_delete()?;

    println!("dirsweep has been uninstalled.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_version_with_prefix() {
        assert_eq!(parse_version("v0.1.3"), Some("0.1.3".to_string()));
    }

    #[test]
    fn test_parse_version_without_prefix() {
        assert_eq!(parse_version("0.1.3"), None);
    }

    #[test]
    fn test_parse_version_just_prefix() {
        assert_eq!(parse_version("v"), Some("".to_string()));
    }

    #[test]
    fn test_parse_version_extra_segments() {
        assert_eq!(parse_version("v1.2.3.4"), Some("1.2.3.4".to_string()));
    }

    #[test]
    fn test_version_is_newer_patch_bump() {
        assert!(version_is_newer("0.1.2", "0.1.3"));
    }

    #[test]
    fn test_version_is_newer_older() {
        assert!(!version_is_newer("0.1.3", "0.1.2"));
    }

    #[test]
    fn test_version_is_newer_same() {
        assert!(!version_is_newer("0.1.3", "0.1.3"));
    }

    #[test]
    fn test_version_is_newer_minor_bump() {
        assert!(version_is_newer("0.1.2", "0.2.0"));
    }

    #[test]
    fn test_version_is_newer_major_bump() {
        assert!(version_is_newer("0.9.9", "1.0.0"));
    }

    #[test]
    fn test_parse_checksums_single_line() {
        let content = "abc123  dirsweep-v0.1-linux.tar.gz\n";
        let result = parse_checksums(content);
        assert_eq!(result.len(), 1);
        assert_eq!(result.get("dirsweep-v0.1-linux.tar.gz").unwrap(), "abc123");
    }

    #[test]
    fn test_parse_checksums_multiple_lines() {
        let content = "abc123  dirsweep-v0.1-linux.tar.gz\ndef456  dirsweep-v0.1-macos.tar.gz\n";
        let result = parse_checksums(content);
        assert_eq!(result.len(), 2);
        assert_eq!(result.get("dirsweep-v0.1-linux.tar.gz").unwrap(), "abc123");
        assert_eq!(result.get("dirsweep-v0.1-macos.tar.gz").unwrap(), "def456");
    }

    #[test]
    fn test_parse_checksums_space_asterisk() {
        let content = "abc123  *dirsweep-v0.1-linux.tar.gz\ndef456 *dirsweep-v0.1-macos.tar.gz\r\n";
        let result = parse_checksums(content);
        assert_eq!(result.len(), 2);
        assert_eq!(result.get("dirsweep-v0.1-linux.tar.gz").unwrap(), "abc123");
        assert_eq!(result.get("dirsweep-v0.1-macos.tar.gz").unwrap(), "def456");
    }

    #[test]
    fn test_parse_checksums_empty() {
        let content = "";
        let result = parse_checksums(content);
        assert!(result.is_empty());
    }

    #[test]
    fn test_verify_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello world\n").unwrap();
        // sha256 of "hello world\n" is d9014c4624844aa5bac314773d6b689ad467fa4e1d1a50a1b8a99d5a95f72ff5
        let expected_hash = "a948904f2f0f479b8f8197694b30184b0d2ed1c1cd2a1ec0fb85d299a192a447";
        assert!(verify_checksum(&file_path, expected_hash));
        assert!(verify_checksum(&file_path, &expected_hash.to_uppercase()));
        assert!(!verify_checksum(&file_path, "invalidhash"));
    }

    #[test]
    fn test_find_asset_url_matching() {
        let assets = vec![
            json!({
                "name": "dirsweep-v0.1.3-x86_64-unknown-linux-gnu.tar.gz",
                "browser_download_url": "https://example.com/linux.tar.gz"
            }),
            json!({
                "name": "checksums.txt",
                "browser_download_url": "https://example.com/checksums.txt"
            }),
        ];
        let result = find_asset_url(&assets, "x86_64-unknown-linux-gnu");
        assert_eq!(result, Some("https://example.com/linux.tar.gz".to_string()));
    }

    #[test]
    fn test_find_asset_url_no_match() {
        let assets = vec![json!({
            "name": "dirsweep-v0.1.3-aarch64-apple-darwin.tar.gz",
            "browser_download_url": "https://example.com/macos.tar.gz"
        })];
        let result = find_asset_url(&assets, "x86_64-unknown-linux-gnu");
        assert_eq!(result, None);
    }

    #[test]
    fn test_find_asset_url_skips_txt() {
        let assets = vec![json!({
            "name": "checksums.txt",
            "browser_download_url": "https://example.com/checksums.txt"
        })];
        let result = find_asset_url(&assets, "checksums");
        assert_eq!(result, None);
    }

    #[test]
    fn test_find_checksums_url_present() {
        let assets = vec![
            json!({
                "name": "dirsweep-v0.1.3-linux.tar.gz",
                "browser_download_url": "https://example.com/linux.tar.gz"
            }),
            json!({
                "name": "checksums.txt",
                "browser_download_url": "https://example.com/checksums.txt"
            }),
        ];
        let result = find_checksums_url(&assets);
        assert_eq!(
            result,
            Some("https://example.com/checksums.txt".to_string())
        );
    }

    #[test]
    fn test_find_checksums_url_not_present() {
        let assets = vec![json!({
            "name": "dirsweep-v0.1.3-linux.tar.gz",
            "browser_download_url": "https://example.com/linux.tar.gz"
        })];
        let result = find_checksums_url(&assets);
        assert_eq!(result, None);
    }

    #[test]
    fn test_detect_target_returns_non_empty() {
        let target = detect_target().unwrap();
        assert!(!target.is_empty());
    }

    #[test]
    fn test_prompt_confirm_force_true() {
        assert!(prompt_confirm("test message", true));
    }
}
