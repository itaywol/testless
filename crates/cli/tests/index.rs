use pick_a_test_core::{indexer::index_repo, DefKind, EdgeKind, Registry};

fn registry() -> Registry {
    Registry::new(vec![
        Box::new(pick_a_test_lang_ts::TsLanguage),
        Box::new(pick_a_test_lang_go::GoLanguage),
    ])
}

#[test]
fn indexes_both_fixture_apps() {
    let g = index_repo(std::path::Path::new("../../fixtures/ts-app"), &registry()).unwrap();
    assert!(g.defs.iter().any(|d| d.name == "add" && d.kind == DefKind::Function));
    assert!(g.defs.iter().any(|d| d.kind == DefKind::TestCase));
    // format.ts imports math.ts
    let format = g.files.iter().position(|f| f.path.ends_with("format.ts")).unwrap() as u32;
    let math = g.files.iter().position(|f| f.path.ends_with("math.ts")).unwrap() as u32;
    assert!(g.edges.contains(&(format, EdgeKind::Imports, math)));

    let g = index_repo(std::path::Path::new("../../fixtures/go-app"), &registry()).unwrap();
    assert!(g.defs.iter().any(|d| d.name == "Calc.Push"));
    let fmt2 = g.files.iter().position(|f| f.path.ends_with("fmt2.go")).unwrap() as u32;
    let calc = g.files.iter().position(|f| f.path.ends_with("calc.go")).unwrap() as u32;
    assert!(g.edges.contains(&(fmt2, EdgeKind::Imports, calc)));
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
    let main = g.files.iter().position(|f| f.path.ends_with("main.ts")).unwrap() as u32;
    let m = g.files.iter().position(|f| f.path.ends_with("m.ts")).unwrap() as u32;
    let import_edges =
        g.edges.iter().filter(|e| **e == (main, EdgeKind::Imports, m)).count();
    assert_eq!(import_edges, 1);
}
