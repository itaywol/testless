//! Def-level diff invariance tests (Plan 3, Task 2), plus (Task 4) the
//! change-classifier's rule table exercised end-to-end against real
//! tree-sitter-parsed TS fixtures: `classify` needs a real `Language` to
//! diff old/new content, so that coverage lives here rather than in
//! `testless-core` (which has no language crate to link against).

use std::path::{Path, PathBuf};

use testless_core::classify::{classify, ChangeMode, Seed, SeedKind};
use testless_core::gitio::{ChangedFile, FileStatus};
use testless_core::indexer::index_repo_incremental;
use testless_core::{diff_defs, DefId, DefKind, Extraction, Graph, Language, Registry};
use testless_lang_ts::TsLanguage;

fn extract_ts(src: &str) -> Extraction {
    let lang = TsLanguage;
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&lang.grammar(std::path::Path::new("math.ts")))
        .unwrap();
    let tree = parser.parse(src, None).unwrap();
    lang.extract(src, &tree)
}

/// `math.ts` reformatted and commented throughout (line breaks moved,
/// indentation changed, a comment added before/inside/after every def and at
/// the top level) but with exactly the same tokens as the original fixture
/// file, with no identifier, literal, or keyword changed. `diff_defs` on the two
/// extractions must report zero changes: this is the whole point of
/// structural (comment/formatting-insensitive) fingerprinting from Task 1.
const MATH_TS_REFORMATTED: &str = r#"
// header comment
export function add(
  a: number,
  b: number
): number {
  // adds two numbers
  return a + b;
}

export const mul = (a: number, b: number): number =>
  a * b; // multiply

export class Calc {
  total = 0;

  push(n: number) {
    /* accumulate */
    this.total = add(this.total, n);
  }
}

console.log("side effect at import"); // side effect
"#;

#[test]
fn comment_and_formatting_changes_yield_no_def_changes() {
    let original =
        std::fs::read_to_string("../../fixtures/ts-app/src/math.ts").expect("read fixture");

    let old = extract_ts(&original);
    let new = extract_ts(MATH_TS_REFORMATTED);

    let changes = diff_defs(&old, &new);
    assert_eq!(
        changes,
        vec![],
        "comment/formatting-only diff must be empty"
    );
}

// --- classify() integration coverage (Plan 3, Task 4) -----------------

fn registry() -> Registry {
    Registry::new(vec![Box::new(TsLanguage)])
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn find_def(g: &Graph, name: &str) -> DefId {
    DefId(
        g.defs
            .iter()
            .position(|d| d.name == name)
            .unwrap_or_else(|| panic!("no def named {name:?}")) as u32,
    )
}

fn no_old_content(_: &Path) -> anyhow::Result<Option<String>> {
    Ok(None)
}

const MATH_ADD_ORIGINAL: &str =
    "export function add(a: number, b: number): number { return a + b; }\n";

/// (a) A pure body edit of `add` (same signature, different implementation)
/// must seed exactly `add`'s def with `SeedKind::Body`; nothing else.
/// `export function add() {}` is wrapped in `export_statement`, but
/// `module_init_skip` looks at the `declaration` field child and still skips
/// it (a def-shaped export), so `<module>` stays clean: precise
/// function-level selection is preserved even for exported defs (the
/// content-aware refinement of Item 1's `module_init_skip`).
#[test]
fn body_edit_seeds_exactly_that_defs_body() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "src/math.ts",
        "export function add(a: number, b: number): number { return a + b + 1; }\n",
    );

    let registry = registry();
    let (graph, extractions, _) = index_repo_incremental(root, &registry, None, None).unwrap();
    let add = find_def(&graph, "add");

    let changed = vec![ChangedFile {
        path: PathBuf::from("src/math.ts"),
        status: FileStatus::Modified,
    }];
    let old_src_of = |p: &Path| -> anyhow::Result<Option<String>> {
        if p == Path::new("src/math.ts") {
            Ok(Some(MATH_ADD_ORIGINAL.to_string()))
        } else {
            Ok(None)
        }
    };

    let mode = classify(root, &graph, &registry, &changed, &extractions, &old_src_of);
    assert_eq!(
        mode,
        ChangeMode::Selection(vec![Seed {
            def: add,
            kind: SeedKind::Body,
        }])
    );
}

/// Content-aware `module_init_skip` refinement: an exported arrow-const's
/// body edit (`export const mul = (a, b) => a * b` -> `a + b`) seeds only
/// `mul`'s own def change (`SigChanged`, per the existing accepted
/// limitation that an arrow/function-expression const's span has no `body`
/// field of its own; see `push_def`'s doc comment) and must NOT also seed
/// `<module>`: the whole `lexical_declaration` has a single, arrow-valued
/// declarator, so `lexical_all_fn_valued` skips it from the module hash.
#[test]
fn exported_arrow_const_body_edit_does_not_seed_module_init() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "src/math.ts",
        "export const mul = (a: number, b: number): number => a + b;\n",
    );

    let registry = registry();
    let (graph, extractions, _) = index_repo_incremental(root, &registry, None, None).unwrap();
    let mul = find_def(&graph, "mul");

    let changed = vec![ChangedFile {
        path: PathBuf::from("src/math.ts"),
        status: FileStatus::Modified,
    }];
    let old_src_of = |p: &Path| -> anyhow::Result<Option<String>> {
        if p == Path::new("src/math.ts") {
            Ok(Some(
                "export const mul = (a: number, b: number): number => a * b;\n".to_string(),
            ))
        } else {
            Ok(None)
        }
    };

    let mode = classify(root, &graph, &registry, &changed, &extractions, &old_src_of);
    match mode {
        ChangeMode::Selection(seeds) => {
            assert!(
                seeds.iter().any(|s| s.def == mul),
                "expected a seed for mul's own def, got {seeds:?}"
            );
            assert!(
                !seeds.iter().any(|s| s.kind == SeedKind::ModuleInit),
                "expected no ModuleInit seed, got {seeds:?}"
            );
        }
        other => panic!("expected Selection, got {other:?}"),
    }
}

/// Item 1 new coverage: a top-level `export const` whose value isn't an
/// arrow/function (so it's never captured as its own def) still surfaces a
/// change, as a `ModuleInit` seed, now that `lexical_declaration`/
/// `export_statement` are no longer excluded from the module-init hash. Before
/// the fix this value edit was hashed nowhere and produced zero seeds.
#[test]
fn export_const_value_edit_seeds_module_init() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "src/config.ts",
        "export const CONFIG = makeConfig(1);\n",
    );

    let registry = registry();
    let (graph, extractions, _) = index_repo_incremental(root, &registry, None, None).unwrap();

    let changed = vec![ChangedFile {
        path: PathBuf::from("src/config.ts"),
        status: FileStatus::Modified,
    }];
    let old_src_of = |p: &Path| -> anyhow::Result<Option<String>> {
        if p == Path::new("src/config.ts") {
            Ok(Some("export const CONFIG = makeConfig(2);\n".to_string()))
        } else {
            Ok(None)
        }
    };

    let mode = classify(root, &graph, &registry, &changed, &extractions, &old_src_of);
    match mode {
        ChangeMode::Selection(seeds) => {
            assert!(
                !seeds.is_empty(),
                "expected a non-empty selection (module_init seed), got {seeds:?}"
            );
            assert!(
                seeds.iter().all(|s| s.kind == SeedKind::ModuleInit),
                "expected only ModuleInit seeds, got {seeds:?}"
            );
        }
        other => panic!("expected Selection, got {other:?}"),
    }
}

/// (b) An edit that only adds a comment (no token change) must classify as
/// an empty selection: zero seeds is a valid outcome for a nonempty
/// `changed` list.
#[test]
fn comment_only_edit_yields_empty_selection() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "src/math.ts",
        "export function add(a: number, b: number): number { /* no-op */ return a + b; }\n",
    );

    let registry = registry();
    let (graph, extractions, _) = index_repo_incremental(root, &registry, None, None).unwrap();

    let changed = vec![ChangedFile {
        path: PathBuf::from("src/math.ts"),
        status: FileStatus::Modified,
    }];
    let old_src_of = |p: &Path| -> anyhow::Result<Option<String>> {
        if p == Path::new("src/math.ts") {
            Ok(Some(MATH_ADD_ORIGINAL.to_string()))
        } else {
            Ok(None)
        }
    };

    let mode = classify(root, &graph, &registry, &changed, &extractions, &old_src_of);
    assert_eq!(mode, ChangeMode::Selection(vec![]));
}

/// (c) A `package.json` edit forces `RunAll`, no matter what else changed.
#[test]
fn config_file_change_forces_run_all() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "package.json", "{}\n");
    write(root, "src/math.ts", MATH_ADD_ORIGINAL);

    let registry = registry();
    let (graph, extractions, _) = index_repo_incremental(root, &registry, None, None).unwrap();

    let changed = vec![ChangedFile {
        path: PathBuf::from("package.json"),
        status: FileStatus::Modified,
    }];

    let mode = classify(
        root,
        &graph,
        &registry,
        &changed,
        &extractions,
        &no_old_content,
    );
    assert!(
        matches!(mode, ChangeMode::RunAll { .. }),
        "expected RunAll, got {mode:?}"
    );
}

/// (d) A newly-added test file seeds its `TestCase` def(s) and its
/// `ModuleInit`, both `Added`.
#[test]
fn added_test_file_seeds_test_cases_and_module_init_as_added() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "src/new.test.ts",
        "import { it } from \"vitest\";\nit(\"works\", () => {});\n",
    );

    let registry = registry();
    let (graph, extractions, _) = index_repo_incremental(root, &registry, None, None).unwrap();
    let file_id = testless_core::FileId(
        graph
            .files
            .iter()
            .position(|f| f.path.ends_with("new.test.ts"))
            .unwrap() as u32,
    );
    let module_init = graph.module_init(file_id).expect("module_init present");
    let test_case = graph
        .defs
        .iter()
        .position(|d| d.kind == DefKind::TestCase)
        .map(|i| DefId(i as u32))
        .expect("a TestCase def exists");

    let changed = vec![ChangedFile {
        path: PathBuf::from("src/new.test.ts"),
        status: FileStatus::Added,
    }];

    let mode = classify(
        root,
        &graph,
        &registry,
        &changed,
        &extractions,
        &no_old_content,
    );
    match mode {
        ChangeMode::Selection(mut seeds) => {
            seeds.sort_by_key(|s| s.def);
            let mut expected = vec![
                Seed {
                    def: test_case,
                    kind: SeedKind::Added,
                },
                Seed {
                    def: module_init,
                    kind: SeedKind::Added,
                },
            ];
            expected.sort_by_key(|s| s.def);
            assert_eq!(seeds, expected);
        }
        other => panic!("expected Selection, got {other:?}"),
    }
}

/// (e) A deleted, previously-indexed source file that still has a live
/// importer (its former importer's raw import text still references it,
/// e.g. `import { helper } from "./removed"`) seeds that importer's
/// `ModuleInit`, not `RunAll`: the importer will fail to resolve/compile
/// against the missing module, so its tests need to run, but nothing
/// unrelated does (issue #13).
#[test]
fn deleted_source_file_with_importer_seeds_importer_module_init() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // src/removed.ts existed before this change and has since been deleted;
    // only its former importer remains on disk for the new-tree index.
    write(
        root,
        "src/consumer.ts",
        "import { helper } from \"./removed\";\nexport function useHelper(): unknown { return helper(); }\n",
    );

    let registry = registry();
    let (graph, extractions, _) = index_repo_incremental(root, &registry, None, None).unwrap();
    let consumer_id = testless_core::FileId(
        graph
            .files
            .iter()
            .position(|f| f.path.ends_with("consumer.ts"))
            .unwrap() as u32,
    );
    let consumer_module_init = graph.module_init(consumer_id).expect("module_init present");

    let changed = vec![ChangedFile {
        path: PathBuf::from("src/removed.ts"),
        status: FileStatus::Deleted,
    }];

    let mode = classify(
        root,
        &graph,
        &registry,
        &changed,
        &extractions,
        &no_old_content,
    );
    assert_eq!(
        mode,
        ChangeMode::Selection(vec![Seed {
            def: consumer_module_init,
            kind: SeedKind::ModuleInit,
        }])
    );
}

/// (e2) A deleted, previously-indexed source file with no remaining
/// importer contributes zero seeds: its own tests died along with it, and
/// nothing else referenced it (issue #13).
#[test]
fn deleted_source_file_with_no_importers_yields_empty_selection() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/math.ts", MATH_ADD_ORIGINAL);

    let registry = registry();
    let (graph, extractions, _) = index_repo_incremental(root, &registry, None, None).unwrap();

    let changed = vec![ChangedFile {
        path: PathBuf::from("src/removed.ts"),
        status: FileStatus::Deleted,
    }];

    let mode = classify(
        root,
        &graph,
        &registry,
        &changed,
        &extractions,
        &no_old_content,
    );
    assert_eq!(mode, ChangeMode::Selection(vec![]));
}

/// (f) An unindexed file (no registered `Language`) with no importer
/// referencing it contributes zero seeds, e.g. a README edit.
#[test]
fn unindexed_file_with_no_importers_yields_empty_selection() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/math.ts", MATH_ADD_ORIGINAL);
    write(root, "README.md", "# hello\n");

    let registry = registry();
    let (graph, extractions, _) = index_repo_incremental(root, &registry, None, None).unwrap();

    let changed = vec![ChangedFile {
        path: PathBuf::from("README.md"),
        status: FileStatus::Modified,
    }];

    let mode = classify(
        root,
        &graph,
        &registry,
        &changed,
        &extractions,
        &no_old_content,
    );
    assert_eq!(mode, ChangeMode::Selection(vec![]));
}

/// (g) A changed `data.json` that's referenced only via raw (unresolved)
/// import text, `import data from "./data.json"`, seeds the importer's
/// `ModuleInit`, since the import can't be an `Edge::Imports` (json isn't an
/// indexed language) but the reference is real.
#[test]
fn unresolved_json_import_seeds_importers_module_init() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/data.json", "{\"a\":1}\n");
    write(
        root,
        "src/consumer.ts",
        "import data from \"./data.json\";\nexport function useData(): unknown { return data; }\n",
    );

    let registry = registry();
    let (graph, extractions, _) = index_repo_incremental(root, &registry, None, None).unwrap();
    let consumer_id = testless_core::FileId(
        graph
            .files
            .iter()
            .position(|f| f.path.ends_with("consumer.ts"))
            .unwrap() as u32,
    );
    let consumer_module_init = graph.module_init(consumer_id).expect("module_init present");

    let changed = vec![ChangedFile {
        path: PathBuf::from("src/data.json"),
        status: FileStatus::Modified,
    }];

    let mode = classify(
        root,
        &graph,
        &registry,
        &changed,
        &extractions,
        &no_old_content,
    );
    assert_eq!(
        mode,
        ChangeMode::Selection(vec![Seed {
            def: consumer_module_init,
            kind: SeedKind::ModuleInit,
        }])
    );
}

/// (h) Item 3: `scan_importers` also matches on the changed file's basename
/// *without* extension: `import cfg from "./config"` (an extensionless
/// specifier) must still match a changed `config.json`, since the raw import
/// text never contains the `.json` suffix.
#[test]
fn extensionless_import_matches_changed_file_by_stem() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/config.json", "{\"a\":1}\n");
    write(
        root,
        "src/consumer.ts",
        "import cfg from \"./config\";\nexport function useCfg(): unknown { return cfg; }\n",
    );

    let registry = registry();
    let (graph, extractions, _) = index_repo_incremental(root, &registry, None, None).unwrap();
    let consumer_id = testless_core::FileId(
        graph
            .files
            .iter()
            .position(|f| f.path.ends_with("consumer.ts"))
            .unwrap() as u32,
    );
    let consumer_module_init = graph.module_init(consumer_id).expect("module_init present");

    let changed = vec![ChangedFile {
        path: PathBuf::from("src/config.json"),
        status: FileStatus::Modified,
    }];

    let mode = classify(
        root,
        &graph,
        &registry,
        &changed,
        &extractions,
        &no_old_content,
    );
    assert_eq!(
        mode,
        ChangeMode::Selection(vec![Seed {
            def: consumer_module_init,
            kind: SeedKind::ModuleInit,
        }])
    );
}

/// (i) Go's imports name package *directories*, never file stems, and
/// same-package sibling files reference each other via nothing at all (no
/// import statement). Deleting `pkg/helper.go` (whose `init()` a sibling
/// test depends on structurally, since `init` folds into the file's
/// `<module>`) must still seed the surviving same-package siblings'
/// `ModuleInit` — the stem-based importer scan alone finds nothing here
/// (no file anywhere imports the literal text `helper`/`helper.go`), so
/// before this fix the selection was empty (issue: Go file deletion
/// under-selects).
#[test]
fn deleted_go_file_seeds_package_siblings_module_init() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "go.mod", "module example.com/m\n\ngo 1.22\n");
    write(
        root,
        "pkg/widget.go",
        "package pkg\n\nfunc Widget() int { return 1 }\n",
    );
    write(
        root,
        "pkg/widget_test.go",
        "package pkg\n\nimport \"testing\"\n\nfunc TestWidget(t *testing.T) {\n\tif Widget() != 1 {\n\t\tt.Fail()\n\t}\n}\n",
    );

    let registry = Registry::new(vec![Box::new(testless_lang_go::GoLanguage)]);
    let (graph, extractions, _) = index_repo_incremental(root, &registry, None, None).unwrap();

    let test_file_id = testless_core::FileId(
        graph
            .files
            .iter()
            .position(|f| f.path.ends_with("widget_test.go"))
            .unwrap() as u32,
    );
    let test_module_init = graph
        .module_init(test_file_id)
        .expect("module_init present");

    // helper.go (a third file in the same package, never written to disk
    // here: it's the file being deleted) held the sibling this test's
    // `<module>` depended on. Its own `<module>` hash included an `init()`
    // whose body a sibling test structurally depended on; deleting it must
    // still seed the surviving siblings' `ModuleInit`.
    let changed = vec![ChangedFile {
        path: PathBuf::from("pkg/helper.go"),
        status: FileStatus::Deleted,
    }];

    let mode = classify(
        root,
        &graph,
        &registry,
        &changed,
        &extractions,
        &no_old_content,
    );
    match mode {
        ChangeMode::Selection(seeds) => {
            assert!(!seeds.is_empty(), "expected a non-empty selection");
            assert!(
                seeds.iter().any(|s| s.def == test_module_init),
                "expected the package test file's ModuleInit seeded, got {seeds:?}"
            );
        }
        other => panic!("expected Selection (not RunAll), got {other:?}"),
    }
}

// --- `testless changes` CLI e2e coverage (Plan 3, Task 5) --------------

mod cli_changes {
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

    fn init_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/math.ts"),
            "export function add(a: number, b: number): number { return a + b; }\n",
        )
        .unwrap();
        git(root, &["init", "-b", "main"]);
        git(root, &["add", "-A"]);
        git(root, &["commit", "-m", "initial"]);
        tmp
    }

    /// (a) A body-only edit of `add` (same signature) surfaces exactly one
    /// seed, `add`'s `body` change, as JSON on a piped stdout, exit 0.
    #[test]
    fn body_edit_yields_selection_seed_json() {
        let tmp = init_repo();
        let root = tmp.path();
        std::fs::write(
            root.join("src/math.ts"),
            "export function add(a: number, b: number): number { return a + b + 1; }\n",
        )
        .unwrap();

        let assert = Command::cargo_bin("testless")
            .unwrap()
            .arg("changes")
            .current_dir(root)
            .assert()
            .success();
        let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| {
            panic!("expected JSON stdout, got {out:?} ({e})");
        });
        assert_eq!(json["version"], 1);
        assert_eq!(json["mode"], "selection");
        let seeds = json["seeds"].as_array().expect("seeds array");
        assert!(
            seeds
                .iter()
                .any(|s| s["def"] == "add" && s["kind"] == "body"),
            "expected an add/body seed, got {seeds:?}"
        );
    }

    /// (b) A comment-only edit (no token change) yields an empty seed list,
    /// still exit 0.
    #[test]
    fn comment_only_edit_yields_empty_seeds() {
        let tmp = init_repo();
        let root = tmp.path();
        std::fs::write(
            root.join("src/math.ts"),
            "export function add(a: number, b: number): number { /* no-op */ return a + b; }\n",
        )
        .unwrap();

        let assert = Command::cargo_bin("testless")
            .unwrap()
            .arg("changes")
            .current_dir(root)
            .assert()
            .success();
        let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(json["mode"], "selection");
        assert_eq!(json["seeds"].as_array().unwrap().len(), 0);
    }

    /// (c) A `package.json` edit forces `run_all` and a distinct (non-hard-
    /// error) exit code of 2.
    #[test]
    fn config_file_edit_forces_run_all_exit_2() {
        let tmp = init_repo();
        let root = tmp.path();
        std::fs::write(root.join("package.json"), "{}\n").unwrap();

        let assert = Command::cargo_bin("testless")
            .unwrap()
            .arg("changes")
            .current_dir(root)
            .assert()
            .code(2);
        let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(json["mode"], "run_all");
        assert!(json["reason"].as_str().unwrap().contains("package.json"));
    }

    /// Item 2: a `--from` rev `changed_files` can't resolve degrades to a
    /// `run_all` fallback (exit 2) rather than a hard error; exit 1 stays
    /// reserved for index/cache infrastructure failures.
    #[test]
    fn bad_from_rev_degrades_to_run_all_exit_2() {
        let tmp = init_repo();
        let assert = Command::cargo_bin("testless")
            .unwrap()
            .args(["changes", "--from", "not-a-real-rev"])
            .current_dir(tmp.path())
            .assert()
            .code(2);
        let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(json["mode"], "run_all");
        assert!(json["reason"].as_str().unwrap().contains("--from"));
    }

    fn rev_parse(dir: &std::path::Path, rev: &str) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", rev])
            .output()
            .expect("failed to spawn git rev-parse");
        assert!(output.status.success(), "git rev-parse {rev} failed");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// `--to <rev>` (issue #17): an explicit `--from`..`--to` rev range
    /// (rather than `--from` vs. the worktree) reports exactly the seed for
    /// a body edit committed between the two revisions.
    #[test]
    fn to_flag_reports_seed_for_rev_range() {
        let tmp = init_repo();
        let root = tmp.path();
        let c1 = rev_parse(root, "HEAD");

        std::fs::write(
            root.join("src/math.ts"),
            "export function add(a: number, b: number): number { return a + b + 1; }\n",
        )
        .unwrap();
        git(root, &["commit", "-am", "edit add's body"]);
        let c2 = rev_parse(root, "HEAD");

        let assert = Command::cargo_bin("testless")
            .unwrap()
            .args(["changes", "--from", &c1, "--to", &c2])
            .current_dir(root)
            .assert()
            .success();
        let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| {
            panic!("expected JSON stdout, got {out:?} ({e})");
        });
        assert_eq!(json["mode"], "selection");
        let seeds = json["seeds"].as_array().expect("seeds array");
        assert!(
            seeds
                .iter()
                .any(|s| s["def"] == "add" && s["kind"] == "body"),
            "expected an add/body seed, got {seeds:?}"
        );
    }

    /// A modified file matching `testless.toml`'s `ignore` globs is
    /// discovery-level dropped (never indexed, see `discover`), so before
    /// this fix it hit `classify`'s "indexed file missing from new graph"
    /// error path and forced `run_all`. `ignore`'s contract is that a
    /// matching change contributes zero seeds, same as any other change
    /// `testless` can prove has no impact — not a blanket run-everything.
    #[test]
    fn ignored_file_change_yields_empty_selection_not_run_all() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src/generated")).unwrap();
        std::fs::write(root.join("src/generated/api.ts"), "export const x = 1;\n").unwrap();
        std::fs::write(
            root.join("testless.toml"),
            "ignore = [\"**/generated/**\"]\n",
        )
        .unwrap();
        git(root, &["init", "-b", "main"]);
        git(root, &["add", "-A"]);
        git(root, &["commit", "-m", "initial"]);

        std::fs::write(root.join("src/generated/api.ts"), "export const x = 2;\n").unwrap();

        let assert = Command::cargo_bin("testless")
            .unwrap()
            .arg("changes")
            .current_dir(root)
            .assert()
            .success();
        let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| {
            panic!("expected JSON stdout, got {out:?} ({e})");
        });
        assert_eq!(json["mode"], "selection");
        assert_eq!(json["seeds"].as_array().unwrap().len(), 0);
    }

    /// A `--to` rev that doesn't resolve locally degrades to the same
    /// `run_all`/exit-2 fallback as a bad `--from` rev, rather than a hard
    /// error.
    #[test]
    fn bad_to_rev_degrades_to_run_all_exit_2() {
        let tmp = init_repo();
        let assert = Command::cargo_bin("testless")
            .unwrap()
            .args(["changes", "--to", "not-a-real-rev"])
            .current_dir(tmp.path())
            .assert()
            .code(2);
        let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(json["mode"], "run_all");
        assert!(json["reason"].as_str().unwrap().contains("--to"));
    }

    // --- multi-language e2e coverage (Plan 3, Task 6) ------------------
    //
    // Mirrors (a)/(b) above but for Go and Rust repos, proving `changes`
    // isn't TS-only. `go.mod`/`Cargo.toml` are config globs (see
    // `classify::EXACT_CONFIG_NAMES`), which would force `RunAll` if they
    // were part of the *changed* set, but here they're only present at
    // commit time and untouched afterwards, so they never enter `changed`.

    fn init_go_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("go.mod"),
            "module example.com/go-app\n\ngo 1.22\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("calc")).unwrap();
        std::fs::write(
            root.join("calc/calc.go"),
            "package calc\n\nfunc Add(a, b int) int { return a + b }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("calc/calc_test.go"),
            "package calc\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n\tif Add(1, 2) != 3 {\n\t\tt.Fail()\n\t}\n}\n",
        )
        .unwrap();
        git(root, &["init", "-b", "main"]);
        git(root, &["add", "-A"]);
        git(root, &["commit", "-m", "initial"]);
        tmp
    }

    /// A body-only edit of Go's `Add` (same signature) surfaces exactly one
    /// seed, `Add`'s `body` change, as JSON on a piped stdout, exit 0.
    #[test]
    fn go_body_edit_yields_selection_seed_json() {
        let tmp = init_go_repo();
        let root = tmp.path();
        std::fs::write(
            root.join("calc/calc.go"),
            "package calc\n\nfunc Add(a, b int) int { return a + b + 1 }\n",
        )
        .unwrap();

        let assert = Command::cargo_bin("testless")
            .unwrap()
            .arg("changes")
            .current_dir(root)
            .assert()
            .success();
        let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| {
            panic!("expected JSON stdout, got {out:?} ({e})");
        });
        assert_eq!(json["mode"], "selection");
        let seeds = json["seeds"].as_array().expect("seeds array");
        assert!(
            seeds
                .iter()
                .any(|s| s["def"] == "Add" && s["kind"] == "body"),
            "expected an Add/body seed, got {seeds:?}"
        );
    }

    fn init_rust_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"rust-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub mod math;\n").unwrap();
        std::fs::write(
            root.join("src/math.rs"),
            "pub fn add(a: i64, b: i64) -> i64 { a + b }\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn add_works() { assert_eq!(add(2, 2), 4); }\n}\n",
        )
        .unwrap();
        git(root, &["init", "-b", "main"]);
        git(root, &["add", "-A"]);
        git(root, &["commit", "-m", "initial"]);
        tmp
    }

    /// A comment-only edit in `math.rs` (no token change) yields an empty
    /// seed list, still exit 0: the Rust mirror of the TS comment-only case.
    #[test]
    fn rust_comment_only_edit_yields_empty_seeds() {
        let tmp = init_rust_repo();
        let root = tmp.path();
        std::fs::write(
            root.join("src/math.rs"),
            "// adds two numbers\npub fn add(a: i64, b: i64) -> i64 { a + b }\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn add_works() { assert_eq!(add(2, 2), 4); }\n}\n",
        )
        .unwrap();

        let assert = Command::cargo_bin("testless")
            .unwrap()
            .arg("changes")
            .current_dir(root)
            .assert()
            .success();
        let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        let json: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(json["mode"], "selection");
        assert_eq!(json["seeds"].as_array().unwrap().len(), 0);
    }
}
