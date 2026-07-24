use testless_core::{
    indexer::{index_repo, index_repo_incremental},
    CallTarget, DefId, DefKind, Edge, FileId, Graph, Registry,
};

/// First def whose `name` matches → its `DefId` (position in `g.defs`).
fn find_def(g: &Graph, name: &str) -> DefId {
    DefId(
        g.defs
            .iter()
            .position(|d| d.name == name)
            .unwrap_or_else(|| panic!("no def named {name:?}")) as u32,
    )
}

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

#[test]
fn resolves_cross_file_call_edges_ts() {
    let g = index_repo(std::path::Path::new("../../fixtures/ts-app"), &registry()).unwrap();
    let fmt = find_def(&g, "fmt");
    let add = find_def(&g, "add");
    assert!(g.edges.contains(&Edge::Calls {
        from: fmt,
        to: CallTarget::Resolved(add)
    }));
    // test → add edge too
    let neg = g
        .defs
        .iter()
        .position(|d| d.test_id.as_deref() == Some(&["add".into(), "handles negatives".into()][..]))
        .map(|i| DefId(i as u32))
        .unwrap();
    assert!(g.edges.contains(&Edge::Calls {
        from: neg,
        to: CallTarget::Resolved(add)
    }));
}

#[test]
fn resolves_cross_package_go_and_cross_module_rust() {
    let g = index_repo(std::path::Path::new("../../fixtures/go-app"), &registry()).unwrap();
    let f = find_def(&g, "Fmt");
    let add = find_def(&g, "Add");
    assert!(g.edges.contains(&Edge::Calls {
        from: f,
        to: CallTarget::Resolved(add)
    }));

    let g = index_repo(std::path::Path::new("../../fixtures/rust-app"), &registry()).unwrap();
    let f = find_def(&g, "fmt");
    let add = find_def(&g, "add");
    assert!(g.edges.contains(&Edge::Calls {
        from: f,
        to: CallTarget::Resolved(add)
    }));
}

#[test]
fn unresolved_calls_become_unknown_markers() {
    // ts fixture: console.log(...) at top level → callee `log` qualifier `console` unresolvable
    let g = index_repo(std::path::Path::new("../../fixtures/ts-app"), &registry()).unwrap();
    assert!(g
        .edges
        .iter()
        .any(|e| matches!(e, Edge::Calls { to: CallTarget::Unknown(n), .. } if n == "log")));
}

#[test]
fn self_only_candidate_recursion_still_widens_to_unknown() {
    // `solo` recursively calls itself and is the *only* def named `solo` in
    // scope, so every raw candidate is filtered out by the self-edge guard.
    // The ref must still yield ≥1 edge; it falls through to `Unknown`
    // rather than silently vanishing, since a same-named symbol whose
    // import failed to resolve would look identical and must not be
    // dropped without a trace.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/solo.ts"),
        "export function solo(n: number): number { return n <= 1 ? 1 : solo(n - 1); }\n",
    )
    .unwrap();

    let g = index_repo(root, &registry()).unwrap();
    let solo = find_def(&g, "solo");
    assert!(g.edges.contains(&Edge::Calls {
        from: solo,
        to: CallTarget::Unknown("solo".to_string())
    }));
}
