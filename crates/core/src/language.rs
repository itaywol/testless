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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedRef {
    /// index into Extraction.defs (enclosing def; module_init if top-level)
    pub from_def: usize,
    /// referenced symbol name ("add")
    pub name: String,
    /// receiver/namespace text if any ("calc", "math", "ns")
    pub qualifier: Option<String>,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extraction {
    pub defs: Vec<ExtractedDef>,
    pub imports: Vec<ImportRef>,
    /// call sites
    pub calls: Vec<ExtractedRef>,
    /// non-call identifier references
    pub reads: Vec<ExtractedRef>,
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
        fn id(&self) -> &'static str {
            "fake"
        }
        fn extensions(&self) -> &'static [&'static str] {
            &["fk"]
        }
        fn grammar(&self, _: &Path) -> tree_sitter::Language {
            unimplemented!()
        }
        fn extract(&self, _: &str, _: &tree_sitter::Tree) -> Extraction {
            Extraction {
                defs: vec![],
                imports: vec![],
                calls: vec![],
                reads: vec![],
            }
        }
        fn resolve_import(&self, _: &Path, _: &str, _: &Path) -> Option<PathBuf> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::Fake;
    use super::*;
    use std::path::Path;

    #[test]
    fn registry_matches_extension() {
        let r = Registry::new(vec![Box::new(Fake)]);
        assert_eq!(r.for_path(Path::new("x/y.fk")).unwrap().id(), "fake");
        assert_eq!(r.for_path(Path::new("x/y.FK")).unwrap().id(), "fake");
        assert!(r.for_path(Path::new("x/y.rs")).is_none());
    }

    #[test]
    fn extraction_calls_reads_roundtrip_through_serde() {
        let extraction = Extraction {
            defs: vec![],
            imports: vec![],
            calls: vec![ExtractedRef {
                from_def: 0,
                name: "add".to_string(),
                qualifier: Some("calc".to_string()),
                line: 12,
            }],
            reads: vec![ExtractedRef {
                from_def: 1,
                name: "counter".to_string(),
                qualifier: None,
                line: 7,
            }],
        };

        let bytes = bincode::serialize(&extraction).unwrap();
        let roundtripped: Extraction = bincode::deserialize(&bytes).unwrap();

        assert_eq!(roundtripped.calls, extraction.calls);
        assert_eq!(roundtripped.reads, extraction.reads);
    }
}
