use pick_a_test_core::{DefKind, Language};
use pick_a_test_lang_ts::TsLanguage;
use std::path::{Path, PathBuf};

// re-use the extract() helper from extract.rs (copied verbatim; integration test files don't share modules)
fn extract(src: &str) -> pick_a_test_core::Extraction {
    let lang = TsLanguage;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang.grammar(std::path::Path::new("x.ts"))).unwrap();
    let tree = parser.parse(src, None).unwrap();
    lang.extract(src, &tree)
}

#[test]
fn extracts_nested_test_ids() {
    let src = std::fs::read_to_string("../../fixtures/ts-app/src/math.test.ts").unwrap();
    let ex = extract(&src);
    let tests: Vec<_> = ex.defs.iter().filter(|d| d.kind == DefKind::TestCase).collect();
    let ids: Vec<_> = tests.iter().filter_map(|d| d.test_id.clone()).collect();
    assert!(ids.contains(&vec!["add".into(), "handles negatives".into()]));
    assert!(ids.contains(&vec!["add".into(), "handles zero".into()]));
    assert!(ids.contains(&vec!["mul works".into()]));
    let computed = tests.iter().find(|d| d.computed_name).expect("computed-name test found");
    assert_eq!(computed.test_id.as_ref().unwrap()[0], "Calc"); // chain truncated at describe
}

#[test]
fn extracts_imports() {
    let src = std::fs::read_to_string("../../fixtures/ts-app/src/format.ts").unwrap();
    let ex = extract(&src);
    assert_eq!(ex.imports.iter().map(|i| i.raw.as_str()).collect::<Vec<_>>(), vec!["./math"]);
}

#[test]
fn resolves_relative_extensionless_index_and_js_suffix() {
    let root = Path::new("../../fixtures/ts-app");
    let l = TsLanguage;
    assert_eq!(l.resolve_import(Path::new("src/format.ts"), "./math", root),
               Some(PathBuf::from("src/math.ts")));
    assert_eq!(l.resolve_import(Path::new("src/format.test.ts"), "./index", root),
               Some(PathBuf::from("src/index.ts")));
    assert_eq!(l.resolve_import(Path::new("src/format.ts"), "./math.js", root),
               Some(PathBuf::from("src/math.ts")));
    assert_eq!(l.resolve_import(Path::new("src/format.ts"), "vitest", root), None);
}

#[test]
fn each_curried_calls_extract_as_computed_not_spurious() {
    let src = r#"
import { describe, it } from "vitest";
describe("math", () => {
  it.each([
    [1, 2],
    [3, 4],
  ])("adds %i and %i", (a, b) => {
    expect(a + b).toBeGreaterThan(0);
  });
});
"#;
    let ex = extract(src);
    let tests: Vec<_> = ex.defs.iter().filter(|d| d.kind == DefKind::TestCase).collect();
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0].test_id.as_ref().unwrap()[0], "math");
    assert!(tests[0].computed_name);
    // no spurious empty-chain defs
    assert!(tests.iter().all(|d| !d.test_id.as_ref().unwrap().is_empty()));
    // The def must be attributed to the OUTER curried call (`it.each(table)("adds %i and %i", cb)`),
    // which spans through the callback's closing `});` on line 9 — not the INNER
    // `it.each(table)` call alone, which ends at the array's closing `])` on line 7.
    // Before the fix, the inner call was misclassified as its own (spurious) Leaf,
    // so its span stopped at line 7 and the real callback body was never attributed
    // to this def.
    assert!(
        tests[0].end_line >= 9,
        "expected span to cover the outer call's callback (end_line >= 9), got {}",
        tests[0].end_line
    );
}
