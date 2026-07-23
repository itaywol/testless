//! `testless select` CLI e2e coverage (Plan 4, Task 2) — mirrors the
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

/// `--to` isn't supported yet in v1: documented punt, hard error exit 1 —
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

/// `--format args` prints `vitest run <file> -t "<pattern>"` lines on
/// stdout — one per selected TS test — for the same body-edit scenario as
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
            .any(|l| l.contains("math.test.ts") && l.contains("-t \"add > handles negatives\"")),
        "got stdout: {out:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("format.test.ts") && l.contains("-t \"formats\"")),
        "got stdout: {out:?}"
    );
    assert_eq!(
        lines.len(),
        3,
        "expected exactly 3 command lines, got {out:?}"
    );
}
