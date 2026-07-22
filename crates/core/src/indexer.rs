use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use tree_sitter::Parser;

use crate::discover::discover;
use crate::graph::{Def, EdgeKind, FileNode, Graph, NodeId};
use crate::language::{Extraction, Registry};

/// Walk `root`, parse every file `registry` matches, and build the full
/// `Graph`: one `FileNode` + one `Def` per extracted def per file (wired
/// with `Contains` edges), plus `Imports` edges between files.
///
/// Two passes: pass 1 parses and extracts every file (needed before pass 2,
/// since resolving a Go import to a package directory requires knowing every
/// file that ended up indexed in that directory). Parsers are reused per
/// language (keyed by `Language::id()`), with the grammar re-set before each
/// parse since it can vary by path within the same language (e.g. TSX).
pub fn index_repo(root: &Path, registry: &Registry) -> Result<Graph> {
    let files = discover(root, registry);

    let mut graph = Graph::default();
    let mut parsers: HashMap<&'static str, Parser> = HashMap::new();

    // Pass 1: parse + extract each file, adding its FileNode and Defs (with
    // Contains edges wired) to the graph. `extractions[i]` holds the
    // per-file `Extraction` (for its imports) alongside the base offset of
    // its defs in `graph.defs`, aligned by index with `files`/`graph.files`.
    let mut extractions: Vec<Extraction> = Vec::with_capacity(files.len());

    for (rel_path, lang) in &files {
        let full_path = root.join(rel_path);
        let src = std::fs::read_to_string(&full_path)
            .with_context(|| format!("reading {}", full_path.display()))?;
        let hash = blake3::hash(src.as_bytes());

        let parser = parsers.entry(lang.id()).or_default();
        let grammar = lang.grammar(rel_path);
        parser
            .set_language(&grammar)
            .with_context(|| format!("setting grammar for {}", rel_path.display()))?;
        let tree = parser
            .parse(&src, None)
            .ok_or_else(|| anyhow!("failed to parse {}", rel_path.display()))?;

        let extraction = lang.extract(&src, &tree);

        let file_id = graph.add_file(FileNode {
            path: rel_path.clone(),
            hash: *hash.as_bytes(),
            lang: lang.id().to_string(),
        });

        let base = graph.defs.len();
        for def in &extraction.defs {
            graph.add_def(Def {
                name: def.name.clone(),
                kind: def.kind,
                file: file_id,
                start_line: def.start_line,
                end_line: def.end_line,
                test_id: def.test_id.clone(),
                computed_name: def.computed_name,
            });
        }

        let module_init_id = graph.module_init(file_id);
        for (i, def) in extraction.defs.iter().enumerate() {
            if def.kind == crate::graph::DefKind::ModuleInit {
                continue;
            }
            let node_id = (base + i) as NodeId;
            let parent_id = match def.parent {
                Some(p) => (base + p) as NodeId,
                None => match module_init_id {
                    Some(m) => m,
                    None => continue,
                },
            };
            graph.add_edge(parent_id, EdgeKind::Contains, node_id);
        }

        extractions.push(extraction);
    }

    // Pass 2: resolve imports now that every file is indexed. A resolved
    // path that matches an indexed file exactly gets a single edge; a Go
    // package-directory result fans out to every *other* indexed file
    // directly under that directory (excluding the importing file itself,
    // so a file importing its own package doesn't get a self-edge).
    // `seen` dedups repeated imports of the same target from the same file
    // (e.g. a type-only import alongside a value import of the same
    // module) down to a single `Imports` edge.
    let mut seen: HashSet<(NodeId, NodeId)> = HashSet::new();
    for (file_id, ((rel_path, lang), extraction)) in files.iter().zip(extractions.iter()).enumerate() {
        let file_id = file_id as NodeId;
        for import in &extraction.imports {
            let Some(resolved) = lang.resolve_import(rel_path, &import.raw, root) else {
                continue;
            };

            if let Some(target_id) = graph.files.iter().position(|f| f.path == resolved) {
                let target_id = target_id as NodeId;
                if seen.insert((file_id, target_id)) {
                    graph.add_edge(file_id, EdgeKind::Imports, target_id);
                }
                continue;
            }

            let dir_targets: Vec<NodeId> = graph
                .files
                .iter()
                .enumerate()
                .filter(|(target_id, f)| {
                    f.path.parent() == Some(resolved.as_path()) && *target_id as NodeId != file_id
                })
                .map(|(target_id, _)| target_id as NodeId)
                .collect();
            for target_id in dir_targets {
                if seen.insert((file_id, target_id)) {
                    graph.add_edge(file_id, EdgeKind::Imports, target_id);
                }
            }
        }
    }

    Ok(graph)
}
