use testless_core::{indexer::index_repo, DefKind, Registry};

fn registry() -> Registry {
    Registry::new(vec![
        Box::new(testless_lang_ts::TsLanguage),
        Box::new(testless_lang_go::GoLanguage),
        Box::new(testless_lang_rust::RustLanguage),
    ])
}

#[test]
fn indexes_rust_fixture() {
    let g = index_repo(std::path::Path::new("../../fixtures/rust-app"), &registry()).unwrap();
    assert!(g.defs.iter().any(|d| d.name == "Calc.push"));
    assert!(g.defs.iter().any(|d| d.kind == DefKind::TestCase));
    let fmt = g
        .files
        .iter()
        .position(|f| f.path.ends_with("fmt.rs"))
        .unwrap() as u32;
    let math = g
        .files
        .iter()
        .position(|f| f.path.ends_with("math.rs"))
        .unwrap() as u32;
    assert!(g
        .edges
        .contains(&(fmt, testless_core::EdgeKind::Imports, math)));
}

#[test]
fn dogfood_indexes_own_repo() {
    // repo root is two levels up from crates/cli
    let g = index_repo(std::path::Path::new("../.."), &registry()).unwrap();
    // our own Rust source is extracted
    assert!(g.defs.iter().any(|d| d.name == "<module>"));
    assert!(
        g.defs
            .iter()
            .filter(|d| d.kind == DefKind::TestCase)
            .count()
            > 20,
        "should find our own #[test] fns plus fixture tests"
    );
}
