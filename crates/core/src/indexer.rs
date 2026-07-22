use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use tree_sitter::Parser;

use crate::cache::CachedExtraction;
use crate::discover::discover;
use crate::graph::{Def, EdgeKind, FileNode, Graph, NodeId};
use crate::language::{Extraction, Language, Registry};

/// Counts of work done by [`index_repo_incremental`]: how many files were
/// actually re-parsed with tree-sitter vs. how many reused a previous run's
/// `Extraction` because their content hash was unchanged.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IndexStats {
    pub parsed: usize,
    pub reused: usize,
}

/// Walk `root`, parse every file `registry` matches, and build the full
/// `Graph` from scratch (no previous run to reuse).
pub fn index_repo(root: &Path, registry: &Registry) -> Result<Graph> {
    let (graph, _extractions, _stats) = index_repo_incremental(root, registry, None)?;
    Ok(graph)
}

/// Like [`index_repo`], but hash-gated against `prev`: files whose blake3
/// hash matches a previous run reuse that run's `Extraction` instead of
/// being re-parsed. The `Graph` is always rebuilt fresh from the full set of
/// (reused + newly parsed) extractions — edges are cheap pure lookups, so
/// there's no in-place patching and no risk of dangling ids. Deleted files
/// drop out naturally since they're no longer in the discovered set; renames
/// are just a delete + an add.
pub fn index_repo_incremental(
    root: &Path,
    registry: &Registry,
    prev: Option<(Graph, Vec<CachedExtraction>)>,
) -> Result<(Graph, Vec<CachedExtraction>, IndexStats)> {
    let files = discover(root, registry);

    let mut prev_extractions: HashMap<PathBuf, ([u8; 32], Extraction)> = prev
        .map(|(_, extractions)| {
            extractions
                .into_iter()
                .map(|(path, hash, extraction)| (path, (hash, extraction)))
                .collect()
        })
        .unwrap_or_default();

    let mut graph = Graph::default();
    let mut parsers: HashMap<&'static str, Parser> = HashMap::new();
    let mut stats = IndexStats::default();

    // Pass 1: hash every discovered file; reuse the previous `Extraction`
    // when the hash is unchanged, otherwise parse + extract fresh. Adds each
    // file's `FileNode` and `Def`s (with `Contains` edges wired) to the
    // graph as we go. `hashes`/`extractions` stay aligned by index with
    // `files`/`graph.files` for pass 2 and for building the returned cache.
    let mut hashes: Vec<[u8; 32]> = Vec::with_capacity(files.len());
    let mut extractions: Vec<Extraction> = Vec::with_capacity(files.len());

    for (rel_path, lang) in &files {
        let full_path = root.join(rel_path);
        let src = std::fs::read_to_string(&full_path)
            .with_context(|| format!("reading {}", full_path.display()))?;
        let hash = *blake3::hash(src.as_bytes()).as_bytes();

        let extraction = match prev_extractions.remove(rel_path) {
            Some((prev_hash, prev_extraction)) if prev_hash == hash => {
                stats.reused += 1;
                prev_extraction
            }
            _ => {
                stats.parsed += 1;
                parse_extract(&mut parsers, *lang, rel_path, &src)?
            }
        };

        let file_id = graph.add_file(FileNode {
            path: rel_path.clone(),
            hash,
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

        hashes.push(hash);
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
                if target_id != file_id && seen.insert((file_id, target_id)) {
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

    let cached: Vec<CachedExtraction> = files
        .iter()
        .zip(hashes)
        .zip(extractions)
        .map(|(((rel_path, _lang), hash), extraction)| (rel_path.clone(), hash, extraction))
        .collect();

    Ok((graph, cached, stats))
}

fn parse_extract(
    parsers: &mut HashMap<&'static str, Parser>,
    lang: &dyn Language,
    rel_path: &Path,
    src: &str,
) -> Result<Extraction> {
    let parser = parsers.entry(lang.id()).or_default();
    let grammar = lang.grammar(rel_path);
    parser
        .set_language(&grammar)
        .with_context(|| format!("setting grammar for {}", rel_path.display()))?;
    let tree = parser
        .parse(src, None)
        .ok_or_else(|| anyhow!("failed to parse {}", rel_path.display()))?;
    Ok(lang.extract(src, &tree))
}
