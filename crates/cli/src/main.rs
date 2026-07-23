use std::io::IsTerminal;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};

use testless_core::cache::Cache;
use testless_core::graph::{DefKind, Graph};
use testless_core::indexer::index_repo_incremental;
use testless_core::Registry;

#[derive(Parser)]
#[command(
    name = "testless",
    after_help = "Examples:\n  testless index\n  testless stats\n  testless completion zsh > _testless"
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

    if std::io::stdout().is_terminal() {
        println!("Indexed {files} files: {defs} defs ({tests} tests)");
        println!(
            "  parsed: {}  reused: {}  time: {}ms",
            stats.parsed, stats.reused, ms
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
    let cache_bytes = std::fs::metadata(cache.file())
        .map(|m| m.len())
        .unwrap_or(0);

    if std::io::stdout().is_terminal() {
        println!("Cache: {}", cache.root.display());
        println!(
            "files: {files}  defs: {defs}  tests: {tests}  edges: {edges}  size: {cache_bytes} bytes"
        );
    } else {
        let out = serde_json::json!({
            "version": 1,
            "files": files,
            "defs": defs,
            "tests": tests,
            "edges": edges,
            "cache_bytes": cache_bytes,
        });
        println!("{out}");
    }
    Ok(())
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
        Cmd::Index { full } => cmd_index(full),
        Cmd::Stats => cmd_stats(),
        Cmd::Completion { shell } => cmd_completion(shell),
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        for cause in err.chain().skip(1) {
            eprintln!("  caused by: {cause}");
        }
        std::process::exit(1);
    }
}
