use std::io::IsTerminal;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};

use testless_core::cache::Cache;
use testless_core::classify::{classify, ChangeMode, SeedKind};
use testless_core::gitio;
use testless_core::graph::{CallTarget, DefKind, Edge, Graph};
use testless_core::indexer::index_repo_incremental;
use testless_core::Registry;

#[derive(Parser)]
#[command(
    name = "testless",
    after_help = "Examples:\n  testless index\n  testless stats\n  testless changes --from origin/main\n  testless completion zsh > _testless"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Index the repo rooted at the current directory and save the cache.
    Index {
        /// Ignore any existing cache and reparse every file.
        #[arg(long)]
        full: bool,
    },
    /// Print counts from the existing cache.
    Stats,
    /// Classify what changed since `--from` into impacted-def seeds (or a
    /// run-all fallback) and print them.
    Changes {
        /// Revision to diff from. Compared against the current worktree.
        #[arg(long, default_value = "HEAD")]
        from: String,
        /// Revision to diff to. Not yet supported — v1 always compares
        /// `--from` against the worktree.
        #[arg(long)]
        to: Option<String>,
    },
    /// Generate a shell completion script and print it to stdout.
    Completion {
        /// Shell to generate completions for (bash, zsh, fish, elvish, powershell).
        shell: clap_complete::Shell,
    },
}

fn registry() -> Registry {
    Registry::new(vec![
        Box::new(testless_lang_ts::TsLanguage),
        Box::new(testless_lang_go::GoLanguage),
        Box::new(testless_lang_rust::RustLanguage),
    ])
}

fn cache_for(cwd: &std::path::Path) -> Cache {
    Cache {
        root: cwd.join(".testless"),
    }
}

fn count_tests(graph: &Graph) -> usize {
    graph
        .defs
        .iter()
        .filter(|d| d.kind == DefKind::TestCase)
        .count()
}

struct EdgeCounts {
    calls: usize,
    reads: usize,
    unresolved: usize,
}

fn count_edges(graph: &Graph) -> EdgeCounts {
    let mut calls = 0;
    let mut reads = 0;
    let mut unresolved = 0;
    for edge in &graph.edges {
        match edge {
            Edge::Calls { to, .. } => {
                calls += 1;
                if matches!(to, CallTarget::Unknown(_)) {
                    unresolved += 1;
                }
            }
            Edge::Reads { .. } => reads += 1,
            _ => {}
        }
    }
    EdgeCounts {
        calls,
        reads,
        unresolved,
    }
}

fn cmd_index(full: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("getting current directory")?;
    let cache = cache_for(&cwd);
    let prev = if full { None } else { cache.load() };

    eprintln!("indexing {}...", cwd.display());
    let start = Instant::now();
    let (graph, extractions, stats) =
        index_repo_incremental(&cwd, &registry(), prev).context("indexing repo")?;
    let ms = start.elapsed().as_millis() as u64;

    cache.save(&graph, &extractions).context("saving cache")?;
    eprintln!(
        "indexed {} files ({} parsed, {} reused) in {}ms",
        graph.files.len(),
        stats.parsed,
        stats.reused,
        ms
    );

    let files = graph.files.len();
    let defs = graph.defs.len();
    let tests = count_tests(&graph);
    let edge_counts = count_edges(&graph);

    if std::io::stdout().is_terminal() {
        println!("Indexed {files} files: {defs} defs ({tests} tests)");
        println!(
            "  parsed: {}  reused: {}  time: {}ms",
            stats.parsed, stats.reused, ms
        );
        println!(
            "  calls: {}  reads: {}  unresolved: {}",
            edge_counts.calls, edge_counts.reads, edge_counts.unresolved
        );
    } else {
        let out = serde_json::json!({
            "version": 1,
            "files": files,
            "defs": defs,
            "tests": tests,
            "parsed": stats.parsed,
            "reused": stats.reused,
            "ms": ms,
            "calls": edge_counts.calls,
            "reads": edge_counts.reads,
            "unresolved": edge_counts.unresolved,
        });
        println!("{out}");
    }
    Ok(())
}

fn cmd_stats() -> Result<()> {
    let cwd = std::env::current_dir().context("getting current directory")?;
    let cache = cache_for(&cwd);
    let Some((graph, _extractions)) = cache.load() else {
        anyhow::bail!("no index — run: testless index");
    };

    let files = graph.files.len();
    let defs = graph.defs.len();
    let tests = count_tests(&graph);
    let edges = graph.edges.len();
    let edge_counts = count_edges(&graph);
    let cache_bytes = std::fs::metadata(cache.file())
        .map(|m| m.len())
        .unwrap_or(0);

    if std::io::stdout().is_terminal() {
        println!("Cache: {}", cache.root.display());
        println!(
            "files: {files}  defs: {defs}  tests: {tests}  edges: {edges}  size: {cache_bytes} bytes"
        );
        println!(
            "calls: {}  reads: {}  unresolved: {}",
            edge_counts.calls, edge_counts.reads, edge_counts.unresolved
        );
    } else {
        let out = serde_json::json!({
            "version": 1,
            "files": files,
            "defs": defs,
            "tests": tests,
            "edges": edges,
            "cache_bytes": cache_bytes,
            "calls": edge_counts.calls,
            "reads": edge_counts.reads,
            "unresolved": edge_counts.unresolved,
        });
        println!("{out}");
    }
    Ok(())
}

/// Machine-readable label for a `SeedKind`, matching the wire format
/// documented for `testless changes` (lowercase, snake_case for
/// `ModuleInit`) — deliberately not `SeedKind`'s own `Serialize` (which
/// would emit PascalCase variant names).
fn seed_kind_label(kind: SeedKind) -> &'static str {
    match kind {
        SeedKind::Body => "body",
        SeedKind::Signature => "signature",
        SeedKind::Added => "added",
        SeedKind::ModuleInit => "module_init",
    }
}

/// `--from <rev>` diffed against the current worktree, classified into
/// impact seeds (or a run-all fallback). Returns the process exit code: 0
/// for a selection (including an empty one), 2 for run-all. A hard
/// infrastructure error (bad rev, `git` missing, I/O failure) is instead
/// surfaced as `Err` so `main` reports it and exits 1.
fn cmd_changes(from: String, to: Option<String>) -> Result<i32> {
    if to.is_some() {
        anyhow::bail!("--to is not yet supported (v1 only diffs --from against the worktree)");
    }

    let cwd = std::env::current_dir().context("getting current directory")?;
    let reg = registry();
    let cache = cache_for(&cwd);
    let prev = cache.load();

    let (graph, extractions, _stats) =
        index_repo_incremental(&cwd, &reg, prev).context("indexing repo")?;
    cache.save(&graph, &extractions).context("saving cache")?;

    let changed =
        gitio::changed_files(&cwd, &from, None).context("listing files changed since --from")?;

    let mode = classify(&cwd, &graph, &reg, &changed, &extractions, &|p| {
        gitio::show_file(&cwd, &from, p)
    });

    let exit_code = match &mode {
        ChangeMode::Selection(_) => 0,
        ChangeMode::RunAll { .. } => 2,
    };

    if std::io::stdout().is_terminal() {
        match &mode {
            ChangeMode::Selection(seeds) if seeds.is_empty() => {
                println!("no impacted defs");
            }
            ChangeMode::Selection(seeds) => {
                for seed in seeds {
                    let def = graph.def(seed.def);
                    let file = &graph.files[def.file.0 as usize].path;
                    println!(
                        "{} :: {} [{}]",
                        file.display(),
                        def.name,
                        seed_kind_label(seed.kind)
                    );
                }
            }
            ChangeMode::RunAll { reason } => {
                println!("run all: {reason}");
            }
        }
    } else {
        let out = match &mode {
            ChangeMode::Selection(seeds) => {
                let seeds_json: Vec<_> = seeds
                    .iter()
                    .map(|seed| {
                        let def = graph.def(seed.def);
                        let file = &graph.files[def.file.0 as usize].path;
                        serde_json::json!({
                            "def": def.name,
                            "file": file,
                            "kind": seed_kind_label(seed.kind),
                        })
                    })
                    .collect();
                serde_json::json!({
                    "version": 1,
                    "mode": "selection",
                    "seeds": seeds_json,
                    "stats": {
                        "changed_files": changed.len(),
                        "seeds": seeds.len(),
                    },
                })
            }
            ChangeMode::RunAll { reason } => serde_json::json!({
                "version": 1,
                "mode": "run_all",
                "reason": reason,
            }),
        };
        println!("{out}");
    }

    Ok(exit_code)
}

fn cmd_completion(shell: clap_complete::Shell) -> Result<()> {
    clap_complete::generate(
        shell,
        &mut Cli::command(),
        "testless",
        &mut std::io::stdout(),
    );
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Index { full } => cmd_index(full).map(|()| 0),
        Cmd::Stats => cmd_stats().map(|()| 0),
        Cmd::Changes { from, to } => cmd_changes(from, to),
        Cmd::Completion { shell } => cmd_completion(shell).map(|()| 0),
    };

    match result {
        Ok(code) => {
            if code != 0 {
                std::process::exit(code);
            }
        }
        Err(err) => {
            eprintln!("error: {err}");
            for cause in err.chain().skip(1) {
                eprintln!("  caused by: {cause}");
            }
            std::process::exit(1);
        }
    }
}
