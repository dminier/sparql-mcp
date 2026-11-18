//! Confining file reads and writes to `[ingest] file_roots`
//! (`docs/designs/sparql-mcp-write-path-hardening.md`, *File access*).
//!
//! The check resolves the path (following symlinks, junctions and `..`) and
//! compares the **resolved** path with the resolved roots, component by
//! component. The resolved path is what the caller then opens, so a link
//! swapped after the check can only race the final open, not the check
//! itself. The residual window is documented in the design.
//!
//! On Windows, some paths are refused before anything is touched, because
//! resolving them could itself reach out to the network or a device:
//!
//! *   UNC paths (`\\host\share\…`), which can leak NTLM credentials;
//! *   device paths (`\\.\…`) and verbatim paths that are not a drive
//!     (`\\?\UNC\…`, `\\?\GLOBALROOT\…`);
//! *   alternate data streams (`file.nt:stream`).
//!
//! A verbatim drive path (`\\?\C:\data\x.nt`) is allowed: it is how Windows
//! reports canonical paths.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::application::limits::ToolError;

/// Resolve the configured roots. A root that does not exist is an error, so a
/// typo fails at startup instead of silently allowing nothing.
pub fn resolve_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    roots
        .iter()
        .map(|root| {
            reject_suspicious(root).map_err(|e| anyhow::anyhow!("[ingest] file_roots: {e}"))?;
            std::fs::canonicalize(root)
                .with_context(|| format!("[ingest] file_roots: {} does not exist", root.display()))
        })
        .collect()
}

/// Check `path` for **reading** and return the resolved path to open.
pub fn check_read(path: &Path, roots: &[PathBuf]) -> Result<PathBuf> {
    reject_suspicious(path).map_err(ToolError::into_anyhow)?;
    let resolved = std::fs::canonicalize(path)
        .with_context(|| format!("cannot resolve {}", path.display()))?;
    confine(path, resolved, roots)
}

/// Check `path` for **writing** (it may not exist yet) and return the path to
/// create. The deepest existing ancestor is resolved; the part that does not
/// exist yet may not contain `..`.
pub fn check_write(path: &Path, roots: &[PathBuf]) -> Result<PathBuf> {
    reject_suspicious(path).map_err(ToolError::into_anyhow)?;
    let mut existing = path;
    let mut missing: Vec<&std::ffi::OsStr> = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                if name == ".." {
                    return Err(ToolError::PathRejected {
                        path: path.display().to_string(),
                    }
                    .into_anyhow());
                }
                missing.push(name);
                existing = parent;
            }
            _ => break,
        }
    }
    // A relative path whose first component is missing resolves against the
    // current directory.
    let base = if existing.as_os_str().is_empty() {
        Path::new(".")
    } else {
        existing
    };
    let mut resolved = std::fs::canonicalize(base)
        .with_context(|| format!("cannot resolve {}", base.display()))?;
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    confine(path, resolved, roots)
}

fn confine(original: &Path, resolved: PathBuf, roots: &[PathBuf]) -> Result<PathBuf> {
    if roots.iter().any(|root| resolved.starts_with(root)) {
        Ok(resolved)
    } else {
        Err(ToolError::PathOutsideRoots {
            path: original.display().to_string(),
        }
        .into_anyhow())
    }
}

/// Refuse the Windows paths that must not even be resolved. A no-op on other
/// platforms, where `\` and `:` are ordinary file name characters.
#[cfg(windows)]
fn reject_suspicious(path: &Path) -> std::result::Result<(), ToolError> {
    let text = path.to_string_lossy().replace('/', "\\");
    let rejected = || {
        Err(ToolError::PathRejected {
            path: path.display().to_string(),
        })
    };

    let rest = if let Some(after) = text.strip_prefix(r"\\?\") {
        // Verbatim: only `\\?\X:\…` (a drive) is allowed.
        let mut chars = after.chars();
        match (chars.next(), chars.next()) {
            (Some(d), Some(':')) if d.is_ascii_alphabetic() => &after[2..],
            _ => return rejected(),
        }
    } else if text.starts_with(r"\\") {
        // `\\.\device`, `\\host\share`.
        return rejected();
    } else {
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(d), Some(':')) if d.is_ascii_alphabetic() => &text[2..],
            _ => text.as_str(),
        }
    };
    // Any further `:` is an alternate data stream.
    if rest.contains(':') {
        return rejected();
    }
    Ok(())
}

#[cfg(not(windows))]
fn reject_suspicious(_path: &Path) -> std::result::Result<(), ToolError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn code(r: Result<PathBuf>) -> String {
        match r {
            Ok(_) => "ok".into(),
            Err(e) => e
                .downcast_ref::<ToolError>()
                .map_or_else(|| format!("error: {e}"), |t| t.code().to_string()),
        }
    }

    fn sandbox() -> (tempfile::TempDir, PathBuf, Vec<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(root.join("sub").join("in.nt"), "x").unwrap();
        fs::write(outside.join("out.nt"), "x").unwrap();
        let roots = resolve_roots(std::slice::from_ref(&root)).unwrap();
        (dir, root, roots)
    }

    #[test]
    fn a_file_inside_a_root_is_allowed_and_resolved() {
        let (_d, root, canon) = sandbox();
        let got = check_read(&root.join("sub").join("in.nt"), &canon).unwrap();
        assert!(got.starts_with(&canon[0]));
    }

    #[test]
    fn a_file_outside_every_root_is_refused() {
        let (dir, root, canon) = sandbox();
        let outside = dir.path().join("outside").join("out.nt");
        assert_eq!(code(check_read(&outside, &canon)), "path_outside_roots");
        // ..-escape from inside the root.
        let sneaky = root
            .join("sub")
            .join("..")
            .join("..")
            .join("outside")
            .join("out.nt");
        assert_eq!(code(check_read(&sneaky, &canon)), "path_outside_roots");
    }

    #[test]
    fn a_sibling_directory_sharing_the_prefix_is_not_inside() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        let sibling = dir.path().join("data-backup");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        fs::write(sibling.join("x.nt"), "x").unwrap();
        let canon = resolve_roots(&[root]).unwrap();
        assert_eq!(
            code(check_read(&sibling.join("x.nt"), &canon)),
            "path_outside_roots"
        );
    }

    #[test]
    fn a_missing_root_is_a_startup_error() {
        let err = resolve_roots(&[PathBuf::from("definitely/not/here")]).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn writes_may_create_new_files_inside_a_root_but_not_escape_it() {
        let (dir, root, canon) = sandbox();
        let ok = check_write(&root.join("sub").join("new").join("out.ttl"), &canon);
        assert!(ok.is_ok(), "{ok:?}");
        assert_eq!(
            code(check_write(
                &dir.path().join("outside").join("new.ttl"),
                &canon
            )),
            "path_outside_roots"
        );
        let sneaky = root
            .join("sub")
            .join("missing")
            .join("..")
            .join("..")
            .join("..")
            .join("x.ttl");
        // Refused either way: Windows folds `missing\..` away before this code
        // sees it (so the path resolves outside the root), other platforms
        // reach the explicit `..` rejection.
        let verdict = code(check_write(&sneaky, &canon));
        assert!(
            verdict == "path_outside_roots" || verdict == "path_rejected",
            "an escaping write was not refused: {verdict}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_that_must_not_be_resolved_are_refused() {
        let (_d, _root, canon) = sandbox();
        for path in [
            r"\\server\share\data.nt",
            "//server/share/data.nt",
            r"\\.\pipe\x",
            r"\\.\C:\x.nt",
            r"\\?\UNC\server\share\x.nt",
            r"\\?\GLOBALROOT\Device\x",
            r"C:\data\x.nt:stream",
            r"C:\data\x.nt::$DATA",
        ] {
            assert_eq!(
                code(check_read(Path::new(path), &canon)),
                "path_rejected",
                "{path}"
            );
            assert_eq!(
                code(check_write(Path::new(path), &canon)),
                "path_rejected",
                "{path}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_verbatim_drive_path_and_a_different_case_are_still_inside() {
        let (_d, root, canon) = sandbox();
        let file = root.join("sub").join("in.nt");
        // `\\?\C:\…` is how Windows reports canonical paths.
        let verbatim = PathBuf::from(format!(r"\\?\{}", file.display()));
        assert!(check_read(&verbatim, &canon).is_ok(), "{verbatim:?}");
        // The file system is case-insensitive: the resolved path has the
        // case stored on disk, so a differently-cased request is still inside.
        let upper = PathBuf::from(file.display().to_string().to_uppercase());
        assert!(check_read(&upper, &canon).is_ok(), "{upper:?}");
    }

    #[cfg(windows)]
    #[test]
    fn a_directory_junction_cannot_escape_the_root() {
        // A junction needs no elevation, unlike a symlink.
        let (dir, root, canon) = sandbox();
        let link = root.join("link");
        let target = dir.path().join("outside");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .expect("cmd runs");
        assert!(status.status.success(), "mklink failed: {status:?}");
        assert_eq!(
            code(check_read(&link.join("out.nt"), &canon)),
            "path_outside_roots"
        );
        // Writing a new file through the junction is refused too.
        assert_eq!(
            code(check_write(&link.join("new.ttl"), &canon)),
            "path_outside_roots"
        );
    }
}
