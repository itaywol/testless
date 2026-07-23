use testless_core::DefKind;
use testless_lang_rust::RustLanguage;
use testless_core::Language;
use std::path::Path;

fn extract(src: &str) -> testless_core::Extraction {
    let lang = RustLanguage;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang.grammar(Path::new("x.rs"))).unwrap();
    let tree = parser.parse(src, None).unwrap();
    lang.extract(src, &tree)
}

#[test]
fn extracts_cfg_test_mod_tests_with_chain() {
    let src = std::fs::read_to_string("../../fixtures/rust-app/src/math.rs").unwrap();
    let ex = extract(&src);
    let ids: Vec<_> = ex.defs.iter().filter(|d| d.kind == DefKind::TestCase)
        .filter_map(|d| d.test_id.clone()).collect();
    assert!(ids.contains(&vec!["tests".into(), "add_works".into()]));
    assert!(ids.contains(&vec!["tests".into(), "calc_push".into()]));
}

#[test]
fn attribute_variants_and_integration_tests() {
    let src = std::fs::read_to_string("../../fixtures/rust-app/src/fmt.rs").unwrap();
    let ex = extract(&src);
    let ids: Vec<_> = ex.defs.iter().filter(|d| d.kind == DefKind::TestCase)
        .filter_map(|d| d.test_id.clone()).collect();
    assert!(ids.contains(&vec!["tests".into(), "fmt_async_style".into()])); // #[tokio::test]

    let src = std::fs::read_to_string("../../fixtures/rust-app/tests/integration.rs").unwrap();
    let ex = extract(&src);
    let ids: Vec<_> = ex.defs.iter().filter(|d| d.kind == DefKind::TestCase)
        .filter_map(|d| d.test_id.clone()).collect();
    assert!(ids.contains(&vec!["integration_add".into()]));
}
