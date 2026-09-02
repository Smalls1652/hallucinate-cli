//! Shared path expansion and working-directory resolution for filesystem tools.

use std::path::{Path, PathBuf};

/// Expand a tool-supplied path and resolve relative values against `cwd`.
///
/// Tilde and environment-variable expansion follow `shellexpand` semantics.
/// If expansion fails, the literal path is retained so each tool can report
/// its normal filesystem error. Relative paths use `cwd`, or the process
/// working directory when no tool working directory is supplied.
pub(super) fn resolve_path(
    path_str: &str,
    cwd: Option<&str>,
) -> PathBuf {
    let expanded = shellexpand::full(path_str).unwrap_or(std::borrow::Cow::Borrowed(path_str));
    let path = Path::new(expanded.as_ref());

    if path.is_absolute() {
        return path.to_path_buf();
    }

    let cwd = cwd
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    cwd.join(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_absolute_paths_and_resolves_relative_paths() {
        assert_eq!(
            resolve_path("/absolute/path.txt", Some("/workspace")),
            PathBuf::from("/absolute/path.txt")
        );
        assert_eq!(
            resolve_path("relative/path.txt", Some("/workspace")),
            PathBuf::from("/workspace/relative/path.txt")
        );
    }
}
