//! `testless.toml`: an optional, repo-root escape-hatch config for two knobs
//! the spec calls out as needing manual override rather than static
//! inference:
//!
//! ```toml
//! always-run = ["tests/smoke/**", "**/*.e2e.test.ts"]
//! ignore = ["**/generated/**", "*.pb.go"]
//! java-runner = "gradle"
//! ```
//!
//! - `ignore`: discovery-level. Matched (via `globset`) against repo-relative
//!   paths; a matching file is dropped before indexing ever sees it, so
//!   nothing it defines is a candidate for anything (selection, `why`,
//!   `total_known`).
//! - `always-run`: selection-level. After the impact walk, every `TestCase`
//!   def whose *file* matches one of these globs is added to the selection
//!   regardless of whether the walk reached it; see
//!   [`always_run_matches`].
//! - `java-runner`: rendering-level, and Java-only. `"maven"` or
//!   `"gradle"`; overrides the build-file sniffing `--format args` does to
//!   decide which command shape to print (see the CLI's `runner` module).
//!
//! A missing `testless.toml` is not an error: [`Config::load`] returns
//! [`Config::default`] (both lists empty, i.e. a no-op). A `testless.toml`
//! that exists but fails to parse (bad TOML syntax, wrong value types) *is*
//! an error: the file is something the user deliberately wrote, so a broken
//! one deserves a loud failure rather than a silent `run_all` degrade (that
//! degrade is reserved for transient/environmental failures like a missing
//! `git`, not user-authored config).

use std::path::Path;

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

use crate::graph::{DefId, DefKind, Graph};

/// Parsed `testless.toml`. Both fields default to empty (a no-op config),
/// so a config with only one of the two keys set is valid.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    #[serde(default)]
    pub always_run: Vec<String>,
    #[serde(default)]
    pub ignore: Vec<String>,
    /// `"maven"` or `"gradle"`: forces which build tool `--format args`
    /// renders Java test commands for, instead of sniffing for a `pom.xml`
    /// / `build.gradle` next to the test's module. Unset (the default)
    /// means sniff. Deliberately *not* validated at parse time: an
    /// unrecognized value degrades that runner to `"unknown"` (no command
    /// printed) rather than failing the whole run, matching how an
    /// unregistered language behaves.
    #[serde(default)]
    pub java_runner: Option<String>,
}

impl Config {
    /// Loads `{repo}/testless.toml`. A missing file yields
    /// `Ok(Config::default())`; any other read failure or a parse error
    /// (bad TOML, wrong value shapes) yields `Err` with the file path in
    /// context, so callers should treat this as a hard failure, not a
    /// `run_all` degrade.
    pub fn load(repo: &Path) -> Result<Config> {
        let path = repo.join("testless.toml");
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        toml::from_str(&src).with_context(|| format!("parsing {}", path.display()))
    }

    /// Compiles `ignore` into a `GlobSet` for discovery-time filtering.
    /// `Err` on any invalid glob pattern (also a "malformed config" failure).
    pub fn ignore_globset(&self) -> Result<GlobSet> {
        build_globset(&self.ignore)
    }

    /// Compiles `always_run` into a `GlobSet`. Exposed mainly so callers can
    /// eagerly validate the config (fail fast on a bad pattern) even before
    /// [`always_run_matches`] is needed.
    pub fn always_run_globset(&self) -> Result<GlobSet> {
        build_globset(&self.always_run)
    }
}

fn build_globset(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern)
            .with_context(|| format!("invalid glob pattern in testless.toml: {pattern:?}"))?;
        builder.add(glob);
    }
    builder
        .build()
        .context("building globset from testless.toml")
}

/// Every `TestCase` def in `graph` whose file matches one of `config`'s
/// `always-run` globs, paired with the glob pattern (as written in
/// `testless.toml`) that matched it. Selection-level: called after the
/// impact walk, so its results get unioned into the walk's own selection
/// rather than replacing it. When a file matches more than one pattern, the
/// first matching pattern (in `always_run`'s declared order) is reported.
pub fn always_run_matches(graph: &Graph, config: &Config) -> Result<Vec<(DefId, String)>> {
    if config.always_run.is_empty() {
        return Ok(Vec::new());
    }
    let set = config.always_run_globset()?;

    let mut out = Vec::new();
    for (idx, def) in graph.defs.iter().enumerate() {
        if def.kind != DefKind::TestCase {
            continue;
        }
        let path = &graph.files[def.file.0 as usize].path;
        if let Some(&first) = set.matches(path).first() {
            out.push((DefId(idx as u32), config.always_run[first].clone()));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Def, FileNode};

    #[test]
    fn missing_file_yields_default() {
        let tmp = tempfile::tempdir().unwrap();
        let config = Config::load(tmp.path()).unwrap();
        assert_eq!(config, Config::default());
        assert!(config.always_run.is_empty());
        assert!(config.ignore.is_empty());
    }

    #[test]
    fn valid_file_parses_both_lists() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("testless.toml"),
            "always-run = [\"tests/smoke/**\", \"**/*.e2e.test.ts\"]\n\
             ignore = [\"**/generated/**\", \"*.pb.go\"]\n",
        )
        .unwrap();

        let config = Config::load(tmp.path()).unwrap();
        assert_eq!(
            config.always_run,
            vec!["tests/smoke/**".to_string(), "**/*.e2e.test.ts".to_string()]
        );
        assert_eq!(
            config.ignore,
            vec!["**/generated/**".to_string(), "*.pb.go".to_string()]
        );
    }

    #[test]
    fn valid_file_with_only_one_key_defaults_the_other() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("testless.toml"), "ignore = [\"*.pb.go\"]\n").unwrap();

        let config = Config::load(tmp.path()).unwrap();
        assert!(config.always_run.is_empty());
        assert_eq!(config.ignore, vec!["*.pb.go".to_string()]);
    }

    #[test]
    fn malformed_toml_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("testless.toml"),
            "always-run = [not valid toml\n",
        )
        .unwrap();

        let err = Config::load(tmp.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("testless.toml"),
            "error should mention testless.toml: {err:#}"
        );
    }

    #[test]
    fn wrong_value_type_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("testless.toml"), "always-run = \"nope\"\n").unwrap();

        assert!(Config::load(tmp.path()).is_err());
    }

    fn file(g: &mut Graph, path: &str) -> crate::graph::FileId {
        g.add_file(FileNode {
            path: path.into(),
            hash: [0; 32],
            lang: "ts".into(),
        })
    }

    fn def(g: &mut Graph, name: &str, kind: DefKind, file: crate::graph::FileId) -> DefId {
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
    fn always_run_matches_selects_test_files_by_glob() {
        let mut g = Graph::default();
        let smoke_file = file(&mut g, "tests/smoke/login.test.ts");
        let other_file = file(&mut g, "src/math.test.ts");
        let smoke_test = def(&mut g, "logs in", DefKind::TestCase, smoke_file);
        let _other_test = def(&mut g, "adds", DefKind::TestCase, other_file);

        let config = Config {
            always_run: vec!["tests/smoke/**".to_string()],
            ..Config::default()
        };

        let matches = always_run_matches(&g, &config).unwrap();
        assert_eq!(matches, vec![(smoke_test, "tests/smoke/**".to_string())]);
    }

    #[test]
    fn empty_always_run_matches_nothing() {
        let mut g = Graph::default();
        let f = file(&mut g, "src/math.test.ts");
        def(&mut g, "adds", DefKind::TestCase, f);

        let config = Config::default();
        assert_eq!(always_run_matches(&g, &config).unwrap(), Vec::new());
    }
}
