use std::path::Path;
use testless_core::Language;
use testless_lang_rust::RustLanguage;

fn extract(src: &str) -> testless_core::Extraction {
    let lang = RustLanguage;
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&lang.grammar(Path::new("x.rs")))
        .unwrap();
    let tree = parser.parse(src, None).unwrap();
    lang.extract(src, &tree)
}

#[test]
fn extracts_cross_module_calls() {
    let src = std::fs::read_to_string("../../fixtures/rust-app/src/fmt.rs").unwrap();
    let ex = extract(&src);
    let f = ex.defs.iter().position(|d| d.name == "fmt").unwrap();
    assert!(ex.calls.iter().any(|c| c.name == "add" && c.from_def == f));
}

#[test]
fn method_calls_and_test_attribution() {
    let src = std::fs::read_to_string("../../fixtures/rust-app/src/math.rs").unwrap();
    let ex = extract(&src);
    // Calc.push body calls add()
    let push = ex.defs.iter().position(|d| d.name == "Calc.push").unwrap();
    assert!(ex
        .calls
        .iter()
        .any(|c| c.name == "add" && c.from_def == push));
    // test add_works calls add()
    let t = ex
        .defs
        .iter()
        .position(|d| d.test_id.as_deref() == Some(&["tests".into(), "add_works".into()][..]))
        .unwrap();
    assert!(ex.calls.iter().any(|c| c.name == "add" && c.from_def == t));
}
