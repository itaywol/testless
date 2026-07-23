use testless_core::{
    indexer::{index_repo, index_repo_incremental},
    DefKind, Edge, FileId, Registry,
};

fn registry() -> Registry {
    Registry::new(vec![
        Box::new(testless_lang_ts::TsLanguage),
        Box::new(testless_lang_go::GoLanguage),
        Box::new(testless_lang_rust::RustLanguage),
    ])
}

/// Recursively copies `src` into `dst` (which must already exist), for
/// setting up a mutable tempdir fixture from a read-only source tree.
fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let ty = entry.file_type().unwrap();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            std::fs::create_dir_all(&dst_path).unwrap();
            copy_dir(&entry.path(), &dst_path);
        } else {
            std::fs::copy(entry.path(), &dst_path).unwrap();
        }
    }
}

#[test]
fn indexes_both_fixture_apps() {
    let g = index_repo(std::path::Path::new("../../fixtures/ts-app"), &registry()).unwrap();
    assert!(g
        .defs
        .iter()
        .any(|d| d.name == "add" && d.kind == DefKind::Function));
    assert!(g.defs.iter().any(|d| d.kind == DefKind::TestCase));
    // format.ts imports math.ts
    let format = FileId(
        g.files
            .iter()
            .position(|f| f.path.ends_with("format.ts"))
            .unwrap() as u32,
    );
    let math = FileId(
        g.files
            .iter()
            .position(|f| f.path.ends_with("math.ts"))
            .unwrap() as u32,
    );
    assert!(g.edges.contains(&Edge::Imports {
        from: format,
        to: math
    }));

    let g = index_repo(std::path::Path::new("../../fixtures/go-app"), &registry()).unwrap();
    assert!(g.defs.iter().any(|d| d.name == "Calc.Push"));
    let fmt2 = FileId(
        g.files
            .iter()
            .position(|f| f.path.ends_with("fmt2.go"))
            .unwrap() as u32,
    );
    let calc = FileId(
        g.files
            .iter()
            .position(|f| f.path.ends_with("calc.go"))
            .unwrap() as u32,
    );
    assert!(g.edges.contains(&Edge::Imports {
        from: fmt2,
        to: calc
    }));
}

#[test]
fn dedups_repeated_imports_of_the_same_target() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/main.ts"),
        concat!(
            "import { a } from \"./m\";\n",
            "import type { B } from \"./m\";\n",
            "export function use(x: B): number { return a(x); }\n",
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("src/m.ts"),
        "export function a(_x: unknown): number { return 0; }\nexport interface B {}\n",
    )
    .unwrap();

    let g = index_repo(root, &registry()).unwrap();
    let main = FileId(
        g.files
            .iter()
            .position(|f| f.path.ends_with("main.ts"))
            .unwrap() as u32,
    );
    let m = FileId(
        g.files
            .iter()
            .position(|f| f.path.ends_with("m.ts"))
            .unwrap() as u32,
    );
    let import_edges = g
        .edges
        .iter()
        .filter(|e| **e == Edge::Imports { from: main, to: m })
        .count();
    assert_eq!(import_edges, 1);
}

#[test]
fn incremental_reindex_reuses_unchanged_files_and_reparses_only_the_changed_one() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    copy_dir(std::path::Path::new("../../fixtures/ts-app"), root);

    let files_count = |g: &testless_core::Graph| g.files.len();

    let (graph, extractions, stats) = index_repo_incremental(root, &registry(), None).unwrap();
    assert_eq!(stats.parsed, files_count(&graph));
    assert_eq!(stats.reused, 0);

    // Mutate math.ts by appending a new function; every other fixture file
    // is untouched.
    let math_path = root.join("src/math.ts");
    let mut src = std::fs::read_to_string(&math_path).unwrap();
    src.push_str("\nexport function sub(a: number, b: number): number { return a - b; }\n");
    std::fs::write(&math_path, src).unwrap();

    let (graph2, _extractions2, stats2) =
        index_repo_incremental(root, &registry(), Some((graph, extractions))).unwrap();

    // Only math.ts changed, so only it should have been re-parsed.
    assert_eq!(stats2.parsed, 1);
    assert_eq!(stats2.reused, files_count(&graph2) - 1);

    // New def from the appended function is present.
    assert!(graph2
        .defs
        .iter()
        .any(|d| d.name == "sub" && d.kind == DefKind::Function));
    // Defs from the unchanged math.ts function are still present.
    assert!(graph2
        .defs
        .iter()
        .any(|d| d.name == "add" && d.kind == DefKind::Function));
    // Defs from an entirely untouched file (format.ts) are still present too.
    assert!(graph2.defs.iter().any(|d| d.kind == DefKind::TestCase));
}
