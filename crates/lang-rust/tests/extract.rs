use testless_core::{DefKind, Language};
use testless_lang_rust::RustLanguage;
use std::path::Path;

fn extract(src: &str) -> testless_core::Extraction {
    let lang = RustLanguage;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang.grammar(Path::new("x.rs"))).unwrap();
    let tree = parser.parse(src, None).unwrap();
    lang.extract(src, &tree)
}

#[test]
fn extracts_fns_methods_types_module_init() {
    let src = std::fs::read_to_string("../../fixtures/rust-app/src/math.rs").unwrap();
    let ex = extract(&src);
    let names: Vec<(&str, DefKind)> = ex.defs.iter().map(|d| (d.name.as_str(), d.kind)).collect();
    assert!(names.contains(&("<module>", DefKind::ModuleInit)));
    assert!(names.contains(&("add", DefKind::Function)));
    assert!(names.contains(&("Calc", DefKind::Class)));
    assert!(names.contains(&("Calc.push", DefKind::Method)));
}
