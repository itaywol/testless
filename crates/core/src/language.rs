use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::graph::DefKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRef {
    pub raw: String,
    pub line: u32,
}

/// language-neutral, pre-graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedDef {
    pub name: String,
    pub kind: DefKind,
    pub start_line: u32,
    pub end_line: u32,
    pub test_id: Option<Vec<String>>,
    pub computed_name: bool,
    /// index into the same Vec (contains nesting)
    pub parent: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extraction {
    pub defs: Vec<ExtractedDef>,
    pub imports: Vec<ImportRef>,
}

pub trait Language: Send + Sync {
    /// "ts" | "go"
    fn id(&self) -> &'static str;
    /// ["ts","tsx","js","jsx","mts","cts"]
    fn extensions(&self) -> &'static [&'static str];
    fn grammar(&self, path: &Path) -> tree_sitter::Language;
    fn extract(&self, src: &str, tree: &tree_sitter::Tree) -> Extraction;
    /// raw import specifier -> repo-relative file path, None if external/unresolvable
    fn resolve_import(&self, from_file: &Path, raw: &str, repo_root: &Path) -> Option<PathBuf>;
}

pub struct Registry {
    langs: Vec<Box<dyn Language>>,
}

impl Registry {
    pub fn new(langs: Vec<Box<dyn Language>>) -> Self {
        Self { langs }
    }

    pub fn for_path(&self, path: &Path) -> Option<&dyn Language> {
        let ext = path.extension()?.to_str()?;
        self.langs
            .iter()
            .find(|l| l.extensions().iter().any(|e| e.eq_ignore_ascii_case(ext)))
            .map(|l| l.as_ref())
    }
}

#[cfg(test)]
pub mod tests_support {
    use super::*;

    pub struct Fake;
    impl Language for Fake {
        fn id(&self) -> &'static str { "fake" }
        fn extensions(&self) -> &'static [&'static str] { &["fk"] }
        fn grammar(&self, _: &Path) -> tree_sitter::Language { unimplemented!() }
        fn extract(&self, _: &str, _: &tree_sitter::Tree) -> Extraction {
            Extraction { defs: vec![], imports: vec![] }
        }
        fn resolve_import(&self, _: &Path, _: &str, _: &Path) -> Option<PathBuf> { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::tests_support::Fake;
    use std::path::Path;

    #[test]
    fn registry_matches_extension() {
        let r = Registry::new(vec![Box::new(Fake)]);
        assert_eq!(r.for_path(Path::new("x/y.fk")).unwrap().id(), "fake");
        assert!(r.for_path(Path::new("x/y.rs")).is_none());
    }
}
