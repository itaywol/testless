use std::path::{Path, PathBuf};
use testless_core::{DefKind, Language};
use testless_lang_go::GoLanguage;

fn extract(src: &str) -> testless_core::Extraction {
    let lang = GoLanguage;
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&lang.grammar(Path::new("x.go")))
        .unwrap();
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
    let ids: Vec<_> = ex
        .defs
        .iter()
        .filter(|d| d.kind == DefKind::TestCase)
        .map(|d| (d.test_id.clone().unwrap(), d.computed_name))
        .collect();
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
    let ids: Vec<_> = ex
        .defs
        .iter()
        .filter(|d| d.kind == DefKind::TestCase)
        .filter_map(|d| d.test_id.clone())
        .collect();
    assert!(ids.contains(&vec!["TestFoo".into(), "outer".into()]));
    assert!(ids.contains(&vec!["TestFoo".into(), "outer".into(), "inner".into()]));
    // inner's parent is outer's def, not TestFoo
    let outer = ex
        .defs
        .iter()
        .position(|d| d.test_id.as_deref() == Some(&["TestFoo".into(), "outer".into()][..]))
        .unwrap();
    let inner = ex
        .defs
        .iter()
        .find(|d| {
            d.test_id.as_deref() == Some(&["TestFoo".into(), "outer".into(), "inner".into()][..])
        })
        .unwrap();
    assert_eq!(inner.parent, Some(outer));
}

#[test]
fn extracts_dot_and_blank_imports() {
    let src = r#"package p

import (
	. "example.com/go-app/calc"
	_ "example.com/go-app/fmt2"
)

func Use() {}
"#;
    let ex = extract(src);
    let raws: Vec<&str> = ex.imports.iter().map(|i| i.raw.as_str()).collect();
    assert!(raws.contains(&"example.com/go-app/calc"));
    assert!(raws.contains(&"example.com/go-app/fmt2"));
}

#[test]
fn resolves_module_internal_imports() {
    let root = Path::new("../../fixtures/go-app");
    let l = GoLanguage;
    assert_eq!(
        l.resolve_import(Path::new("fmt2/fmt2.go"), "example.com/go-app/calc", root),
        Some(PathBuf::from("calc"))
    );
    assert_eq!(
        l.resolve_import(Path::new("fmt2/fmt2.go"), "fmt", root),
        None
    );
}

/// The ModuleInit `<module>` def's `sig_hash` (there is always exactly one).
fn module_init_sig_hash(ex: &testless_core::Extraction) -> [u8; 32] {
    ex.defs
        .iter()
        .find(|d| d.kind == DefKind::ModuleInit)
        .expect("ModuleInit def")
        .sig_hash
}

#[test]
fn init_body_change_moves_module_init_hash() {
    // `init()` contributes no def of its own (folded into ModuleInit per
    // extract's doc comment), so a change confined entirely to its body
    // must still show up in the ModuleInit sig_hash, or such a change would
    // select zero tests on re-index.
    let a = extract(
        r#"package p

func Add(a, b int) int { return a + b }

func init() { _ = Add(0, 0) }
"#,
    );
    let b = extract(
        r#"package p

func Add(a, b int) int { return a + b }

func init() { _ = Add(1, 1) }
"#,
    );
    assert_ne!(
        module_init_sig_hash(&a),
        module_init_sig_hash(&b),
        "init() body change must change the ModuleInit sig_hash"
    );
}

#[test]
fn normal_function_body_change_does_not_move_module_init_hash() {
    // A change confined to an ordinary top-level function's body is already
    // covered by that function's own def hash; it must NOT also move the
    // ModuleInit hash (that def's own body is excluded from the module
    // hash, same as every other non-`init` function).
    let a = extract(
        r#"package p

func Add(a, b int) int { return a + b }

func init() {}
"#,
    );
    let b = extract(
        r#"package p

func Add(a, b int) int { return a - b }

func init() {}
"#,
    );
    assert_eq!(
        module_init_sig_hash(&a),
        module_init_sig_hash(&b),
        "a normal function's body change must not affect the ModuleInit sig_hash"
    );
}
