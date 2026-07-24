use std::io::IsTerminal;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};

mod format;

use testless_core::cache::{Cache, CachedExtraction};
use testless_core::classify::{classify, ChangeMode, SeedKind};
use testless_core::gitio;
use testless_core::graph::{CallTarget, DefKind, Edge, Graph};
use testless_core::indexer::index_repo_incremental;
use testless_core::walk::impacted_tests;
use testless_core::Registry;

#[derive(Parser)]
#[command(
    name = "testless",
    version,
    after_help = "Examples:\n  testless index\n  testless stats\n  testless changes --from origin/main\n  testless select --from origin/main\n  testless select --from origin/main --format args\n  testless completion zsh > _testless"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

/// Output format for `select`. Defaults (when omitted) to `Json` on a piped
/// stdout and `Text` on a terminal — the same TTY-sniffing convention as
/// `index`/`stats`/`changes`. `Args` is never a default: it must be asked
/// for explicitly with `--format args`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Format {
    Json,
    Text,
    /// Runner-consumable command lines (`vitest run ...`, `go test ...`,
    /// `cargo test ...`) — one per selected test, via `format::command_lines`.
    Args,
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
    /// Select the tests impacted by what changed since `--from` (or a
    /// run-all fallback) and print them.
    Select {
        /// Revision to diff from. Compared against the current worktree.
        #[arg(long, default_value = "HEAD")]
        from: String,
        /// Revision to diff to. Not yet supported — v1 only diffs
        /// `--from` against the worktree.
        #[arg(long)]
        to: Option<String>,
        /// Output format. Defaults to `json` when stdout is piped, `text`
        /// when it's a terminal.
        #[arg(long, value_enum)]
        format: Option<Format>,
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

/// Shared `--from <rev>` pipeline for `changes` and `select`: incrementally
/// (re)indexes the repo, diffs the worktree against `from`, classifies the
/// change into a `ChangeMode`, and saves the (possibly freshly-parsed)
/// cache — deliberately in that order.
///
/// `changed_files`/`classify` must run against the on-disk worktree before
/// the cache is (re)written: saving first would leave a freshly-created (or
/// freshly-modified) `.testless/graph.bin` sitting in the worktree, which
/// `git ls-files --others` would then report as an untracked "changed" file
/// in any repo that hasn't gitignored `.testless/` yet — polluting both the
/// `changed_files` stat and (harmlessly, but wastefully) the importer scan.
///
/// Failure to list changed files (bad rev, `git` missing, an unrecognized
/// git status token) degrades to a run-all fallback rather than a hard
/// error — see Item 2 on `cmd_changes`'s original doc comment; callers
/// still map that to a distinct exit code, not a `main`-reported `Err`.
///
/// Returns the graph, its cached per-file extractions, the classification,
/// and the count of files `changed_files` reported (0 on the degrade-to-
/// run-all path) — the last used only for stats reporting by callers.
fn analyze(from: &str) -> Result<(Graph, Vec<CachedExtraction>, ChangeMode, usize)> {
    let cwd = std::env::current_dir().context("getting current directory")?;
    let reg = registry();
    let cache = cache_for(&cwd);
    let prev = cache.load();

    let (graph, extractions, _stats) =
        index_repo_incremental(&cwd, &reg, prev).context("indexing repo")?;

    let (mode, changed_count) = match gitio::changed_files(&cwd, from, None) {
        Ok(changed) => {
            let mode = classify(&cwd, &graph, &reg, &changed, &extractions, &|p| {
                gitio::show_file(&cwd, from, p)
            });
            (mode, changed.len())
        }
        Err(err) => {
            let reason = format!("listing files changed since --from: {err:#}");
            (ChangeMode::RunAll { reason }, 0)
        }
    };

    cache.save(&graph, &extractions).context("saving cache")?;

    Ok((graph, extractions, mode, changed_count))
}

/// `--from <rev>` diffed against the current worktree, classified into
/// impact seeds (or a run-all fallback). Returns the process exit code: 0
/// for a selection (including an empty one), 2 for run-all. Exit 1 stays
/// reserved for index/cache infrastructure failures (indexing the repo,
/// saving the cache), which are still surfaced as `Err` so `main` reports
/// them.
fn cmd_changes(from: String, to: Option<String>) -> Result<i32> {
    if to.is_some() {
        anyhow::bail!("--to is not yet supported (v1 only diffs --from against the worktree)");
    }

    let (graph, _extractions, mode, changed_count) = analyze(&from)?;

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
                        "changed_files": changed_count,
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

/// The test-runner label for a def's file language, per the `select` wire
/// contract: `ts` -> `vitest`, `go` -> `gotest`, `rust` -> `cargo`. Any
/// other/future registered language degrades to `"unknown"` rather than
/// erroring — a missing runner mapping shouldn't crash test selection.
fn runner_for_lang(lang: &str) -> &'static str {
    match lang {
        "ts" => "vitest",
        "go" => "gotest",
        "rust" => "cargo",
        _ => "unknown",
    }
}

/// One selected test, ready to render in either `select` output format.
struct SelectedTest {
    file: std::path::PathBuf,
    /// The full `test_id` chain (e.g. `["add", "handles negatives"]`), or
    /// (for the vanishingly rare def with no `test_id`) a single-segment
    /// fallback of the def's own name.
    name: Vec<String>,
    runner: &'static str,
    lang: String,
    /// Mirrors `Def::computed_name`: set when any segment of `name` was
    /// truncated because a later segment couldn't be statically resolved
    /// (e.g. a template-literal test title) — consumers should widen their
    /// match pattern rather than expect an exact `name` match.
    computed: bool,
}

/// `--from <rev>` diffed against the current worktree, classified, and
/// (for a `Selection`) walked out to the impacted `TestCase` defs via
/// `walk::impacted_tests`. Returns the process exit code: 0 for a
/// selection (including an empty one), 2 for run-all — mirroring
/// `cmd_changes`'s exit-code contract exactly.
fn cmd_select(from: String, to: Option<String>, format: Option<Format>) -> Result<i32> {
    if to.is_some() {
        anyhow::bail!("--to is not yet supported (v1 only diffs --from against the worktree)");
    }

    let (graph, _extractions, mode, changed_count) = analyze(&from)?;
    // `--format` always wins; omitted, it sniffs the TTY like `changes`
    // does. `Args` is never the sniffed default — it must be requested.
    let resolved_format = format.unwrap_or_else(|| {
        if std::io::stdout().is_terminal() {
            Format::Text
        } else {
            Format::Json
        }
    });

    let seeds = match mode {
        ChangeMode::Selection(seeds) => seeds,
        ChangeMode::RunAll { reason } => {
            match resolved_format {
                Format::Text => println!("run all: {reason}"),
                Format::Json => {
                    let out = serde_json::json!({
                        "version": 1,
                        "mode": "run_all",
                        "reason": reason,
                    });
                    println!("{out}");
                }
                // `args`'s stdout contract is "runner-consumable command
                // lines, nothing else" — a run-all reason isn't one of
                // those, so it goes to stderr instead, same as the
                // selection footer below.
                Format::Args => eprintln!("run all: {reason}"),
            }
            return Ok(2);
        }
    };

    let total_known = count_tests(&graph);
    let seed_count = seeds.len();
    let test_defs = impacted_tests(&graph, &seeds);
    let tests: Vec<SelectedTest> = test_defs
        .into_iter()
        .map(|id| {
            let def = graph.def(id);
            let file = &graph.files[def.file.0 as usize];
            SelectedTest {
                file: file.path.clone(),
                name: def
                    .test_id
                    .clone()
                    .unwrap_or_else(|| vec![def.name.clone()]),
                runner: runner_for_lang(&file.lang),
                lang: file.lang.clone(),
                computed: def.computed_name,
            }
        })
        .collect();

    match resolved_format {
        Format::Text => {
            for t in &tests {
                println!("{} :: {}", t.file.display(), t.name.join(" > "));
            }
            eprintln!(
                "selected {}/{} tests ({} seeds, {} changed files)",
                tests.len(),
                total_known,
                seed_count,
                changed_count
            );
        }
        Format::Json => {
            let tests_json: Vec<_> = tests
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "file": t.file,
                        "name": t.name,
                        "runner": t.runner,
                        "lang": t.lang,
                        "computed": t.computed,
                    })
                })
                .collect();
            let out = serde_json::json!({
                "version": 1,
                "mode": "selection",
                "tests": tests_json,
                "stats": {
                    "total_known": total_known,
                    "selected": tests.len(),
                    "seeds": seed_count,
                    "changed_files": changed_count,
                },
            });
            println!("{out}");
        }
        Format::Args => {
            for line in format::command_lines(&tests) {
                println!("{line}");
            }
            eprintln!(
                "selected {}/{} tests ({} seeds, {} changed files)",
                tests.len(),
                total_known,
                seed_count,
                changed_count
            );
        }
    }

    Ok(0)
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
        Cmd::Select { from, to, format } => cmd_select(from, to, format),
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
