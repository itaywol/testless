use testless_core::Language;
use testless_lang_ts::TsLanguage;

// re-use the extract() helper from extract.rs (copied verbatim; integration test files don't share modules)
fn extract(src: &str) -> testless_core::Extraction {
    let lang = TsLanguage;
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&lang.grammar(std::path::Path::new("x.ts")))
        .unwrap();
    let tree = parser.parse(src, None).unwrap();
    lang.extract(src, &tree)
}

#[test]
fn extracts_calls_with_enclosing_def_attribution() {
    let src = std::fs::read_to_string("../../fixtures/ts-app/src/format.ts").unwrap();
    let ex = extract(&src);
    // fmt() calls add() imported from ./math
    let fmt_idx = ex.defs.iter().position(|d| d.name == "fmt").unwrap();
    assert!(ex
        .calls
        .iter()
        .any(|c| c.name == "add" && c.from_def == fmt_idx));
}

#[test]
fn test_bodies_attribute_calls_to_testcase() {
    let src = std::fs::read_to_string("../../fixtures/ts-app/src/math.test.ts").unwrap();
    let ex = extract(&src);
    let neg = ex
        .defs
        .iter()
        .position(|d| d.test_id.as_deref() == Some(&["add".into(), "handles negatives".into()][..]))
        .unwrap();
    assert!(ex
        .calls
        .iter()
        .any(|c| c.name == "add" && c.from_def == neg));
}

#[test]
fn method_calls_carry_qualifier() {
    let src = r#"
import { Calc } from "./math";
export function run() { const c = new Calc(); c.push(1); }
"#;
    let ex = extract(src);
    let run = ex.defs.iter().position(|d| d.name == "run").unwrap();
    assert!(ex
        .calls
        .iter()
        .any(|c| c.name == "Calc" && c.from_def == run)); // new Calc()
    assert!(ex
        .calls
        .iter()
        .any(|c| c.name == "push" && c.qualifier.is_some() && c.from_def == run));
}
