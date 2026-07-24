use testless_core::{DefKind, Language};
use testless_lang_go::GoLanguage;

fn extract(src: &str) -> testless_core::Extraction {
    let lang = GoLanguage;
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&lang.grammar(std::path::Path::new("x.go")))
        .unwrap();
    let tree = parser.parse(src, None).unwrap();
    lang.extract(src, &tree)
}

#[test]
fn extracts_cross_package_calls() {
    let src = std::fs::read_to_string("../../fixtures/go-app/fmt2/fmt2.go").unwrap();
    let ex = extract(&src);
    let f = ex.defs.iter().position(|d| d.name == "Fmt").unwrap();
    assert!(ex
        .calls
        .iter()
        .any(|c| c.name == "Add" && c.qualifier.as_deref() == Some("calc") && c.from_def == f));
}

#[test]
fn aliased_import_qualifier_is_recorded_as_known_name() {
    let src = r#"package p

import foo "example.com/go-app/calc"

func Fmt() int {
	return foo.Something
}
"#;
    let ex = extract(src);
    let f = ex.defs.iter().position(|d| d.name == "Fmt").unwrap();
    assert!(ex.reads.iter().any(|r| r.name == "Something"
        && r.qualifier.as_deref() == Some("foo")
        && r.from_def == f));
}

#[test]
fn non_testing_run_is_plain_call_not_subtest() {
    let src = r#"package p

import "testing"

func TestServer(t *testing.T) {
	srv := newServer()
	srv.Run(":8080")
	t.Run("real subtest", func(t *testing.T) {})
}
"#;
    let ex = extract(src);
    let ids: Vec<_> = ex
        .defs
        .iter()
        .filter(|d| d.kind == DefKind::TestCase)
        .filter_map(|d| d.test_id.clone())
        .collect();
    assert!(ids.contains(&vec!["TestServer".into(), "real subtest".into()]));
    // srv.Run must NOT create a subtest
    assert_eq!(ids.iter().filter(|i| i.len() == 2).count(), 1);
    // and IS recorded as an ordinary call
    let root = ex
        .defs
        .iter()
        .position(|d| d.test_id.as_deref() == Some(&["TestServer".into()][..]))
        .unwrap();
    assert!(ex
        .calls
        .iter()
        .any(|c| c.name == "Run" && c.qualifier.as_deref() == Some("srv") && c.from_def == root));
}
