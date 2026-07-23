use std::path::PathBuf;

use crate::graph::Graph;
use crate::language::Extraction;

/// One file's cached extraction, keyed by its repo-relative path and content
/// hash — lets `index_repo_incremental` skip re-parsing files whose hash is
/// unchanged since the last run.
pub type CachedExtraction = (PathBuf, [u8; 32], Extraction);

/// Magic + version prefix written before the bincode payload in the cache
/// file. bincode's wire format is non-self-describing — it has no schema
/// tag, so a `Graph`/`CachedExtraction` shape change can deserialize a
/// stale cache into silently wrong data instead of failing loudly. This
/// prefix gives `load` something concrete to check: bump it whenever the
/// cached types' shape changes, and old caches get rejected (and rebuilt)
/// instead of misread.
const CACHE_MAGIC: &[u8; 4] = b"TST2";

/// On-disk cache of a previous index run, rooted at `{repo}/.testless/`.
///
/// Serialized with bincode, which is compact but not self-describing (no
/// embedded schema/type info) — see [`CACHE_MAGIC`] for how `save`/`load`
/// guard against schema drift between binary versions.
pub struct Cache {
    pub root: PathBuf,
}

impl Cache {
    /// Path to the on-disk cache file, `{root}/graph.bin`.
    pub fn file(&self) -> PathBuf {
        self.root.join("graph.bin")
    }

    /// Loads the cached `Graph` and per-file extractions. Returns `None` if
    /// the cache file is missing, unreadable, doesn't start with the
    /// expected magic/version prefix, or fails to deserialize (corrupt) —
    /// callers should silently fall back to a full rebuild.
    pub fn load(&self) -> Option<(Graph, Vec<CachedExtraction>)> {
        let bytes = std::fs::read(self.file()).ok()?;
        let payload = bytes.strip_prefix(CACHE_MAGIC.as_slice())?;
        bincode::deserialize(payload).ok()
    }

    /// Serializes `graph` + `extractions` to `{root}/graph.bin`, prefixed
    /// with the magic/version tag, creating `root` first if it doesn't
    /// exist.
    pub fn save(&self, graph: &Graph, extractions: &[CachedExtraction]) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let mut bytes = CACHE_MAGIC.to_vec();
        bytes.extend(bincode::serialize(&(graph, extractions))?);
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
        let cache = Cache {
            root: tmp.path().join(".testless"),
        };
        assert!(cache.load().is_none());
    }

    #[test]
    fn corrupt_cache_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache {
            root: tmp.path().join(".testless"),
        };
        std::fs::create_dir_all(&cache.root).unwrap();
        std::fs::write(cache.root.join("graph.bin"), b"garbage").unwrap();
        assert!(cache.load().is_none());
    }

    #[test]
    fn save_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache {
            root: tmp.path().join(".testless"),
        };
        let mut g = Graph::default();
        g.add_file(FileNode {
            path: "a.ts".into(),
            hash: [1; 32],
            lang: "ts".into(),
        });
        cache.save(&g, &[]).unwrap();
        let (loaded, extractions) = cache.load().unwrap();
        assert_eq!(loaded.files.len(), 1);
        assert!(extractions.is_empty());
    }

    #[test]
    fn wrong_magic_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache {
            root: tmp.path().join(".testless"),
        };
        std::fs::create_dir_all(&cache.root).unwrap();
        let g = Graph::default();
        let mut bytes = b"TST9".to_vec();
        bytes.extend(
            bincode::serialize(&(g, Vec::<crate::cache::CachedExtraction>::new())).unwrap(),
        );
        std::fs::write(cache.root.join("graph.bin"), bytes).unwrap();
        assert!(cache.load().is_none());
    }

    #[test]
    fn save_creates_parent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache {
            root: tmp.path().join(".testless"),
        };
        assert!(!cache.root.exists());
        cache.save(&Graph::default(), &[]).unwrap();
        assert!(cache.file().exists());
    }
}
