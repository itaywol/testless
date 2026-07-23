use testless_core::{indexer::index_repo, CallTarget, DefKind, Edge, FileId, Registry};

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
    let fmt = FileId(
        g.files
            .iter()
            .position(|f| f.path.ends_with("fmt.rs"))
            .unwrap() as u32,
    );
    let math = FileId(
        g.files
            .iter()
            .position(|f| f.path.ends_with("math.rs"))
            .unwrap() as u32,
    );
    assert!(g.edges.contains(&Edge::Imports {
        from: fmt,
        to: math
    }));
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

    let calls = g
        .edges
        .iter()
        .filter(|e| matches!(e, Edge::Calls { .. }))
        .count();
    assert!(calls > 0, "should find calls in our own repo");

    let cross_file_resolved_call = g.edges.iter().any(|e| match e {
        Edge::Calls {
            from,
            to: CallTarget::Resolved(to),
        } => g.def(*from).file != g.def(*to).file,
        _ => false,
    });
    assert!(
        cross_file_resolved_call,
        "should find at least one resolved call edge crossing file boundaries"
    );
}
