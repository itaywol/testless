use std::io::IsTerminal;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};

mod format;

use testless_core::cache::{Cache, CachedExtraction};
use testless_core::classify::{classify, ChangeMode, SeedKind};
use testless_core::config::{self, Config};
use testless_core::gitio;
use testless_core::graph::{CallTarget, DefId, DefKind, Edge, Graph};
use testless_core::indexer::index_repo_incremental;
use testless_core::walk::{impacted_tests, impacted_tests_with_paths, Hop, HopKind};
use testless_core::Registry;

#[derive(Parser)]
#[command(
    name = "testless",
    version,
    after_help = "Examples:\n  testless index\n  testless stats\n  testless changes --from origin/main\n  testless select --from origin/main\n  testless select --from origin/main --format args\n  testless why \"formats a sum\"\n  testless completion zsh > _testless"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

/// Output format for `select`. Defaults (when omitted) to `Json` on a piped
/// stdout and `Text` on a terminal: the same TTY-sniffing convention as
/// `index`/`stats`/`changes`. `Args` is never a default: it must be asked
/// for explicitly with `--format args`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Format {
    Json,
    Text,
    /// Runner-consumable command lines (`vitest run ...`, `go test ...`,
    /// `cargo test ...`), one per selected test, via `format::command_lines`.
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
        /// Revision to diff to. Not yet supported: v1 always compares
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
        /// Revision to diff to. Not yet supported: v1 only diffs
        /// `--from` against the worktree.
        #[arg(long)]
        to: Option<String>,
        /// Output format. Defaults to `json` when stdout is piped, `text`
        /// when it's a terminal.
        #[arg(long, value_enum)]
        format: Option<Format>,
    },
    /// Explain why a test was selected: prints the hop path from a change
    /// seed to the matching test.
    Why {
        /// Which test to explain. Forgiving substring match against
        /// `<file> :: <name chain>` (e.g. a bare test name, a file path
        /// prefix, or the full `file :: chain` string all work).
        test_id: String,
        /// Revision to diff from. Compared against the current worktree.
        #[arg(long, default_value = "HEAD")]
        from: String,
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

/// Loads `{cwd}/testless.toml` (see `testless_core::config::Config`) and
/// eagerly validates both glob lists, so a malformed config (bad TOML, or a
/// syntactically invalid glob pattern in either `ignore` or `always-run`)
/// fails loudly right here, in every command that reaches it, rather than
/// only surfacing later when a glob is actually evaluated. This is a hard
/// error (bubbles up through `main`'s `Err` branch, exit 1): a `testless.toml`
/// the user wrote and got wrong deserves a clear failure, not a silent
/// `run_all` degrade.
fn load_config(cwd: &std::path::Path) -> Result<Config> {
    let config = Config::load(cwd).context("loading testless.toml")?;
    config
        .ignore_globset()
        .context("parsing testless.toml `ignore` globs")?;
    config
        .always_run_globset()
        .context("parsing testless.toml `always-run` globs")?;
    Ok(config)
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
    let config = load_config(&cwd)?;
    let ignore = config
        .ignore_globset()
        .context("parsing testless.toml ignore globs")?;

    eprintln!("indexing {}...", cwd.display());
    let start = Instant::now();
    let (graph, extractions, stats) =
        index_repo_incremental(&cwd, &registry(), Some(&ignore), prev).context("indexing repo")?;
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
        anyhow::bail!("no index, run: testless index");
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
/// `ModuleInit`), deliberately not `SeedKind`'s own `Serialize` (which
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
/// cache, deliberately in that order.
///
/// `changed_files`/`classify` must run against the on-disk worktree before
/// the cache is (re)written: saving first would leave a freshly-created (or
/// freshly-modified) `.testless/graph.bin` sitting in the worktree, which
/// `git ls-files --others` would then report as an untracked "changed" file
/// in any repo that hasn't gitignored `.testless/` yet, polluting both the
/// `changed_files` stat and (harmlessly, but wastefully) the importer scan.
///
/// Failure to list changed files (bad rev, `git` missing, an unrecognized
/// git status token) degrades to a run-all fallback rather than a hard
/// error. See Item 2 on `cmd_changes`'s original doc comment; callers
/// still map that to a distinct exit code, not a `main`-reported `Err`.
///
/// Returns the graph, its cached per-file extractions, the classification,
/// the count of files `changed_files` reported (0 on the degrade-to-
/// run-all path, used only for stats reporting by callers), and the loaded
/// `testless.toml` config (empty defaults if none exists), so callers that
/// need `always_run` (namely `select`/`why`) don't have to reload it.
fn analyze(from: &str) -> Result<(Graph, Vec<CachedExtraction>, ChangeMode, usize, Config)> {
    let cwd = std::env::current_dir().context("getting current directory")?;
    let reg = registry();
    let cache = cache_for(&cwd);
    let prev = cache.load();

    let config = load_config(&cwd)?;
    let ignore = config
        .ignore_globset()
        .context("parsing testless.toml ignore globs")?;

    let (graph, extractions, _stats) =
        index_repo_incremental(&cwd, &reg, Some(&ignore), prev).context("indexing repo")?;

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

    Ok((graph, extractions, mode, changed_count, config))
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

    let (graph, _extractions, mode, changed_count, _config) = analyze(&from)?;

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

/// The impact walk's selected `TestCase` defs, unioned with every
/// `TestCase` whose file matches one of `config`'s `always-run` globs (see
/// `testless_core::config::always_run_matches`). This is the escape hatch's
/// selection-level half: a smoke test matching an `always-run` glob is
/// selected even when the walk itself found nothing to seed (e.g. a
/// comment-only edit), because it's never reached via any `Seed` at all.
/// Ascending `DefId` order, matching `impacted_tests`'s own determinism.
fn selected_test_defs(
    graph: &Graph,
    seeds: &[testless_core::classify::Seed],
    config: &Config,
) -> Result<Vec<DefId>> {
    let mut selected: std::collections::BTreeSet<DefId> =
        impacted_tests(graph, seeds).into_iter().collect();
    for (id, _glob) in config::always_run_matches(graph, config)? {
        selected.insert(id);
    }
    Ok(selected.into_iter().collect())
}

/// The test-runner label for a def's file language, per the `select` wire
/// contract: `ts` -> `vitest`, `go` -> `gotest`, `rust` -> `cargo`. Any
/// other/future registered language degrades to `"unknown"` rather than
/// erroring: a missing runner mapping shouldn't crash test selection.
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
    /// (e.g. a template-literal test title); consumers should widen their
    /// match pattern rather than expect an exact `name` match.
    computed: bool,
}

/// `--from <rev>` diffed against the current worktree, classified, and
/// (for a `Selection`) walked out to the impacted `TestCase` defs via
/// `walk::impacted_tests`. Returns the process exit code: 0 for a
/// selection (including an empty one), 2 for run-all, mirroring
/// `cmd_changes`'s exit-code contract exactly.
fn cmd_select(from: String, to: Option<String>, format: Option<Format>) -> Result<i32> {
    if to.is_some() {
        anyhow::bail!("--to is not yet supported (v1 only diffs --from against the worktree)");
    }

    let (graph, _extractions, mode, changed_count, config) = analyze(&from)?;
    // `--format` always wins; omitted, it sniffs the TTY like `changes`
    // does. `Args` is never the sniffed default; it must be requested.
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
                // lines, nothing else"; a run-all reason isn't one of
                // those, so it goes to stderr instead, same as the
                // selection footer below.
                Format::Args => eprintln!("run all: {reason}"),
            }
            return Ok(2);
        }
    };

    let total_known = count_tests(&graph);
    let seed_count = seeds.len();
    let test_defs = selected_test_defs(&graph, &seeds, &config)?;
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

/// A selected test that matched a `why` query, along with its
/// seed -> test hop path (from `walk::impacted_tests_with_paths`) and the
/// `<file> :: <name chain>` string `test_id` was matched against.
///
/// `always_run_glob` is `Some(<glob>)` exactly when this test was selected
/// *only* via `testless.toml`'s `always-run` list (the impact walk itself
/// never reached it at all, not even as a bare/empty-path seed): in that
/// case `path` is empty and the `always-run` glob is the sole explanation
/// for the selection, rendered distinctly from an ordinary empty-path seed.
struct WhyCandidate {
    file: std::path::PathBuf,
    name: Vec<String>,
    path: Vec<Hop>,
    match_str: String,
    always_run_glob: Option<String>,
}

/// Human-facing verb phrase for a hop's edge kind, used as the prefix of
/// every non-final line in `print_why_text` (e.g. `called by fmt (...)`).
fn hop_kind_label(kind: &HopKind) -> String {
    match kind {
        HopKind::Calls => "called by".to_string(),
        HopKind::Reads => "read by".to_string(),
        HopKind::Contains => "contained in".to_string(),
        HopKind::ImportCloses => "imported by".to_string(),
        HopKind::UnknownName(name) => format!("possibly called by (unresolved name \"{name}\")"),
    }
}

/// Machine-readable label for a hop's edge kind, matching the lowercase
/// snake_case convention `seed_kind_label` already established for the
/// wire format. `UnknownName` carries its matched name, so it serializes
/// as a small object instead of a bare string.
fn hop_kind_json(kind: &HopKind) -> serde_json::Value {
    match kind {
        HopKind::Calls => serde_json::json!("calls"),
        HopKind::Reads => serde_json::json!("reads"),
        HopKind::Contains => serde_json::json!("contains"),
        HopKind::ImportCloses => serde_json::json!("import_closes"),
        HopKind::UnknownName(name) => serde_json::json!({ "unknown_name": name }),
    }
}

/// A def's display name and file path, looked up by `DefId`. Shared by both
/// `why` renderers so a hop's endpoints are always rendered identically.
fn def_display(graph: &Graph, id: DefId) -> (String, std::path::PathBuf) {
    let def = graph.def(id);
    (
        def.name.clone(),
        graph.files[def.file.0 as usize].path.clone(),
    )
}

/// `why`'s human output, one line per hop: the seed (path's first hop's
/// `from`) renders as `changed <name> (<file>)`; every intermediate hop
/// renders as `  <verb> <name> (<file>)` (e.g. `  called by fmt (...)`,
/// `  read by ...`); the final hop (arriving at the matched test itself)
/// always renders as `  = test "<name chain>" (<file>)` regardless of its
/// edge kind, since it's the destination, not another impacted def. A test
/// that's itself a seed (empty path, e.g. a newly `Added` test) has no
/// preceding `changed` line: just the bare `= test ...` line. A test
/// selected only via a `testless.toml` `always-run` glob (also an empty
/// path, but distinguished by `always_run_glob` being `Some`) instead gets a
/// `selected by always-run glob '<glob>'` line, since "no walk path" means
/// something different there: the walk never reached this test by any means
/// at all. Returns lines rather than printing directly so it's unit-testable
/// without capturing stdout; `print_why_text` is the printing wrapper
/// callers actually use.
fn why_text_lines(graph: &Graph, candidate: &WhyCandidate) -> Vec<String> {
    let test_name = candidate.name.join(" > ");
    if let Some(glob) = &candidate.always_run_glob {
        return vec![
            format!("selected by always-run glob '{glob}'"),
            format!("  = test \"{test_name}\" ({})", candidate.file.display()),
        ];
    }
    if candidate.path.is_empty() {
        return vec![format!(
            "= test \"{test_name}\" ({})",
            candidate.file.display()
        )];
    }

    let mut lines = Vec::with_capacity(candidate.path.len() + 1);
    let (seed_name, seed_file) = def_display(graph, candidate.path[0].from);
    lines.push(format!("changed {seed_name} ({})", seed_file.display()));

    let last = candidate.path.len() - 1;
    for (i, hop) in candidate.path.iter().enumerate() {
        if i == last {
            lines.push(format!(
                "  = test \"{test_name}\" ({})",
                candidate.file.display()
            ));
        } else {
            let (name, file) = def_display(graph, hop.to);
            lines.push(format!(
                "  {} {name} ({})",
                hop_kind_label(&hop.edge),
                file.display()
            ));
        }
    }
    lines
}

fn print_why_text(graph: &Graph, candidate: &WhyCandidate) {
    for line in why_text_lines(graph, candidate) {
        println!("{line}");
    }
}

/// `why`'s JSON output for an unambiguous match: `{"version":1,"test":
/// {...},"path":[{"from":{...},"edge":...,"to":{...}}, ...]}`, each hop
/// endpoint rendered as `{"name":..., "file":...}` via `def_display`. A test
/// selected only via an `always-run` glob (see `WhyCandidate::always_run_glob`)
/// carries an empty `path` plus an extra top-level `"always_run"` string
/// field naming the matched glob, instead of the usual hop chain.
fn why_json(graph: &Graph, candidate: &WhyCandidate) -> serde_json::Value {
    let path_json: Vec<_> = candidate
        .path
        .iter()
        .map(|hop| {
            let (from_name, from_file) = def_display(graph, hop.from);
            let (to_name, to_file) = def_display(graph, hop.to);
            serde_json::json!({
                "from": { "name": from_name, "file": from_file },
                "edge": hop_kind_json(&hop.edge),
                "to": { "name": to_name, "file": to_file },
            })
        })
        .collect();

    let mut out = serde_json::json!({
        "version": 1,
        "mode": "explained",
        "test": {
            "file": candidate.file,
            "name": candidate.name,
        },
        "path": path_json,
    });
    if let Some(glob) = &candidate.always_run_glob {
        out["always_run"] = serde_json::json!(glob);
    }
    out
}

/// `testless why <test-id>`: explain why a test was (or wasn't) selected,
/// by running the same analyze+walk pipeline as `select` and printing the
/// seed -> test hop path for whichever selected test matches `test_id`.
///
/// Matching is deliberately forgiving: `test_id` is a substring match
/// against `<file> :: <name chain>` (so a bare test name, a file path, or
/// the full string all work). Zero matches means the test wasn't among
/// this change's selected tests (exit 1); more than one lists every
/// matching candidate rather than guessing (exit 1). Exactly one match
/// prints its path (exit 0). A run-all classification short-circuits with
/// the same reason/exit-code (2) contract as `select`/`changes`: there's no
/// specific walk to explain when everything runs.
fn cmd_why(test_id: String, from: String) -> Result<i32> {
    let (graph, _extractions, mode, _changed_count, config) = analyze(&from)?;
    let is_tty = std::io::stdout().is_terminal();

    let seeds = match mode {
        ChangeMode::Selection(seeds) => seeds,
        ChangeMode::RunAll { reason } => {
            if is_tty {
                println!("run all: {reason}");
            } else {
                let out = serde_json::json!({
                    "version": 1,
                    "mode": "run_all",
                    "reason": reason,
                });
                println!("{out}");
            }
            return Ok(2);
        }
    };

    // The walk's own selection (possibly with an empty path, for a def
    // that's itself a seed) takes precedence over `always-run`: a test is
    // only explained via its `always-run` glob when the walk didn't reach
    // it by any means at all.
    let with_paths: std::collections::HashMap<DefId, Vec<Hop>> =
        impacted_tests_with_paths(&graph, &seeds)
            .into_iter()
            .collect();
    let always_run: std::collections::HashMap<DefId, String> =
        config::always_run_matches(&graph, &config)?
            .into_iter()
            .collect();

    let mut all_ids: std::collections::BTreeSet<DefId> = with_paths.keys().copied().collect();
    all_ids.extend(always_run.keys().copied());

    let candidates: Vec<WhyCandidate> = all_ids
        .into_iter()
        .map(|id| {
            let def = graph.def(id);
            let file = graph.files[def.file.0 as usize].path.clone();
            let name = def
                .test_id
                .clone()
                .unwrap_or_else(|| vec![def.name.clone()]);
            let match_str = format!("{} :: {}", file.display(), name.join(" > "));
            let (path, always_run_glob) = match with_paths.get(&id) {
                Some(path) => (path.clone(), None),
                None => (Vec::new(), always_run.get(&id).cloned()),
            };
            WhyCandidate {
                file,
                name,
                path,
                match_str,
                always_run_glob,
            }
        })
        .collect();

    let matches: Vec<&WhyCandidate> = candidates
        .iter()
        .filter(|c| c.match_str.contains(&test_id))
        .collect();

    match matches.len() {
        0 => {
            if is_tty {
                println!("not selected: no impacted test matches \"{test_id}\"");
            } else {
                let out = serde_json::json!({
                    "version": 1,
                    "mode": "not_selected",
                    "query": test_id,
                });
                println!("{out}");
            }
            Ok(1)
        }
        1 => {
            let candidate = matches[0];
            if is_tty {
                print_why_text(&graph, candidate);
            } else {
                println!("{}", why_json(&graph, candidate));
            }
            Ok(0)
        }
        _ => {
            if is_tty {
                println!("ambiguous match for \"{test_id}\", candidates:");
                for c in &matches {
                    println!("  {}", c.match_str);
                }
            } else {
                let candidates_json: Vec<_> = matches
                    .iter()
                    .map(|c| serde_json::json!({ "file": c.file, "name": c.name }))
                    .collect();
                let out = serde_json::json!({
                    "version": 1,
                    "mode": "ambiguous",
                    "query": test_id,
                    "candidates": candidates_json,
                });
                println!("{out}");
            }
            Ok(1)
        }
    }
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
        Cmd::Why { test_id, from } => cmd_why(test_id, from),
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

#[cfg(test)]
mod why_tests {
    use super::*;
    use std::path::PathBuf;
    use testless_core::graph::{Def, DefKind, FileNode};

    fn file(g: &mut Graph, path: &str) -> testless_core::graph::FileId {
        g.add_file(FileNode {
            path: PathBuf::from(path),
            hash: [0; 32],
            lang: "ts".into(),
        })
    }

    fn def(g: &mut Graph, name: &str, kind: DefKind, file: testless_core::graph::FileId) -> DefId {
        g.add_def(Def {
            name: name.into(),
            kind,
            file,
            start_line: 1,
            end_line: 2,
            test_id: None,
            computed_name: false,
        })
    }

    #[test]
    fn why_text_lines_two_hop_path() {
        // changed add (src/math.ts) -> called by fmt (src/format.ts) ->
        // = test "formats a sum" (src/format.test.ts)
        let mut g = Graph::default();
        let math_file = file(&mut g, "src/math.ts");
        let format_file = file(&mut g, "src/format.ts");
        let test_file = file(&mut g, "src/format.test.ts");
        let add = def(&mut g, "add", DefKind::Function, math_file);
        let fmt = def(&mut g, "fmt", DefKind::Function, format_file);

        let candidate = WhyCandidate {
            file: PathBuf::from("src/format.test.ts"),
            name: vec!["formats a sum".to_string()],
            path: vec![
                Hop {
                    from: add,
                    edge: HopKind::Calls,
                    to: fmt,
                },
                Hop {
                    from: fmt,
                    edge: HopKind::Reads,
                    to: def(&mut g, "formats a sum", DefKind::TestCase, test_file),
                },
            ],
            match_str: "src/format.test.ts :: formats a sum".to_string(),
            always_run_glob: None,
        };

        let lines = why_text_lines(&g, &candidate);
        assert_eq!(
            lines,
            vec![
                "changed add (src/math.ts)".to_string(),
                "  called by fmt (src/format.ts)".to_string(),
                "  = test \"formats a sum\" (src/format.test.ts)".to_string(),
            ]
        );
    }

    #[test]
    fn why_text_lines_empty_path_is_bare_test_line() {
        let mut g = Graph::default();
        let test_file = file(&mut g, "src/a.test.ts");
        def(&mut g, "t", DefKind::TestCase, test_file);

        let candidate = WhyCandidate {
            file: PathBuf::from("src/a.test.ts"),
            name: vec!["t".to_string()],
            path: vec![],
            match_str: "src/a.test.ts :: t".to_string(),
            always_run_glob: None,
        };

        let lines = why_text_lines(&g, &candidate);
        assert_eq!(lines, vec!["= test \"t\" (src/a.test.ts)".to_string()]);
    }

    #[test]
    fn why_text_lines_always_run_glob_overrides_empty_path() {
        let mut g = Graph::default();
        let test_file = file(&mut g, "tests/smoke/login.test.ts");
        def(&mut g, "logs in", DefKind::TestCase, test_file);

        let candidate = WhyCandidate {
            file: PathBuf::from("tests/smoke/login.test.ts"),
            name: vec!["logs in".to_string()],
            path: vec![],
            match_str: "tests/smoke/login.test.ts :: logs in".to_string(),
            always_run_glob: Some("tests/smoke/**".to_string()),
        };

        let lines = why_text_lines(&g, &candidate);
        assert_eq!(
            lines,
            vec![
                "selected by always-run glob 'tests/smoke/**'".to_string(),
                "  = test \"logs in\" (tests/smoke/login.test.ts)".to_string(),
            ]
        );
    }

    #[test]
    fn hop_kind_label_covers_every_variant() {
        assert_eq!(hop_kind_label(&HopKind::Calls), "called by");
        assert_eq!(hop_kind_label(&HopKind::Reads), "read by");
        assert_eq!(hop_kind_label(&HopKind::Contains), "contained in");
        assert_eq!(hop_kind_label(&HopKind::ImportCloses), "imported by");
        assert_eq!(
            hop_kind_label(&HopKind::UnknownName("add".to_string())),
            "possibly called by (unresolved name \"add\")"
        );
    }

    #[test]
    fn hop_kind_json_labels() {
        assert_eq!(hop_kind_json(&HopKind::Calls), serde_json::json!("calls"));
        assert_eq!(hop_kind_json(&HopKind::Reads), serde_json::json!("reads"));
        assert_eq!(
            hop_kind_json(&HopKind::Contains),
            serde_json::json!("contains")
        );
        assert_eq!(
            hop_kind_json(&HopKind::ImportCloses),
            serde_json::json!("import_closes")
        );
        assert_eq!(
            hop_kind_json(&HopKind::UnknownName("add".to_string())),
            serde_json::json!({ "unknown_name": "add" })
        );
    }
}
