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
