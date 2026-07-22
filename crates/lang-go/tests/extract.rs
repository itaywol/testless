use pick_a_test_core::{DefKind, Language};
use pick_a_test_lang_go::GoLanguage;
use std::path::{Path, PathBuf};

fn extract(src: &str) -> pick_a_test_core::Extraction {
    let lang = GoLanguage;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang.grammar(Path::new("x.go"))).unwrap();
    let tree = parser.parse(src, None).unwrap();
    lang.extract(src, &tree)
}

#[test]
fn extracts_funcs_methods_init() {
    let src = std::fs::read_to_string("../../fixtures/go-app/calc/calc.go").unwrap();
    let ex = extract(&src);
    let names: Vec<(&str, DefKind)> = ex.defs.iter().map(|d| (d.name.as_str(), d.kind)).collect();
    assert!(names.contains(&("Add", DefKind::Function)));
    assert!(names.contains(&("Calc.Push", DefKind::Method)));
    assert!(names.contains(&("<module>", DefKind::ModuleInit))); // init() folded in
}

#[test]
fn extracts_tests_subtests_and_computed() {
    let src = std::fs::read_to_string("../../fixtures/go-app/calc/calc_test.go").unwrap();
    let ex = extract(&src);
    let ids: Vec<_> = ex.defs.iter().filter(|d| d.kind == DefKind::TestCase)
        .map(|d| (d.test_id.clone().unwrap(), d.computed_name)).collect();
    assert!(ids.contains(&(vec!["TestAdd".into(), "negatives".into()], false)));
    assert!(ids.contains(&(vec!["TestAdd".into(), "zero".into()], false)));
    assert!(ids.contains(&(vec!["TestCalc".into(), "<computed>".into()], true)));
    assert!(ids.contains(&(vec!["BenchmarkAdd".into()], false)));
}

#[test]
fn nested_t_run_chains_fully() {
    let src = r#"package p

import "testing"

func TestFoo(t *testing.T) {
	t.Run("outer", func(t *testing.T) {
		t.Run("inner", func(t *testing.T) {})
	})
}
"#;
    let ex = extract(src);
    let ids: Vec<_> = ex.defs.iter().filter(|d| d.kind == DefKind::TestCase)
        .filter_map(|d| d.test_id.clone()).collect();
    assert!(ids.contains(&vec!["TestFoo".into(), "outer".into()]));
    assert!(ids.contains(&vec!["TestFoo".into(), "outer".into(), "inner".into()]));
    // inner's parent is outer's def, not TestFoo
    let outer = ex.defs.iter().position(|d| d.test_id.as_deref() == Some(&["TestFoo".into(), "outer".into()][..])).unwrap();
    let inner = ex.defs.iter().find(|d| d.test_id.as_deref() == Some(&["TestFoo".into(), "outer".into(), "inner".into()][..])).unwrap();
    assert_eq!(inner.parent, Some(outer));
}

#[test]
fn resolves_module_internal_imports() {
    let root = Path::new("../../fixtures/go-app");
    let l = GoLanguage;
    assert_eq!(l.resolve_import(Path::new("fmt2/fmt2.go"), "example.com/go-app/calc", root),
               Some(PathBuf::from("calc")));
    assert_eq!(l.resolve_import(Path::new("fmt2/fmt2.go"), "fmt", root), None);
}
