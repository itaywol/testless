//! Def-level diff invariance tests (Plan 3, Task 2), plus (Task 4) the
//! change-classifier's rule table exercised end-to-end against real
//! tree-sitter-parsed TS fixtures — `classify` needs a real `Language` to
//! diff old/new content, so that coverage lives here rather than in
//! `testless-core` (which has no language crate to link against).

use std::path::{Path, PathBuf};

use testless_core::classify::{classify, ChangeMode, Seed, SeedKind};
use testless_core::gitio::{ChangedFile, FileStatus};
use testless_core::indexer::index_repo;
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
/// file — no identifier, literal, or keyword changed. `diff_defs` on the two
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
/// must seed exactly `add`'s def with `SeedKind::Body` — nothing else.
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
    let graph = index_repo(root, &registry).unwrap();
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

    let mode = classify(root, &graph, &registry, &changed, &old_src_of);
    assert_eq!(
        mode,
        ChangeMode::Selection(vec![Seed {
            def: add,
            kind: SeedKind::Body,
        }])
    );
}

/// (b) An edit that only adds a comment (no token change) must classify as
/// an empty selection — zero seeds is a valid outcome for a nonempty
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
    let graph = index_repo(root, &registry).unwrap();

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

    let mode = classify(root, &graph, &registry, &changed, &old_src_of);
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
    let graph = index_repo(root, &registry).unwrap();

    let changed = vec![ChangedFile {
        path: PathBuf::from("package.json"),
        status: FileStatus::Modified,
    }];

    let mode = classify(root, &graph, &registry, &changed, &no_old_content);
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
    let graph = index_repo(root, &registry).unwrap();
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

    let mode = classify(root, &graph, &registry, &changed, &no_old_content);
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

/// (e) A deleted, previously-indexed source file forces `RunAll` — the
/// documented sound-but-coarse decision for Plan 3.
#[test]
fn deleted_source_file_forces_run_all() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/math.ts", MATH_ADD_ORIGINAL);

    let registry = registry();
    let graph = index_repo(root, &registry).unwrap();

    let changed = vec![ChangedFile {
        path: PathBuf::from("src/removed.ts"),
        status: FileStatus::Deleted,
    }];

    let mode = classify(root, &graph, &registry, &changed, &no_old_content);
    match &mode {
        ChangeMode::RunAll { reason } => {
            assert!(reason.contains("removed.ts"), "reason: {reason}");
        }
        other => panic!("expected RunAll, got {other:?}"),
    }
}

/// (f) An unindexed file (no registered `Language`) with no importer
/// referencing it contributes zero seeds — e.g. a README edit.
#[test]
fn unindexed_file_with_no_importers_yields_empty_selection() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/math.ts", MATH_ADD_ORIGINAL);
    write(root, "README.md", "# hello\n");

    let registry = registry();
    let graph = index_repo(root, &registry).unwrap();

    let changed = vec![ChangedFile {
        path: PathBuf::from("README.md"),
        status: FileStatus::Modified,
    }];

    let mode = classify(root, &graph, &registry, &changed, &no_old_content);
    assert_eq!(mode, ChangeMode::Selection(vec![]));
}

/// (g) A changed `data.json` that's referenced only via raw (unresolved)
/// import text — `import data from "./data.json"` — seeds the importer's
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
    let graph = index_repo(root, &registry).unwrap();
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

    let mode = classify(root, &graph, &registry, &changed, &no_old_content);
    assert_eq!(
        mode,
        ChangeMode::Selection(vec![Seed {
            def: consumer_module_init,
            kind: SeedKind::ModuleInit,
        }])
    );
}
