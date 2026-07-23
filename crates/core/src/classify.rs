//! Change classification: maps a set of changed files (`gitio::ChangedFile`)
//! plus the freshly-indexed graph of the "new" side into either a concrete
//! set of `Seed`s (defs whose impact should be walked) or a `RunAll`
//! fallback for changes that can't be soundly narrowed. See
//! `.superpowers/sdd/task-4-brief.md` for the full precedence-ordered rule
//! table this implements.
//!
//! Rule precedence (highest first):
//! 1. Any changed path matching [`is_config_file`] -> `RunAll` immediately.
//! 2. Deleted/renamed-away *indexed* source file -> `RunAll { reason:
//!    "deleted source file" }` — sound if coarse; a later plan can narrow
//!    this to just the file's former importers.
//! 3. Added indexed file -> seed its `TestCase` defs and its `ModuleInit`,
//!    both `SeedKind::Added` (new exports; nothing referenced them before).
//! 4. Modified/Renamed indexed file -> re-parse old vs. new content with
//!    the same `Language` and seed per `diff_defs`'s `DefChange`s.
//! 5. Changed file with no registered `Language` (and not config) -> if any
//!    *indexed* file's raw import text references it (substring match on
//!    basename, e.g. an import of `"./config.json"` matches changed path
//!    `config.json`), seed that importer's `ModuleInit`; otherwise the file
//!    contributes zero seeds (e.g. a README edit).
//! 6. Any I/O/parse error anywhere -> `RunAll` with a reason naming the
//!    file.
//!
//! A zero-seed `Selection` for a nonempty `changed` slice is a valid,
//! expected result (comment-only edits, docs-only changes).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use tree_sitter::Parser;

use crate::cache::CachedExtraction;
use crate::diffdef::{diff_defs, DefChange};
use crate::gitio::{ChangedFile, FileStatus};
use crate::graph::{DefId, DefKind, FileId, Graph};
use crate::language::{Extraction, Language, Registry};

/// What changed about a def, driving how its dependents get re-selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SeedKind {
    Body,
    Signature,
    Added,
    ModuleInit,
}

/// A single def whose impact should be walked, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Seed {
    pub def: DefId,
    pub kind: SeedKind,
}

/// Overall classification result: either a precise set of seeds, or a
/// `RunAll` fallback (with the reason it couldn't be narrowed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeMode {
    Selection(Vec<Seed>),
    RunAll { reason: String },
}

/// Basenames that always trigger a `RunAll` regardless of extension/dir.
const EXACT_CONFIG_NAMES: &[&str] = &[
    "package.json",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "go.mod",
    "go.sum",
    "Cargo.toml",
    "Cargo.lock",
];

/// Whether `path`'s basename matches one of the config globs: exact names in
/// [`EXACT_CONFIG_NAMES`], `tsconfig*.json`, or `.env*`. Matched on the
/// basename alone so nesting depth never matters.
pub fn is_config_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    EXACT_CONFIG_NAMES.contains(&name)
        || (name.starts_with("tsconfig") && name.ends_with(".json"))
        || name.starts_with(".env")
}

/// Classify `changed` against `new_graph` (built by indexing `repo`'s
/// current content) into a `ChangeMode`. `extractions` is the
/// `CachedExtraction` slice returned alongside `new_graph` by
/// `index_repo_incremental` — same order as `new_graph.files`, reused here
/// (instead of re-reading + re-parsing every indexed file) whenever an
/// unrecognized changed path needs an importer scan. `old_src_of` yields the
/// from-rev content of a given (repo-relative) path, `Ok(None)` if it didn't
/// exist there, `Err` for any read failure.
pub fn classify(
    repo: &Path,
    new_graph: &Graph,
    registry: &Registry,
    changed: &[ChangedFile],
    extractions: &[CachedExtraction],
    old_src_of: &dyn Fn(&Path) -> anyhow::Result<Option<String>>,
) -> ChangeMode {
    for c in changed {
        if is_config_file(&c.path) {
            return ChangeMode::RunAll {
                reason: format!("config file changed: {}", c.path.display()),
            };
        }
        if let FileStatus::Renamed { old } = &c.status {
            if is_config_file(old) {
                return ChangeMode::RunAll {
                    reason: format!("config file changed: {}", old.display()),
                };
            }
        }
    }

    let mut seeds: Vec<Seed> = Vec::new();
    let mut needs_import_scan: Vec<PathBuf> = Vec::new();

    for c in changed {
        match classify_one(repo, new_graph, registry, c, old_src_of) {
            Ok(PerFile::Seeds(mut s)) => seeds.append(&mut s),
            Ok(PerFile::ScanImporters(path)) => needs_import_scan.push(path),
            Err(reason) => return ChangeMode::RunAll { reason },
        }
    }

    if !needs_import_scan.is_empty() {
        seeds.append(&mut scan_importers(
            new_graph,
            extractions,
            &needs_import_scan,
        ));
    }

    dedup_seeds(&mut seeds);
    ChangeMode::Selection(seeds)
}

enum PerFile {
    Seeds(Vec<Seed>),
    /// `path` isn't recognized by any registered `Language`: batched up for
    /// a single pass over every indexed file's raw imports.
    ScanImporters(PathBuf),
}

fn classify_one(
    repo: &Path,
    new_graph: &Graph,
    registry: &Registry,
    c: &ChangedFile,
    old_src_of: &dyn Fn(&Path) -> anyhow::Result<Option<String>>,
) -> Result<PerFile, String> {
    if c.status == FileStatus::Deleted {
        if registry.for_path(&c.path).is_some() {
            return Err(format!("deleted source file: {}", c.path.display()));
        }
        return Ok(PerFile::ScanImporters(c.path.clone()));
    }

    let Some(lang) = registry.for_path(&c.path) else {
        return Ok(PerFile::ScanImporters(c.path.clone()));
    };

    let file_id = find_file_id(new_graph, &c.path)
        .ok_or_else(|| format!("indexed file missing from new graph: {}", c.path.display()))?;

    let old_path: &Path = match &c.status {
        FileStatus::Added => return Ok(PerFile::Seeds(seed_added_file(new_graph, file_id))),
        FileStatus::Modified => c.path.as_path(),
        FileStatus::Renamed { old } => old.as_path(),
        FileStatus::Deleted => unreachable!("handled above"),
    };

    let old_src = old_src_of(old_path)
        .map_err(|e| format!("reading old content of {}: {e:#}", old_path.display()))?;
    let Some(old_src) = old_src else {
        // No prior content found (unexpected for Modified/Renamed) — treat
        // gracefully as if the file were newly added rather than erroring.
        return Ok(PerFile::Seeds(seed_added_file(new_graph, file_id)));
    };

    let old_extraction = parse_and_extract(lang, old_path, &old_src)
        .map_err(|e| format!("parsing old content of {}: {e:#}", old_path.display()))?;

    let new_full_path = repo.join(&c.path);
    let new_src = std::fs::read_to_string(&new_full_path)
        .map_err(|e| format!("reading new content of {}: {e}", c.path.display()))?;
    let new_extraction = parse_and_extract(lang, &c.path, &new_src)
        .map_err(|e| format!("parsing new content of {}: {e:#}", c.path.display()))?;

    let changes = diff_defs(&old_extraction, &new_extraction);
    let def_ids: Vec<DefId> = new_graph.defs_in_file(file_id).map(|(id, _)| id).collect();

    let mut seeds = Vec::new();
    for change in changes {
        match change {
            DefChange::BodyChanged { new_idx } => seeds.push(Seed {
                def: def_ids[new_idx],
                kind: SeedKind::Body,
            }),
            DefChange::SigChanged { new_idx } => seeds.push(Seed {
                def: def_ids[new_idx],
                kind: SeedKind::Signature,
            }),
            DefChange::Added { new_idx } => seeds.push(Seed {
                def: def_ids[new_idx],
                kind: SeedKind::Added,
            }),
            DefChange::ModuleInitChanged | DefChange::Removed { .. } => {
                if let Some(m) = new_graph.module_init(file_id) {
                    seeds.push(Seed {
                        def: m,
                        kind: SeedKind::ModuleInit,
                    });
                }
            }
        }
    }
    Ok(PerFile::Seeds(seeds))
}

/// A newly-added file: seed every `TestCase` def plus the file's
/// `ModuleInit`, both `Added` — nothing referenced this file's other defs
/// before, so only its own tests and its top-level side effects need
/// running.
fn seed_added_file(new_graph: &Graph, file_id: FileId) -> Vec<Seed> {
    let mut seeds: Vec<Seed> = new_graph
        .defs_in_file(file_id)
        .filter(|(_, def)| def.kind == DefKind::TestCase)
        .map(|(id, _)| Seed {
            def: id,
            kind: SeedKind::Added,
        })
        .collect();
    if let Some(m) = new_graph.module_init(file_id) {
        seeds.push(Seed {
            def: m,
            kind: SeedKind::Added,
        });
    }
    seeds
}

/// Scan every already-indexed file's raw import text (from `extractions`,
/// which lines up index-for-index with `new_graph.files` — no re-reading or
/// re-parsing) for a reference to any of `changed_paths` (basename substring
/// match). Each match seeds that importing file's `ModuleInit`.
fn scan_importers(
    new_graph: &Graph,
    extractions: &[CachedExtraction],
    changed_paths: &[PathBuf],
) -> Vec<Seed> {
    let stems: Vec<&str> = changed_paths
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .collect();
    if stems.is_empty() {
        return Vec::new();
    }

    let mut seeds = Vec::new();
    for (i, (_path, _hash, extraction)) in extractions.iter().enumerate() {
        let matched = extraction
            .imports
            .iter()
            .any(|imp| stems.iter().any(|stem| imp.raw.contains(stem)));
        if matched {
            let file_id = FileId(i as u32);
            if let Some(m) = new_graph.module_init(file_id) {
                seeds.push(Seed {
                    def: m,
                    kind: SeedKind::ModuleInit,
                });
            }
        }
    }
    seeds
}

fn find_file_id(graph: &Graph, path: &Path) -> Option<FileId> {
    graph
        .files
        .iter()
        .position(|f| f.path == path)
        .map(|i| FileId(i as u32))
}

fn dedup_seeds(seeds: &mut Vec<Seed>) {
    let mut seen: HashSet<Seed> = HashSet::new();
    seeds.retain(|s| seen.insert(*s));
}

/// Parse `src` (the content of `path` at some point in time) with `lang`'s
/// grammar and run its `extract`. Mirrors `indexer::parse_extract`, but
/// self-contained (fresh `Parser` per call) since classification only ever
/// touches a handful of files per invocation.
fn parse_and_extract(lang: &dyn Language, path: &Path, src: &str) -> anyhow::Result<Extraction> {
    let mut parser = Parser::new();
    let grammar = lang.grammar(path);
    parser
        .set_language(&grammar)
        .with_context(|| format!("setting grammar for {}", path.display()))?;
    let tree = parser
        .parse(src, None)
        .ok_or_else(|| anyhow!("failed to parse {}", path.display()))?;
    Ok(lang.extract(src, &tree))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::FileNode;
    use crate::language::tests_support::Fake;
    use anyhow::anyhow;

    #[test]
    fn is_config_file_table() {
        let positives = [
            "package.json",
            "package-lock.json",
            "pnpm-lock.yaml",
            "yarn.lock",
            "tsconfig.json",
            "tsconfig.build.json",
            "go.mod",
            "go.sum",
            "Cargo.toml",
            "Cargo.lock",
            ".env",
            ".env.local",
            "nested/dir/package.json",
            "a/b/c/tsconfig.build.json",
            "a/b/.env.production",
        ];
        for p in positives {
            assert!(is_config_file(Path::new(p)), "{p} should be a config file");
        }

        let negatives = [
            "main.rs",
            "package.json5",
            "src/index.ts",
            "envfile",
            "tsconfig.txt",
            "README.md",
        ];
        for p in negatives {
            assert!(
                !is_config_file(Path::new(p)),
                "{p} should not be a config file"
            );
        }
    }

    #[test]
    fn config_file_precedence_beats_other_rules() {
        // First entry alone (a Deleted, unrecognized-by-this-registry file)
        // would classify as a zero-seed Selection; the second entry being a
        // config file must still force RunAll regardless of position or
        // what the other rules would have produced.
        let graph = Graph::default();
        let registry = Registry::new(vec![]);
        let changed = vec![
            ChangedFile {
                path: PathBuf::from("src/other.ts"),
                status: FileStatus::Deleted,
            },
            ChangedFile {
                path: PathBuf::from("package.json"),
                status: FileStatus::Modified,
            },
        ];
        let mode = classify(
            Path::new("/nonexistent"),
            &graph,
            &registry,
            &changed,
            &[],
            &|_| panic!("old_src_of should never be called once a config file short-circuits"),
        );
        assert!(
            matches!(mode, ChangeMode::RunAll { .. }),
            "expected RunAll, got {mode:?}"
        );
    }

    #[test]
    fn old_src_error_triggers_run_all_mentioning_the_file() {
        let mut graph = Graph::default();
        graph.add_file(FileNode {
            path: PathBuf::from("foo.fk"),
            hash: [0; 32],
            lang: "fake".into(),
        });
        let registry = Registry::new(vec![Box::new(Fake)]);
        let changed = vec![ChangedFile {
            path: PathBuf::from("foo.fk"),
            status: FileStatus::Modified,
        }];
        let mode = classify(
            Path::new("/nonexistent"),
            &graph,
            &registry,
            &changed,
            &[],
            &|_| Err(anyhow!("boom")),
        );
        match mode {
            ChangeMode::RunAll { reason } => {
                assert!(
                    reason.contains("foo.fk"),
                    "reason should mention the file: {reason}"
                );
            }
            other => panic!("expected RunAll, got {other:?}"),
        }
    }
}
