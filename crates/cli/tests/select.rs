//! `testless select` CLI e2e coverage (Plan 4, Task 2): mirrors the
//! git/tempdir harness in `changes.rs`'s `cli_changes` module, but drives
//! the `select` subcommand end to end: index -> diff -> classify -> walk
//! `impacted_tests` -> wire-format output.
//!
//! Fixture (all TS, one repo):
//! - `src/math.ts`: `add` (called by tests + by `format.ts`'s `fmt`) and an
//!   unrelated `unrelatedHelper`.
//! - `src/format.ts`: `fmt`, which calls `add`.
//! - `src/math.test.ts`: `describe("add")` with two `it`s, plus a separate
//!   `describe("unrelatedHelper")` with one `it`.
//! - `src/format.test.ts`: a single `it("formats")` that calls `fmt`.
//! - `src/unrelated.test.ts`: a single independent `it` touching neither
//!   `add` nor `fmt`.
//!
//! An `add`-body edit must select exactly the two `describe("add")` tests
//! and `format.test.ts`'s `"formats"` (via `fmt` -> `add`), and must NOT
//! select `unrelatedHelper`'s test or `unrelated.test.ts`'s test.
//!
//! Plan 4, Task 4 adds cross-language e2e coverage in the same style, each
//! in its own tempdir/git repo:
//! - Go (`go_*` below): `calc.Add` (called by `TestAdd`'s subtests and,
//!   cross-package, by `fmt2.Fmt`) plus an unrelated `calc.Unrelated` and a
//!   fully independent `third` package.
//! - Rust (`rust_*` below): `math::add` (called by its own `tests` module
//!   and, cross-module, by `fmt::fmt`) plus a fully independent `extra`
//!   module.
//! - Module-init widening (`module_init_*` below, TS): a changed top-level
//!   `console.log` must widen to every test in the transitive importer
//!   closure of the changed file, not just its own tests.

use assert_cmd::Command;

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .expect("failed to spawn git");
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

const MATH_TS: &str = "\
export function add(a: number, b: number): number { return a + b; }
export function unrelatedHelper(x: number): number { return x * 2; }
";

const MATH_TS_BODY_EDITED: &str = "\
export function add(a: number, b: number): number { return a + b + 1; }
export function unrelatedHelper(x: number): number { return x * 2; }
";

const MATH_TS_COMMENT_EDITED: &str = "\
export function add(a: number, b: number): number { /* no-op */ return a + b; }
export function unrelatedHelper(x: number): number { return x * 2; }
";

const FORMAT_TS: &str = "\
import { add } from \"./math\";
export function fmt(a: number, b: number): string { return `${add(a, b)}`; }
";

const MATH_TEST_TS: &str = "\
import { describe, it, expect } from \"vitest\";
import { add, unrelatedHelper } from \"./math\";

describe(\"add\", () => {
  it(\"handles negatives\", () => { expect(add(-1, -2)).toBe(-3); });
  it(\"handles zero\", () => { expect(add(0, 5)).toBe(5); });
});

describe(\"unrelatedHelper\", () => {
  it(\"doubles\", () => { expect(unrelatedHelper(2)).toBe(4); });
});
";

const FORMAT_TEST_TS: &str = "\
import { it, expect } from \"vitest\";
import { fmt } from \"./format\";
it(\"formats\", () => { expect(fmt(1, 2)).toBe(\"3\"); });
";

const UNRELATED_TEST_TS: &str = "\
import { it, expect } from \"vitest\";
it(\"stands alone\", () => { expect(1 + 1).toBe(2); });
";

fn init_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("package.json"),
        "{ \"name\": \"select-fixture\" }\n",
    )
    .unwrap();
    std::fs::write(root.join("src/math.ts"), MATH_TS).unwrap();
    std::fs::write(root.join("src/format.ts"), FORMAT_TS).unwrap();
    std::fs::write(root.join("src/math.test.ts"), MATH_TEST_TS).unwrap();
    std::fs::write(root.join("src/format.test.ts"), FORMAT_TEST_TS).unwrap();
    std::fs::write(root.join("src/unrelated.test.ts"), UNRELATED_TEST_TS).unwrap();
    git(root, &["init", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "initial"]);
    tmp
}

/// (a) A body-only edit of `add` selects both `describe("add")` tests and
/// `format.test.ts`'s `"formats"` test (which reaches `add` via `fmt`),
/// but excludes `unrelatedHelper`'s test and `unrelated.test.ts`'s
/// independent test. Exit 0, JSON on a piped stdout.
#[test]
fn add_body_edit_selects_add_and_formats_tests_excludes_unrelated() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::write(root.join("src/math.ts"), MATH_TS_BODY_EDITED).unwrap();

    let assert = Command::cargo_bin("testless")
        .unwrap()
        .arg("select")
        .current_dir(root)
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| {
        panic!("expected JSON stdout, got {out:?} ({e})");
    });

    assert_eq!(json["version"], 1);
    assert_eq!(json["mode"], "selection");

    let tests = json["tests"].as_array().expect("tests array");

    let names: Vec<Vec<String>> = tests
        .iter()
        .map(|t| {
            t["name"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect()
        })
        .collect();

    assert!(
        names.contains(&vec!["add".to_string(), "handles negatives".to_string()]),
        "expected add/handles negatives in {names:?}"
    );
    assert!(
        names.contains(&vec!["add".to_string(), "handles zero".to_string()]),
        "expected add/handles zero in {names:?}"
    );
    assert!(
        names.contains(&vec!["formats".to_string()]),
        "expected formats in {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.first().map(|s| s.as_str()) == Some("unrelatedHelper")),
        "unrelatedHelper's test must NOT be selected, got {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.first().map(|s| s.as_str()) == Some("stands alone")),
        "unrelated.test.ts's test must NOT be selected, got {names:?}"
    );

    assert_eq!(
        tests.len(),
        3,
        "expected exactly 3 selected tests, got {tests:?}"
    );

    for t in tests {
        assert_eq!(t["runner"], "vitest");
        assert_eq!(t["lang"], "ts");
        assert!(t["file"].as_str().unwrap().ends_with(".test.ts"));
    }

    assert_eq!(json["stats"]["total_known"], 5);
    assert_eq!(json["stats"]["selected"], 3);
    assert_eq!(json["stats"]["seeds"], 1);
    assert_eq!(json["stats"]["changed_files"], 1);
}

/// (b) A comment-only edit (no token change) yields an empty test list,
/// still exit 0.
#[test]
fn comment_only_edit_yields_empty_tests() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::write(root.join("src/math.ts"), MATH_TS_COMMENT_EDITED).unwrap();

    let assert = Command::cargo_bin("testless")
        .unwrap()
        .arg("select")
        .current_dir(root)
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap();

    assert_eq!(json["mode"], "selection");
    assert_eq!(json["tests"].as_array().unwrap().len(), 0);
    assert_eq!(json["stats"]["selected"], 0);
    assert_eq!(json["stats"]["seeds"], 0);
    assert_eq!(json["stats"]["total_known"], 5);
}

/// (c) A `package.json` edit forces `run_all`, exit 2.
#[test]
fn config_file_edit_forces_run_all_exit_2() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::write(root.join("package.json"), "{}\n").unwrap();

    let assert = Command::cargo_bin("testless")
        .unwrap()
        .arg("select")
        .current_dir(root)
        .assert()
        .code(2);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(json["mode"], "run_all");
    assert!(json["reason"].as_str().unwrap().contains("package.json"));
}

/// `--to` isn't supported yet in v1: documented punt, hard error exit 1;
/// mirrors `changes`'s identical rejection.
#[test]
fn to_flag_is_rejected() {
    let tmp = init_repo();
    Command::cargo_bin("testless")
        .unwrap()
        .args(["select", "--to", "HEAD"])
        .current_dir(tmp.path())
        .assert()
        .code(1)
        .stderr(predicates::str::contains("not yet supported"));
}

/// `--format text` prints `file :: seg1 > seg2` lines on stdout and a
/// summary footer on stderr, regardless of TTY.
#[test]
fn text_format_prints_file_and_name_lines() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::write(root.join("src/math.ts"), MATH_TS_BODY_EDITED).unwrap();

    let assert = Command::cargo_bin("testless")
        .unwrap()
        .args(["select", "--format", "text"])
        .current_dir(root)
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("src/math.test.ts :: add > handles negatives"),
        "got stdout: {out:?}"
    );
    assert!(
        out.contains("src/format.test.ts :: formats"),
        "got stdout: {out:?}"
    );
    let err = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(err.contains("3"), "expected a summary footer, got {err:?}");
}

/// `--format args` prints `vitest run <file> -t '<pattern>'` lines on
/// stdout, one per selected TS test, for the same body-edit scenario as
/// the JSON/text tests above (Plan 4 Task 3).
#[test]
fn args_format_prints_vitest_run_lines() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::write(root.join("src/math.ts"), MATH_TS_BODY_EDITED).unwrap();

    let assert = Command::cargo_bin("testless")
        .unwrap()
        .args(["select", "--format", "args"])
        .current_dir(root)
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let lines: Vec<&str> = out.lines().collect();

    assert!(
        lines.iter().all(|l| l.starts_with("vitest run")),
        "expected only `vitest run` lines, got {out:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("math.test.ts") && l.contains("-t 'add > handles negatives'")),
        "got stdout: {out:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("format.test.ts") && l.contains("-t formats")),
        "got stdout: {out:?}"
    );
    assert_eq!(
        lines.len(),
        3,
        "expected exactly 3 command lines, got {out:?}"
    );
}

// ---------------------------------------------------------------------
// Go scenario (Plan 4, Task 4): a body edit to `calc.Add` must select
// `TestAdd`'s two subtests (calc package) and `fmt2`'s `TestFmt` (a
// cross-package caller reached via `calc.Add`), but must NOT select calc's
// own `TestUnrelated` or `third`'s independent `TestSolo`. Caveat from
// prior tasks: `go.mod` is written once at repo init and never touched
// again by these tests (editing it would force `run_all`).
// ---------------------------------------------------------------------

const GO_MOD: &str = "module example.com/go-app\n\ngo 1.22\n";

const CALC_GO: &str = "\
package calc

func Add(a, b int) int { return a + b }

func Unrelated(x int) int { return x * 2 }
";

const CALC_GO_BODY_EDITED: &str = "\
package calc

func Add(a, b int) int { return a + b + 1 }

func Unrelated(x int) int { return x * 2 }
";

const CALC_TEST_GO: &str = "\
package calc

import \"testing\"

func TestAdd(t *testing.T) {
	t.Run(\"negatives\", func(t *testing.T) {
		if Add(-1, -2) != -3 {
			t.Fail()
		}
	})
	t.Run(\"zero\", func(t *testing.T) {
		if Add(0, 5) != 5 {
			t.Fail()
		}
	})
}

func TestUnrelated(t *testing.T) {
	if Unrelated(2) != 4 {
		t.Fail()
	}
}
";

const FMT2_GO: &str = "\
package fmt2

import (
	\"fmt\"

	\"example.com/go-app/calc\"
)

func Fmt(a, b int) string { return fmt.Sprintf(\"%d\", calc.Add(a, b)) }
";

const FMT2_TEST_GO: &str = "\
package fmt2

import \"testing\"

func TestFmt(t *testing.T) {
	if Fmt(1, 2) != \"3\" {
		t.Fail()
	}
}
";

const THIRD_GO: &str = "\
package third

func Solo(x int) int { return x + 1 }
";

const THIRD_TEST_GO: &str = "\
package third

import \"testing\"

func TestSolo(t *testing.T) {
	if Solo(1) != 2 {
		t.Fail()
	}
}
";

fn init_go_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("calc")).unwrap();
    std::fs::create_dir_all(root.join("fmt2")).unwrap();
    std::fs::create_dir_all(root.join("third")).unwrap();
    std::fs::write(root.join("go.mod"), GO_MOD).unwrap();
    std::fs::write(root.join("calc/calc.go"), CALC_GO).unwrap();
    std::fs::write(root.join("calc/calc_test.go"), CALC_TEST_GO).unwrap();
    std::fs::write(root.join("fmt2/fmt2.go"), FMT2_GO).unwrap();
    std::fs::write(root.join("fmt2/fmt2_test.go"), FMT2_TEST_GO).unwrap();
    std::fs::write(root.join("third/third.go"), THIRD_GO).unwrap();
    std::fs::write(root.join("third/third_test.go"), THIRD_TEST_GO).unwrap();
    git(root, &["init", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "initial"]);
    tmp
}

/// Go: `calc.Add`'s body edit selects `TestAdd`'s two subtests and
/// `fmt2`'s `TestFmt` (via `calc.Add`), excludes `TestUnrelated` and
/// `third`'s `TestSolo`. `--format args` emits a `go test` line for the
/// calc package with a `-run '^TestAdd$'`-style anchor.
///
/// `TestAdd` itself (the bare, no-subtest name) is *also* selected: a
/// `t.Run` subtest is genuinely contained within its parent test function
/// (the subtest closure only runs as part of `TestAdd`'s body executing),
/// so the walker's `Contains`-parent widening (see `walk::impacted_tests`)
/// sweeps the parent test in alongside its impacted subtests; unlike the
/// TS `describe`/`it` fixture above, where `describe` isn't itself a
/// `TestCase` def.
#[test]
fn go_add_body_edit_selects_add_and_fmt_tests_excludes_others() {
    let tmp = init_go_repo();
    let root = tmp.path();
    std::fs::write(root.join("calc/calc.go"), CALC_GO_BODY_EDITED).unwrap();

    let assert = Command::cargo_bin("testless")
        .unwrap()
        .arg("select")
        .current_dir(root)
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| {
        panic!("expected JSON stdout, got {out:?} ({e})");
    });

    assert_eq!(json["mode"], "selection");
    let tests = json["tests"].as_array().expect("tests array");
    let names: Vec<Vec<String>> = tests
        .iter()
        .map(|t| {
            t["name"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect()
        })
        .collect();

    assert!(
        names.contains(&vec!["TestAdd".to_string()]),
        "expected bare TestAdd (parent of the impacted subtests) in {names:?}"
    );
    assert!(
        names.contains(&vec!["TestAdd".to_string(), "negatives".to_string()]),
        "expected TestAdd/negatives in {names:?}"
    );
    assert!(
        names.contains(&vec!["TestAdd".to_string(), "zero".to_string()]),
        "expected TestAdd/zero in {names:?}"
    );
    assert!(
        names.contains(&vec!["TestFmt".to_string()]),
        "expected TestFmt in {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.first().map(|s| s.as_str()) == Some("TestUnrelated")),
        "TestUnrelated must NOT be selected, got {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.first().map(|s| s.as_str()) == Some("TestSolo")),
        "third's TestSolo must NOT be selected, got {names:?}"
    );
    assert_eq!(
        tests.len(),
        4,
        "expected exactly 4 selected tests, got {tests:?}"
    );

    for t in tests {
        assert_eq!(t["runner"], "gotest");
        assert_eq!(t["lang"], "go");
    }

    let assert = Command::cargo_bin("testless")
        .unwrap()
        .args(["select", "--format", "args"])
        .current_dir(root)
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        out.lines()
            .any(|l| l.starts_with("go test ./calc") && l.contains("-run '^TestAdd$")),
        "expected a go test line for calc's TestAdd, got {out:?}"
    );
}

// ---------------------------------------------------------------------
// Rust scenario (Plan 4, Task 4): a body edit to `math::add` must select
// `math`'s own test (`tests::add_works`) and `fmt`'s test
// (`tests::fmt_works`, a cross-module caller reached via
// `crate::math::add`), excluding `extra`'s independent test. A
// comment-only edit of `add` yields an empty selection. Caveat: `Cargo.toml`
// is written once at repo init and never touched again (editing it would
// force `run_all`).
// ---------------------------------------------------------------------

const RUST_CARGO_TOML: &str = "\
[package]
name = \"select-rust-fixture\"
version = \"0.1.0\"
edition = \"2021\"

[workspace]
";

const RUST_LIB_RS: &str = "\
mod math;
mod fmt;
mod extra;
";

const RUST_MATH_RS: &str = "\
pub fn add(a: i64, b: i64) -> i64 { a + b }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_works() { assert_eq!(add(2, 2), 4); }
}
";

const RUST_MATH_RS_BODY_EDITED: &str = "\
pub fn add(a: i64, b: i64) -> i64 { a + b + 1 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_works() { assert_eq!(add(2, 2), 4); }
}
";

const RUST_MATH_RS_COMMENT_EDITED: &str = "\
pub fn add(a: i64, b: i64) -> i64 { /* no-op */ a + b }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_works() { assert_eq!(add(2, 2), 4); }
}
";

const RUST_FMT_RS: &str = "\
pub fn fmt(a: i64, b: i64) -> String { format!(\"{}\", crate::math::add(a, b)) }

#[cfg(test)]
mod tests {
    #[test]
    fn fmt_works() { assert_eq!(crate::fmt::fmt(1, 2), \"3\"); }
}
";

const RUST_EXTRA_RS: &str = "\
pub fn extra_fn(x: i64) -> i64 { x + 100 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_works() { assert_eq!(extra_fn(1), 101); }
}
";

fn init_rust_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), RUST_CARGO_TOML).unwrap();
    std::fs::write(root.join("src/lib.rs"), RUST_LIB_RS).unwrap();
    std::fs::write(root.join("src/math.rs"), RUST_MATH_RS).unwrap();
    std::fs::write(root.join("src/fmt.rs"), RUST_FMT_RS).unwrap();
    std::fs::write(root.join("src/extra.rs"), RUST_EXTRA_RS).unwrap();
    git(root, &["init", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "initial"]);
    tmp
}

/// Rust: `math::add`'s body edit selects `tests::add_works` (math's own
/// test) and `tests::fmt_works` (fmt's test, via `crate::math::add`),
/// excludes `extra`'s independent test.
#[test]
fn rust_add_body_edit_selects_math_and_fmt_tests_excludes_extra() {
    let tmp = init_rust_repo();
    let root = tmp.path();
    std::fs::write(root.join("src/math.rs"), RUST_MATH_RS_BODY_EDITED).unwrap();

    let assert = Command::cargo_bin("testless")
        .unwrap()
        .arg("select")
        .current_dir(root)
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| {
        panic!("expected JSON stdout, got {out:?} ({e})");
    });

    assert_eq!(json["mode"], "selection");
    let tests = json["tests"].as_array().expect("tests array");
    let names: Vec<Vec<String>> = tests
        .iter()
        .map(|t| {
            t["name"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect()
        })
        .collect();

    assert!(
        names.contains(&vec!["tests".to_string(), "add_works".to_string()]),
        "expected tests::add_works in {names:?}"
    );
    assert!(
        names.contains(&vec!["tests".to_string(), "fmt_works".to_string()]),
        "expected tests::fmt_works in {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.last().map(|s| s.as_str()) == Some("extra_works")),
        "extra's test must NOT be selected, got {names:?}"
    );
    assert_eq!(
        tests.len(),
        2,
        "expected exactly 2 selected tests, got {tests:?}"
    );

    for t in tests {
        assert_eq!(t["runner"], "cargo");
        assert_eq!(t["lang"], "rust");
    }
}

/// Rust: a comment-only edit of `add`'s body (no token change) yields an
/// empty selection, same invariant as the TS scenario above.
#[test]
fn rust_comment_only_edit_yields_empty_tests() {
    let tmp = init_rust_repo();
    let root = tmp.path();
    std::fs::write(root.join("src/math.rs"), RUST_MATH_RS_COMMENT_EDITED).unwrap();

    let assert = Command::cargo_bin("testless")
        .unwrap()
        .arg("select")
        .current_dir(root)
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap();

    assert_eq!(json["mode"], "selection");
    assert_eq!(json["tests"].as_array().unwrap().len(), 0);
    assert_eq!(json["stats"]["selected"], 0);
    assert_eq!(json["stats"]["seeds"], 0);
}

// ---------------------------------------------------------------------
// Module-init widening scenario (Plan 4, Task 4, TS): a changed top-level
// `console.log` in `src/side.ts` must widen to every test in the
// transitive importer closure of `side.ts`: `side.test.ts` (direct
// importer), `mid.test.ts` (imports `mid.ts`, which imports `side.ts`),
// and `top.test.ts` (imports `top.ts`, which imports `mid.ts`, two hops
// removed from `side.ts`), while `unrelated.test.ts`, which imports
// nothing in the chain, stays excluded.
// ---------------------------------------------------------------------

const SIDE_TS: &str = "\
export const value = 1;
console.log(\"side effect\");
";

const SIDE_TS_EDITED: &str = "\
export const value = 1;
console.log(\"side effect v2\");
";

const MID_TS: &str = "\
import { value } from \"./side\";
export const midValue = value + 1;
";

const TOP_TS: &str = "\
import { midValue } from \"./mid\";
export const topValue = midValue + 1;
";

const SIDE_TEST_TS: &str = "\
import { it, expect } from \"vitest\";
import { value } from \"./side\";
it(\"side value\", () => { expect(value).toBe(1); });
";

const MID_TEST_TS: &str = "\
import { it, expect } from \"vitest\";
import { midValue } from \"./mid\";
it(\"mid value\", () => { expect(midValue).toBe(2); });
";

const TOP_TEST_TS: &str = "\
import { it, expect } from \"vitest\";
import { topValue } from \"./top\";
it(\"top value\", () => { expect(topValue).toBe(3); });
";

const MODULE_INIT_UNRELATED_TEST_TS: &str = "\
import { it, expect } from \"vitest\";
it(\"stands fully alone\", () => { expect(1 + 1).toBe(2); });
";

fn init_module_init_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("package.json"),
        "{ \"name\": \"module-init-fixture\" }\n",
    )
    .unwrap();
    std::fs::write(root.join("src/side.ts"), SIDE_TS).unwrap();
    std::fs::write(root.join("src/mid.ts"), MID_TS).unwrap();
    std::fs::write(root.join("src/top.ts"), TOP_TS).unwrap();
    std::fs::write(root.join("src/side.test.ts"), SIDE_TEST_TS).unwrap();
    std::fs::write(root.join("src/mid.test.ts"), MID_TEST_TS).unwrap();
    std::fs::write(root.join("src/top.test.ts"), TOP_TEST_TS).unwrap();
    std::fs::write(
        root.join("src/unrelated.test.ts"),
        MODULE_INIT_UNRELATED_TEST_TS,
    )
    .unwrap();
    git(root, &["init", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "initial"]);
    tmp
}

/// A top-level (module-init) edit in `side.ts` widens to every test in its
/// transitive importer closure: including `top.test.ts`, two import hops
/// removed, but not `unrelated.test.ts`.
#[test]
fn module_init_edit_selects_all_transitive_importer_tests() {
    let tmp = init_module_init_repo();
    let root = tmp.path();
    std::fs::write(root.join("src/side.ts"), SIDE_TS_EDITED).unwrap();

    let assert = Command::cargo_bin("testless")
        .unwrap()
        .arg("select")
        .current_dir(root)
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| {
        panic!("expected JSON stdout, got {out:?} ({e})");
    });

    assert_eq!(json["mode"], "selection");
    let tests = json["tests"].as_array().expect("tests array");
    let files: Vec<String> = tests
        .iter()
        .map(|t| t["file"].as_str().unwrap().to_string())
        .collect();

    for expected in ["src/side.test.ts", "src/mid.test.ts", "src/top.test.ts"] {
        assert!(
            files.iter().any(|f| f == expected),
            "expected {expected} in selected files {files:?}"
        );
    }
    assert!(
        !files.iter().any(|f| f == "src/unrelated.test.ts"),
        "unrelated.test.ts must NOT be selected, got {files:?}"
    );
    assert_eq!(
        tests.len(),
        3,
        "expected exactly 3 selected tests, got {tests:?}"
    );
}
