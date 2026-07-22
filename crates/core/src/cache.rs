use std::path::PathBuf;

use crate::graph::Graph;
use crate::language::Extraction;

/// One file's cached extraction, keyed by its repo-relative path and content
/// hash — lets `index_repo_incremental` skip re-parsing files whose hash is
/// unchanged since the last run.
pub type CachedExtraction = (PathBuf, [u8; 32], Extraction);

/// On-disk cache of a previous index run, rooted at `{repo}/.pick-a-test/`.
pub struct Cache {
    pub root: PathBuf,
}

impl Cache {
    fn file(&self) -> PathBuf {
        self.root.join("graph.bin")
    }

    /// Loads the cached `Graph` and per-file extractions. Returns `None` if
    /// the cache file is missing, unreadable, or fails to deserialize
    /// (corrupt) — callers should silently fall back to a full rebuild.
    pub fn load(&self) -> Option<(Graph, Vec<CachedExtraction>)> {
        let bytes = std::fs::read(self.file()).ok()?;
        bincode::deserialize(&bytes).ok()
    }

    /// Serializes `graph` + `extractions` to `{root}/graph.bin`, creating
    /// `root` first if it doesn't exist.
    pub fn save(&self, graph: &Graph, extractions: &[CachedExtraction]) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let bytes = bincode::serialize(&(graph, extractions))?;
        std::fs::write(self.file(), bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::FileNode;

    #[test]
    fn missing_cache_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache { root: tmp.path().join(".pick-a-test") };
        assert!(cache.load().is_none());
    }

    #[test]
    fn corrupt_cache_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache { root: tmp.path().join(".pick-a-test") };
        std::fs::create_dir_all(&cache.root).unwrap();
        std::fs::write(cache.root.join("graph.bin"), b"garbage").unwrap();
        assert!(cache.load().is_none());
    }

    #[test]
    fn save_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache { root: tmp.path().join(".pick-a-test") };
        let mut g = Graph::default();
        g.add_file(FileNode { path: "a.ts".into(), hash: [1; 32], lang: "ts".into() });
        cache.save(&g, &[]).unwrap();
        let (loaded, extractions) = cache.load().unwrap();
        assert_eq!(loaded.files.len(), 1);
        assert!(extractions.is_empty());
    }

    #[test]
    fn save_creates_parent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache { root: tmp.path().join(".pick-a-test") };
        assert!(!cache.root.exists());
        cache.save(&Graph::default(), &[]).unwrap();
        assert!(cache.file().exists());
    }
}
