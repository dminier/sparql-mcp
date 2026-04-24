//! DocStore port — write agent-generated markdown to the Docusaurus front.

use std::path::PathBuf;

use anyhow::Result;

/// Abstraction over "write a markdown file to a docs folder".
///
/// The concrete implementation (`crate::infrastructure::FsDocStore`) resolves
/// relative paths inside a configured root, protects against path traversal,
/// and returns the canonical destination path.
pub trait DocStore: Send + Sync {
    /// Write `content` to `rel_path` inside the doc root.
    ///
    /// `rel_path` is relative, e.g. `"architecture/attack-surface.md"`.
    /// Returns the absolute path of the file that was written.
    fn write(&self, rel_path: &str, content: &str) -> Result<PathBuf>;
}
