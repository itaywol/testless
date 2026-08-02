//! Which test runner a selected test should be handed to, and which build
//! module it lives in.
//!
//! For TS/Go/Rust the runner falls straight out of the language id: one
//! language, one canonical runner. Java breaks that assumption — the same
//! `.java` file is driven by Maven or by Gradle depending on which build
//! file sits above it — so the mapping takes the test's path and the repo
//! root as well, and `testless.toml`'s `java-runner` can override the
//! sniffing outright.
//!
//! Module detection is pure path arithmetic against the standard
//! Maven/Gradle source layout (`<module>/src/{main,test}/java/...`), never a
//! filesystem walk: `select --to <rev>` indexes a throwaway worktree that is
//! already gone by the time commands are rendered, and the commands are
//! meant to run in the user's real working tree anyway. Only the final
//! maven-vs-gradle question touches the disk, and it deliberately touches
//! the *repo* the user will run in.

use std::path::{Path, PathBuf};

/// The build-module directory a Java source file belongs to: the path
/// prefix sitting above a `src/<sourceset>/java` segment triple, as laid
/// out by both Maven and Gradle by convention.
///
/// `services/billing/src/test/java/com/foo/BarTest.java` yields
/// `Some("services/billing")`; a single-module repo's
/// `src/test/java/com/foo/BarTest.java` yields `None` (the module *is* the
/// repo root, so there's nothing to scope a command to). A file that isn't
/// under a conventional source root at all also yields `None` — the
/// non-conventional layouts this misses degrade to repo-root-scoped
/// commands, which still run the test, just with a wider build.
pub fn module_dir(file: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = file.components().collect();
    // Scan for the `src/<sourceset>/java` triple. Searching from the *end*
    // matters: a module legitimately named `src` (or a repo path containing
    // one) would otherwise truncate at the wrong segment.
    let idx = (0..parts.len().saturating_sub(2))
        .rev()
        .find(|&i| parts[i].as_os_str() == "src" && parts[i + 2].as_os_str() == "java")?;
    if idx == 0 {
        return None;
    }
    Some(parts[..idx].iter().collect())
}

/// Whether `dir` (relative to `repo`) holds a Maven or a Gradle build file.
fn build_tool_at(repo: &Path, dir: &Path) -> Option<&'static str> {
    let full = repo.join(dir);
    if full.join("pom.xml").is_file() {
        return Some("maven");
    }
    if full.join("build.gradle").is_file() || full.join("build.gradle.kts").is_file() {
        return Some("gradle");
    }
    None
}

/// The runner label for a def's language, per the `select` wire contract:
/// `ts` -> `vitest`, `go` -> `gotest`, `rust` -> `cargo`, and for `java`
/// either `maven` or `gradle`.
///
/// Java resolution order: an explicit `java-runner` in `testless.toml`
/// wins; otherwise the test's own module directory is sniffed for a build
/// file, then the repo root. A Java repo with neither (or an unrecognized
/// `java-runner` value) degrades to `"unknown"`, exactly like any
/// unregistered language: `select`'s json/text formats still name the test,
/// `--format args` just has no command it can honestly print.
pub fn runner_for(
    lang: &str,
    module: Option<&Path>,
    repo: &Path,
    java_override: Option<&str>,
) -> &'static str {
    match lang {
        "ts" => "vitest",
        "go" => "gotest",
        "rust" => "cargo",
        "java" => {
            match java_override {
                Some("maven") => return "maven",
                Some("gradle") => return "gradle",
                _ => {}
            }
            module
                .and_then(|m| build_tool_at(repo, m))
                .or_else(|| build_tool_at(repo, Path::new("")))
                .unwrap_or("unknown")
        }
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_dir_finds_multi_module_prefix() {
        assert_eq!(
            module_dir(Path::new(
                "services/billing/src/test/java/com/foo/BarTest.java"
            )),
            Some(PathBuf::from("services/billing"))
        );
    }

    #[test]
    fn module_dir_is_none_at_repo_root() {
        assert_eq!(
            module_dir(Path::new("src/test/java/com/foo/BarTest.java")),
            None
        );
        assert_eq!(
            module_dir(Path::new("src/main/java/com/foo/Bar.java")),
            None
        );
    }

    #[test]
    fn module_dir_is_none_for_unconventional_layout() {
        assert_eq!(module_dir(Path::new("java/com/foo/BarTest.java")), None);
        assert_eq!(module_dir(Path::new("BarTest.java")), None);
    }

    #[test]
    fn module_dir_prefers_the_last_src_triple() {
        // A module directory that is itself named `src` must not truncate
        // the prefix at the wrong segment.
        assert_eq!(
            module_dir(Path::new("src/legacy/src/test/java/com/foo/BarTest.java")),
            Some(PathBuf::from("src/legacy"))
        );
    }

    #[test]
    fn non_java_langs_ignore_path_and_repo() {
        let repo = Path::new("/nonexistent");
        assert_eq!(runner_for("ts", None, repo, None), "vitest");
        assert_eq!(runner_for("go", None, repo, None), "gotest");
        assert_eq!(runner_for("rust", None, repo, None), "cargo");
        assert_eq!(runner_for("cobol", None, repo, None), "unknown");
    }

    #[test]
    fn java_override_wins_over_sniffing() {
        let repo = Path::new("/nonexistent");
        assert_eq!(runner_for("java", None, repo, Some("maven")), "maven");
        assert_eq!(runner_for("java", None, repo, Some("gradle")), "gradle");
        // An unrecognized value falls through to sniffing rather than
        // silently pretending to be a runner.
        assert_eq!(runner_for("java", None, repo, Some("bazel")), "unknown");
    }

    #[test]
    fn java_sniffs_module_then_repo_root() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        std::fs::create_dir_all(repo.join("services/billing")).unwrap();
        std::fs::write(repo.join("build.gradle"), "").unwrap();

        // Nothing in the module yet: falls back to the root's gradle build.
        let module = Path::new("services/billing");
        assert_eq!(runner_for("java", Some(module), repo, None), "gradle");

        // A module-local pom.xml wins over the root.
        std::fs::write(repo.join("services/billing/pom.xml"), "").unwrap();
        assert_eq!(runner_for("java", Some(module), repo, None), "maven");
    }

    #[test]
    fn java_with_no_build_file_is_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(runner_for("java", None, tmp.path(), None), "unknown");
    }
}
