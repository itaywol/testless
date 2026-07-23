//! `--format args`: render selected tests as ready-to-run test-runner
//! command lines (vitest / `go test` / `cargo test`). Pure string
//! generation — this module never spawns a process, it only builds the
//! strings a human or CI job would paste into a shell.
//!
//! One line per selected test (v1 keeps it simple: no multi-test `-t`
//! grouping), deduplicated (a `computed` entry drops its exactness flag,
//! so two computed tests in the same file/package collapse to one
//! whole-file invocation) and returned in deterministic (lexicographic)
//! order regardless of input order — callers can rely on stable diffs.

use crate::SelectedTest;

/// Regex metacharacters that need a `\` escape inside a `go test -run`
/// pattern segment, so a literal test name like `TestAdd$weird` doesn't
/// get misread as an anchor.
fn escape_go_regex(segment: &str) -> String {
    let mut escaped = String::with_capacity(segment.len());
    for ch in segment.chars() {
        if matches!(
            ch,
            '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// `vitest run <file> -t "<name joined with ' > '>"`. `computed` widens to
/// the whole file: a truncated (template-literal) test name can't be
/// matched exactly, so `-t` is dropped rather than printing a pattern that
/// would under-select.
fn vitest_line(t: &SelectedTest) -> String {
    let file = t.file.display();
    if t.computed {
        format!("vitest run {file}")
    } else {
        let pattern = t.name.join(" > ");
        format!("vitest run {file} -t \"{pattern}\"")
    }
}

/// `go test ./<dir> -run '^Root$/^sub$'`. `<dir>` is the parent directory
/// of the `_test.go` file (Go packages are directories, not files); a
/// synthetic `<computed>` segment (see `lang-go`'s `handle_run_call`)
/// widens to `.*` instead of an exact anchored literal.
fn gotest_line(t: &SelectedTest) -> String {
    let dir = t.file.parent().unwrap_or_else(|| std::path::Path::new(""));
    let pkg = if dir.as_os_str().is_empty() {
        ".".to_string()
    } else {
        format!("./{}", dir.display())
    };
    let pattern = t
        .name
        .iter()
        .map(|seg| {
            if seg == "<computed>" {
                ".*".to_string()
            } else {
                format!("^{}$", escape_go_regex(seg))
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    format!("go test {pkg} -run '{pattern}'")
}

/// `cargo test '<chain joined with ::>' -- --exact`. `computed` (rare —
/// Rust test paths are almost always static module chains) drops
/// `--exact`, matching by substring instead of exact path.
fn cargo_line(t: &SelectedTest) -> String {
    let chain = t.name.join("::");
    if t.computed {
        format!("cargo test '{chain}'")
    } else {
        format!("cargo test '{chain}' -- --exact")
    }
}

/// Render `tests` as one runner-consumable command line each, deduplicated
/// and sorted for a deterministic, script-friendly stdout stream. A
/// `runner` this module doesn't recognize (only `"unknown"` today — see
/// `runner_for_lang`) is silently skipped: there's no sensible command to
/// print for it, and `select`'s `json`/`text` formats already surface the
/// `"unknown"` label for inspection.
pub fn command_lines(tests: &[SelectedTest]) -> Vec<String> {
    let mut lines: Vec<String> = tests
        .iter()
        .filter_map(|t| match t.runner {
            "vitest" => Some(vitest_line(t)),
            "gotest" => Some(gotest_line(t)),
            "cargo" => Some(cargo_line(t)),
            _ => None,
        })
        .collect();
    lines.sort();
    lines.dedup();
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test(file: &str, name: &[&str], runner: &'static str, computed: bool) -> SelectedTest {
        SelectedTest {
            file: PathBuf::from(file),
            name: name.iter().map(|s| s.to_string()).collect(),
            runner,
            lang: "irrelevant".to_string(),
            computed,
        }
    }

    #[test]
    fn vitest_simple() {
        let t = test(
            "src/math.test.ts",
            &["add", "handles negatives"],
            "vitest",
            false,
        );
        assert_eq!(
            command_lines(&[t]),
            vec!["vitest run src/math.test.ts -t \"add > handles negatives\""]
        );
    }

    #[test]
    fn vitest_single_segment() {
        let t = test("src/format.test.ts", &["formats"], "vitest", false);
        assert_eq!(
            command_lines(&[t]),
            vec!["vitest run src/format.test.ts -t \"formats\""]
        );
    }

    #[test]
    fn vitest_computed_drops_dash_t() {
        let t = test("src/math.test.ts", &["add"], "vitest", true);
        assert_eq!(command_lines(&[t]), vec!["vitest run src/math.test.ts"]);
    }

    #[test]
    fn gotest_simple() {
        let t = test(
            "pkg/calc/add_test.go",
            &["TestAdd", "negatives"],
            "gotest",
            false,
        );
        assert_eq!(
            command_lines(&[t]),
            vec!["go test ./pkg/calc -run '^TestAdd$/^negatives$'"]
        );
    }

    #[test]
    fn gotest_root_level_file_has_no_dir() {
        let t = test("add_test.go", &["TestAdd"], "gotest", false);
        assert_eq!(command_lines(&[t]), vec!["go test . -run '^TestAdd$'"]);
    }

    #[test]
    fn gotest_computed_segment_widens_to_dot_star() {
        let t = test(
            "pkg/calc/add_test.go",
            &["TestAdd", "<computed>"],
            "gotest",
            true,
        );
        assert_eq!(
            command_lines(&[t]),
            vec!["go test ./pkg/calc -run '^TestAdd$/.*'"]
        );
    }

    #[test]
    fn gotest_escapes_regex_metachars() {
        let t = test("pkg/calc/add_test.go", &["TestAdd$weird"], "gotest", false);
        assert_eq!(
            command_lines(&[t]),
            vec!["go test ./pkg/calc -run '^TestAdd\\$weird$'"]
        );
    }

    #[test]
    fn cargo_simple() {
        let t = test("src/lib.rs", &["math", "add_works"], "cargo", false);
        assert_eq!(
            command_lines(&[t]),
            vec!["cargo test 'math::add_works' -- --exact"]
        );
    }

    #[test]
    fn cargo_computed_drops_exact() {
        let t = test("src/lib.rs", &["math", "add_works"], "cargo", true);
        assert_eq!(command_lines(&[t]), vec!["cargo test 'math::add_works'"]);
    }

    #[test]
    fn unknown_runner_is_skipped() {
        let t = test("weird.file", &["whatever"], "unknown", false);
        assert!(command_lines(&[t]).is_empty());
    }

    #[test]
    fn dedup_and_deterministic_order() {
        let a = test("src/b.test.ts", &["z"], "vitest", true);
        let b = test("src/a.test.ts", &["y"], "vitest", true);
        // Two distinct computed entries collapse to distinct whole-file
        // lines (different files), but a duplicate input is deduped.
        let c = test("src/a.test.ts", &["y"], "vitest", true);
        let lines = command_lines(&[a, b, c]);
        assert_eq!(
            lines,
            vec!["vitest run src/a.test.ts", "vitest run src/b.test.ts"]
        );
    }
}
