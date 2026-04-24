//! FsDocStore — writes markdown files to a configured root directory.

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::domain::DocStore;

pub struct FsDocStore {
    root: PathBuf,
}

impl FsDocStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl DocStore for FsDocStore {
    fn write(&self, rel_path: &str, content: &str) -> Result<PathBuf> {
        // Reject path traversal attempts.
        let rel = Path::new(rel_path);
        for component in rel.components() {
            match component {
                Component::ParentDir => bail!("path traversal rejected: {rel_path}"),
                Component::RootDir | Component::Prefix(_) => {
                    bail!("absolute path rejected: {rel_path}")
                }
                _ => {}
            }
        }

        let dest = self.root.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating directories for {}", dest.display()))?;
        }
        fs::write(&dest, content).with_context(|| format!("writing doc to {}", dest.display()))?;
        Ok(dest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_creates_file_and_subdirs() {
        let dir = TempDir::new().unwrap();
        let store = FsDocStore::new(dir.path().to_path_buf());
        let dest = store.write("sub/page.md", "# Hello").unwrap();
        assert!(dest.exists());
        assert_eq!(fs::read_to_string(dest).unwrap(), "# Hello");
    }

    #[test]
    fn write_rejects_path_traversal() {
        let dir = TempDir::new().unwrap();
        let store = FsDocStore::new(dir.path().to_path_buf());
        assert!(store.write("../../etc/passwd", "bad").is_err());
    }

    #[test]
    fn write_rejects_absolute_path() {
        let dir = TempDir::new().unwrap();
        let store = FsDocStore::new(dir.path().to_path_buf());
        assert!(store.write("/etc/passwd", "bad").is_err());
    }
}
