//! `testless why` CLI e2e coverage (#14): mirrors the git/tempdir harness in
//! `select.rs`, but drives the `why` subcommand end to end: index -> diff ->
//! classify -> walk `impacted_tests_with_paths` -> hop-path output.
//!
//! Fixture (TS, one repo, same shape as `select.rs`'s primary scenario):
//! - `src/math.ts`: `add`, called by `format.ts`'s `fmt` and by
//!   `math.test.ts`'s `describe("add")` tests.
//! - `src/format.ts`: `fmt`, which calls `add`.
//! - `src/math.test.ts`: `describe("add")` with an `it("handles negatives")`.
//! - `src/format.test.ts`: a single `it("formats a sum")` that calls `fmt`.
//!
//! An `add`-body edit selects both, so `why formats` explains the
//! `add -> fmt -> "formats a sum"` path (exit 0, stdout mentions both
//! `add` and `formats`), while `why nonexistent` matches no selected test
//! (exit 1).

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
";

const MATH_TS_BODY_EDITED: &str = "\
export function add(a: number, b: number): number { return a + b + 1; }
";

const FORMAT_TS: &str = "\
import { add } from \"./math\";
export function fmt(a: number, b: number): string { return `${add(a, b)}`; }
";

const MATH_TEST_TS: &str = "\
import { describe, it, expect } from \"vitest\";
import { add } from \"./math\";

describe(\"add\", () => {
  it(\"handles negatives\", () => { expect(add(-1, -2)).toBe(-3); });
});
";

const FORMAT_TEST_TS: &str = "\
import { it, expect } from \"vitest\";
import { fmt } from \"./format\";
it(\"formats a sum\", () => { expect(fmt(1, 2)).toBe(\"3\"); });
";

fn init_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("package.json"), "{ \"name\": \"why-fixture\" }\n").unwrap();
    std::fs::write(root.join("src/math.ts"), MATH_TS).unwrap();
    std::fs::write(root.join("src/format.ts"), FORMAT_TS).unwrap();
    std::fs::write(root.join("src/math.test.ts"), MATH_TEST_TS).unwrap();
    std::fs::write(root.join("src/format.test.ts"), FORMAT_TEST_TS).unwrap();
    git(root, &["init", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "initial"]);
    tmp
}

/// `why formats` (a substring of `"formats a sum"`) explains the impacted
/// path from `add`'s body edit, through `fmt`, to the matching test. Exit
/// 0; stdout is human text (not piped through anything, so it's the `Text`
/// default) and mentions both the changed def and the destination test.
#[test]
fn why_explains_path_to_selected_test() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::write(root.join("src/math.ts"), MATH_TS_BODY_EDITED).unwrap();

    let assert = Command::cargo_bin("testless")
        .unwrap()
        .args(["why", "formats"])
        .current_dir(root)
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(out.contains("add"), "expected the changed def in {out:?}");
    assert!(
        out.contains("formats a sum"),
        "expected the matched test's name in {out:?}"
    );
    assert!(
        out.contains("src/math.ts"),
        "expected the changed def's file in {out:?}"
    );
    assert!(
        out.contains("src/format.test.ts"),
        "expected the matched test's file in {out:?}"
    );
}

/// A query matching no selected test exits 1 and says so, rather than
/// silently printing nothing or crashing.
#[test]
fn why_nonexistent_test_exits_1() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::write(root.join("src/math.ts"), MATH_TS_BODY_EDITED).unwrap();

    Command::cargo_bin("testless")
        .unwrap()
        .args(["why", "nonexistent"])
        .current_dir(root)
        .assert()
        .code(1);
}

/// JSON output (piped stdout) for an unambiguous match: `version`, `mode`,
/// a `test` object naming the matched test, and a non-empty `path` whose
/// hops both mention `add` and `fmt` along the way.
#[test]
fn why_json_output_when_piped() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::write(root.join("src/math.ts"), MATH_TS_BODY_EDITED).unwrap();

    // assert_cmd pipes stdout by default (it's captured, not a TTY), so
    // `why`'s TTY-sniffing already selects JSON here, same convention as
    // `select`/`changes`.
    let assert = Command::cargo_bin("testless")
        .unwrap()
        .args(["why", "formats"])
        .current_dir(root)
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| {
        panic!("expected JSON stdout, got {out:?} ({e})");
    });

    assert_eq!(json["version"], 1);
    assert_eq!(json["mode"], "explained");
    assert_eq!(json["test"]["name"][0], "formats a sum");
    assert_eq!(json["test"]["file"], "src/format.test.ts");

    let path = json["path"].as_array().expect("path array");
    assert!(!path.is_empty(), "expected a non-empty hop path");
    let path_str = format!("{path:?}");
    assert!(
        path_str.contains("add"),
        "expected the changed def somewhere in the path: {path_str}"
    );
    assert!(
        path_str.contains("fmt"),
        "expected the intermediate caller somewhere in the path: {path_str}"
    );
}

/// A config-file edit forces `run_all`; `why` mirrors `select`/`changes`'s
/// exit-2 contract rather than pretending to explain a walk that never ran.
#[test]
fn why_config_file_edit_forces_run_all_exit_2() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::write(root.join("package.json"), "{}\n").unwrap();

    Command::cargo_bin("testless")
        .unwrap()
        .args(["why", "formats"])
        .current_dir(root)
        .assert()
        .code(2);
}
