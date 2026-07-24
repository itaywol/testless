use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DefId(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FileId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DefKind {
    Function,
    Method,
    Class,
    TestCase,
    ModuleInit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edge {
    Contains { parent: DefId, child: DefId },
    Imports { from: FileId, to: FileId },
    Calls { from: DefId, to: CallTarget },
    Reads { from: DefId, to: DefId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallTarget {
    Resolved(DefId),
    Unknown(String), // name recorded for walk-time widening
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Def {
    pub name: String,
    pub kind: DefKind,
    pub file: FileId,
    pub start_line: u32,
    pub end_line: u32,
    pub test_id: Option<Vec<String>>,
    pub computed_name: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    pub path: PathBuf,
    pub hash: [u8; 32],
    pub lang: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Graph {
    pub files: Vec<FileNode>,
    pub defs: Vec<Def>,
    pub edges: Vec<Edge>,
    /// `ModuleInit` def per file, indexed by `FileId`; built incrementally
    /// in `add_def` so `module_init` is an O(1) lookup instead of an
    /// O(defs) scan. Kept in sync with `add_file` (which appends `None` so
    /// the vec stays aligned with `files`) and `add_def`.
    module_inits: Vec<Option<DefId>>,
    /// Defs belonging to each file, indexed by `FileId`; built
    /// incrementally in `add_def` so `defs_in_file` is O(defs in that
    /// file) instead of an O(all defs) scan.
    file_defs: Vec<Vec<DefId>>,
}

impl Graph {
    pub fn add_file(&mut self, f: FileNode) -> FileId {
        self.files.push(f);
        self.module_inits.push(None);
        self.file_defs.push(Vec::new());
        FileId((self.files.len() - 1) as u32)
    }
    pub fn add_def(&mut self, d: Def) -> DefId {
        let file = d.file;
        let kind = d.kind;
        self.defs.push(d);
        let id = DefId((self.defs.len() - 1) as u32);
        self.file_defs[file.0 as usize].push(id);
        if kind == DefKind::ModuleInit {
            self.module_inits[file.0 as usize] = Some(id);
        }
        id
    }
    pub fn add_edge(&mut self, e: Edge) {
        self.edges.push(e);
    }
    pub fn def(&self, id: DefId) -> &Def {
        &self.defs[id.0 as usize]
    }
    pub fn defs_in_file(&self, f: FileId) -> impl Iterator<Item = (DefId, &Def)> {
        self.file_defs[f.0 as usize]
            .iter()
            .map(move |&id| (id, self.def(id)))
    }
    pub fn module_init(&self, f: FileId) -> Option<DefId> {
        self.module_inits[f.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn f(path: &str) -> FileNode {
        FileNode {
            path: PathBuf::from(path),
            hash: [0; 32],
            lang: "ts".into(),
        }
    }
    fn d(name: &str, kind: DefKind, file: FileId) -> Def {
        Def {
            name: name.into(),
            kind,
            file,
            start_line: 1,
            end_line: 2,
            test_id: None,
            computed_name: false,
        }
    }

    #[test]
    fn add_and_query() {
        let mut g = Graph::default();
        let fa = g.add_file(f("a.ts"));
        let fb = g.add_file(f("b.ts"));
        let init = g.add_def(d("<module>", DefKind::ModuleInit, fa));
        let add = g.add_def(d("add", DefKind::Function, fa));
        let other = g.add_def(d("x", DefKind::Function, fb));
        g.add_edge(Edge::Contains {
            parent: init,
            child: add,
        });

        assert_eq!(g.defs_in_file(fa).count(), 2);
        assert_eq!(
            g.defs_in_file(fb).map(|(id, _)| id).collect::<Vec<_>>(),
            vec![other]
        );
        assert_eq!(g.module_init(fa), Some(init));
        assert_eq!(g.module_init(fb), None);
    }

    #[test]
    fn roundtrips_through_bincode() {
        let mut g = Graph::default();
        let fa = g.add_file(f("a.ts"));
        g.add_def(d("add", DefKind::Function, fa));
        let bytes = bincode::serialize(&g).unwrap();
        let g2: Graph = bincode::deserialize(&bytes).unwrap();
        assert_eq!(g2.defs.len(), 1);
        assert_eq!(g2.files[0].path, PathBuf::from("a.ts"));
    }
}
