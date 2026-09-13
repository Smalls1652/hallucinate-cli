//! Deterministic, bounded filesystem traversal for search tools.
//!
//! The walker sorts each directory lexically and visits depth-first. It skips
//! known generated directories and never follows directory symlinks.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::limits::ToolLimits;

const SKIPPED_DIRECTORIES: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "build",
    "dist",
    ".cache",
    ".next",
    ".turbo",
];

/// Signal whether a walk callback wants traversal to continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkControl {
    /// Continue with the next candidate file.
    Continue,

    /// Stop traversal because the caller's result budget is full.
    Stop,
}

/// Outcome details for a completed bounded traversal.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WalkStatus {
    /// Whether a resource limit or callback stopped traversal early.
    pub truncated: bool,
}

/// Walk matching regular files in deterministic lexical depth-first order.
///
/// `pattern` matches a path relative to `root`; patterns without a path
/// separator also match a file name, preserving the familiar `*.rs` behavior.
pub fn walk_matching_files<F>(
    root: &Path,
    pattern: Option<&glob::Pattern>,
    limits: &ToolLimits,
    mut visit: F,
) -> Result<WalkStatus, anyhow::Error>
where
    F: FnMut(PathBuf) -> WalkControl,
{
    let mut candidate_files = 0_usize;

    walk_entries(root, true, limits, |path, file_type| {
        if !file_type.is_file() || !matches_pattern(root, &path, pattern) {
            return WalkControl::Continue;
        }

        if candidate_files >= limits.max_candidate_files {
            return WalkControl::Stop;
        }

        candidate_files += 1;

        visit(path)
    })
}

enum Pending {
    Directory(PathBuf),
    Entry(PathBuf, fs::FileType),
}

/// Walk directory entries in deterministic lexical depth-first order.
///
/// Generated directories are included as entries but not descended into, and
/// directory symlinks are never followed. Set `recursive` to `false` to visit
/// only immediate children of `root`.
pub fn walk_entries<F>(
    root: &Path,
    recursive: bool,
    limits: &ToolLimits,
    mut visit: F,
) -> Result<WalkStatus, anyhow::Error>
where
    F: FnMut(PathBuf, fs::FileType) -> WalkControl,
{
    if root.is_file() {
        let file_type = fs::symlink_metadata(root)?.file_type();
        return Ok(WalkStatus {
            truncated: visit(root.to_path_buf(), file_type) == WalkControl::Stop,
        });
    }

    if !root.is_dir() {
        return Ok(WalkStatus::default());
    }

    let mut status = WalkStatus::default();
    let mut directory_entries = 0_usize;
    let mut pending = vec![Pending::Directory(root.to_path_buf())];

    while let Some(next) = pending.pop() {
        match next {
            Pending::Entry(path, file_type) => {
                if visit(path.clone(), file_type) == WalkControl::Stop {
                    status.truncated = true;
                    return Ok(status);
                }

                if recursive && file_type.is_dir() && !is_skipped_directory(&path) {
                    pending.push(Pending::Directory(path));
                }
            },

            Pending::Directory(directory) => {
                let remaining_entries = limits
                    .max_directory_entries
                    .saturating_sub(directory_entries);

                if remaining_entries == 0 {
                    status.truncated = true;
                    break;
                }

                let read_dir = match fs::read_dir(&directory) {
                    Ok(read_dir) => read_dir,
                    Err(_) => continue,
                };

                let mut children = Vec::new();

                for entry in read_dir.take(remaining_entries + 1).flatten() {
                    if children.len() == remaining_entries {
                        status.truncated = true;
                        break;
                    }
                    children.push(entry);
                }

                directory_entries += children.len();
                children.sort_by_key(|entry| entry.file_name());

                for entry in children.into_iter().rev() {
                    let path = entry.path();

                    if let Ok(file_type) = entry.file_type() {
                        pending.push(Pending::Entry(path, file_type));
                    }
                }

                if status.truncated {
                    break;
                }
            },
        }
    }

    Ok(status)
}

fn is_skipped_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| SKIPPED_DIRECTORIES.contains(&name))
}

fn matches_pattern(
    root: &Path,
    path: &Path,
    pattern: Option<&glob::Pattern>,
) -> bool {
    let Some(pattern) = pattern else {
        return true;
    };

    let relative_path = path.strip_prefix(root).unwrap_or(path);

    pattern.matches_path(relative_path)
        || (!pattern.as_str().contains('/')
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| pattern.matches(name)))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn skips_generated_directories_and_reports_result_cutoffs() {
        let root =
            std::env::temp_dir().join(format!("hallucinate-walker-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("target")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("target/ignored.rs"), "ignored").unwrap();
        fs::write(root.join("src/a.rs"), "a").unwrap();
        fs::write(root.join("src/b.rs"), "b").unwrap();

        let pattern = glob::Pattern::new("*.rs").unwrap();
        let mut matches = Vec::new();
        let status = walk_matching_files(&root, Some(&pattern), &ToolLimits::default(), |path| {
            matches.push(path);
            if matches.len() == 1 {
                WalkControl::Stop
            } else {
                WalkControl::Continue
            }
        })
        .unwrap();

        assert!(status.truncated);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].ends_with("src/a.rs"));
        fs::remove_dir_all(root).unwrap();
    }
}
