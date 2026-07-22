use pick_a_test_core::{DefKind, Language};
use pick_a_test_lang_ts::TsLanguage;

fn extract(src: &str) -> pick_a_test_core::Extraction {
    let lang = TsLanguage;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang.grammar(std::path::Path::new("x.ts"))).unwrap();
    let tree = parser.parse(src, None).unwrap();
    lang.extract(src, &tree)
}

#[test]
fn extracts_function_arrow_class_method() {
    let src = std::fs::read_to_string("../../fixtures/ts-app/src/math.ts").unwrap();
    let ex = extract(&src);
    let names: Vec<(&str, DefKind)> =
        ex.defs.iter().map(|d| (d.name.as_str(), d.kind)).collect();
    assert!(names.contains(&("<module>", DefKind::ModuleInit)));
    assert!(names.contains(&("add", DefKind::Function)));
    assert!(names.contains(&("mul", DefKind::Function)));
    assert!(names.contains(&("Calc", DefKind::Class)));
    assert!(names.contains(&("push", DefKind::Method)));
    let push = ex.defs.iter().position(|d| d.name == "push").unwrap();
    let calc = ex.defs.iter().position(|d| d.name == "Calc").unwrap();
    assert_eq!(ex.defs[push].parent, Some(calc));
}
