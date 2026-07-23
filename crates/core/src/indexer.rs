use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use tree_sitter::Parser;

use crate::cache::CachedExtraction;
use crate::discover::discover;
use crate::graph::{CallTarget, Def, DefId, Edge, FileId, FileNode, Graph};
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
    // Per-file base offset into `graph.defs`, aligned by index with
    // `files`/`extractions` — lets pass 3 map an `ExtractedRef.from_def`
    // (an index into that file's own `Extraction.defs`) back to a `DefId`.
    let mut file_def_base: Vec<usize> = Vec::with_capacity(files.len());

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
        file_def_base.push(base);
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
            let node_id = DefId((base + i) as u32);
            let parent_id = match def.parent {
                Some(p) => DefId((base + p) as u32),
                None => match module_init_id {
                    Some(m) => m,
                    None => continue,
                },
            };
            graph.add_edge(Edge::Contains {
                parent: parent_id,
                child: node_id,
            });
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
    // Built once so exact-path and Go dir-fanout import resolution are O(1)
    // hashmap lookups instead of an O(files) scan per import.
    let path_to_file: HashMap<PathBuf, FileId> = graph
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.path.clone(), FileId(i as u32)))
        .collect();
    let mut dir_to_files: HashMap<PathBuf, Vec<FileId>> = HashMap::new();
    for (i, f) in graph.files.iter().enumerate() {
        if let Some(parent) = f.path.parent() {
            dir_to_files
                .entry(parent.to_path_buf())
                .or_default()
                .push(FileId(i as u32));
        }
    }

    let mut seen: HashSet<(FileId, FileId)> = HashSet::new();
    for (file_id, ((rel_path, lang), extraction)) in
        files.iter().zip(extractions.iter()).enumerate()
    {
        let file_id = FileId(file_id as u32);
        for import in &extraction.imports {
            let Some(resolved) = lang.resolve_import(rel_path, &import.raw, root) else {
                continue;
            };

            if let Some(&target_id) = path_to_file.get(&resolved) {
                if target_id != file_id && seen.insert((file_id, target_id)) {
                    graph.add_edge(Edge::Imports {
                        from: file_id,
                        to: target_id,
                    });
                }
                continue;
            }

            if let Some(targets) = dir_to_files.get(&resolved) {
                for &target_id in targets {
                    if target_id != file_id && seen.insert((file_id, target_id)) {
                        graph.add_edge(Edge::Imports {
                            from: file_id,
                            to: target_id,
                        });
                    }
                }
            }
        }
    }

    // Pass 3: resolve calls/reads to tier-1 candidates. Scope for a ref in
    // file F is F itself plus every file F `Imports` (reusing `seen`, which
    // pass 2 already built as exactly that from/to set). Defs are indexed
    // under their *short* name — a method def like `Calc.push` is indexed
    // under `push` too — so a bare-identifier ref matches both plain
    // functions and qualified methods whether or not `ref.qualifier` is
    // set (the receiver variable rarely equals the type name, so this is a
    // deliberate over-approximation). `ModuleInit` defs (name `<module>`)
    // are excluded from candidacy since nothing ever references them by
    // name.
    let mut imports_of: HashMap<FileId, Vec<FileId>> = HashMap::new();
    for (from, to) in &seen {
        imports_of.entry(*from).or_default().push(*to);
    }

    // Keyed by file first, then short name — lets `candidates_for` look up
    // `by_short_name.get(f).and_then(|m| m.get(name))` with a borrowed
    // `&str` instead of allocating a `String` per lookup per scope file.
    let mut by_short_name: HashMap<FileId, HashMap<String, Vec<DefId>>> = HashMap::new();
    for (i, def) in graph.defs.iter().enumerate() {
        if def.kind == crate::graph::DefKind::ModuleInit {
            continue;
        }
        let short = def.name.rsplit('.').next().unwrap_or(&def.name).to_string();
        by_short_name
            .entry(def.file)
            .or_default()
            .entry(short)
            .or_default()
            .push(DefId(i as u32));
    }

    let mut calls_seen: HashSet<(DefId, DefId)> = HashSet::new();
    let mut reads_seen: HashSet<(DefId, DefId)> = HashSet::new();
    let mut unknown_seen: HashSet<(DefId, String)> = HashSet::new();

    for (file_idx, extraction) in extractions.iter().enumerate() {
        let file_id = FileId(file_idx as u32);
        let base = file_def_base[file_idx];

        let mut scope: Vec<FileId> = vec![file_id];
        if let Some(targets) = imports_of.get(&file_id) {
            scope.extend(targets.iter().copied());
        }
        let candidates_for = |name: &str| -> Vec<DefId> {
            scope
                .iter()
                .filter_map(|f| by_short_name.get(f).and_then(|m| m.get(name)))
                .flatten()
                .copied()
                .collect()
        };

        for r in &extraction.calls {
            let from = DefId((base + r.from_def) as u32);
            // `emitted` tracks whether this ref produced at least one
            // `Resolved` edge (new or already-deduped) — not just whether
            // `candidates` was non-empty. A ref whose only candidates are
            // all self-edges (e.g. recursion where the recursive callee is
            // the sole same-named def in scope) must still widen to
            // `Unknown` rather than silently vanishing: a real cross-file
            // callee with the same name whose import failed to resolve
            // would look identical, and dropping it without a trace would
            // violate "every call ref yields >=1 edge".
            let mut emitted = false;
            for c in candidates_for(&r.name) {
                if c != from {
                    emitted = true;
                    if calls_seen.insert((from, c)) {
                        graph.add_edge(Edge::Calls {
                            from,
                            to: CallTarget::Resolved(c),
                        });
                    }
                }
            }
            if !emitted && unknown_seen.insert((from, r.name.clone())) {
                graph.add_edge(Edge::Calls {
                    from,
                    to: CallTarget::Unknown(r.name.clone()),
                });
            }
        }

        for r in &extraction.reads {
            let from = DefId((base + r.from_def) as u32);
            for c in candidates_for(&r.name) {
                if c != from && reads_seen.insert((from, c)) {
                    graph.add_edge(Edge::Reads { from, to: c });
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
