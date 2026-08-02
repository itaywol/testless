use std::path::{Path, PathBuf};
use testless_core::Language;
use testless_lang_java::JavaLanguage;

fn extract(src: &str) -> testless_core::Extraction {
    let lang = JavaLanguage::default();
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&lang.grammar(Path::new("X.java")))
        .unwrap();
    let tree = parser.parse(src, None).unwrap();
    lang.extract(src, &tree)
}

fn fixture(rel: &str) -> String {
    std::fs::read_to_string(format!("../../fixtures/java-app/{rel}")).unwrap()
}

const FIXTURE_ROOT: &str = "../../fixtures/java-app";

#[test]
fn extracts_qualified_method_calls() {
    let ex = extract(&fixture("src/test/java/com/example/calc/CalcTest.java"));
    let from = ex
        .defs
        .iter()
        .position(|d| d.name == "CalcTest.addsNegatives")
        .unwrap();
    assert!(
        ex.calls.iter().any(|c| c.name == "add"
            && c.qualifier.as_deref() == Some("calc")
            && c.from_def == from),
        "calls: {:?}",
        ex.calls
    );
}

/// `new Calc()` is both a constructor call and a dependency on the class,
/// recorded under the type's simple name so it matches the `Class` def.
#[test]
fn object_creation_is_a_call_on_the_type() {
    let ex = extract(&fixture("src/test/java/com/example/report/ReportTest.java"));
    let names: Vec<&str> = ex.calls.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"Calc"), "calls: {:?}", ex.calls);
    assert!(names.contains(&"Report"), "calls: {:?}", ex.calls);
}

#[test]
fn qualified_object_creation_uses_simple_name() {
    let src = r#"package com.foo;
class A {
    void go() { Object x = new com.example.calc.Calc(); }
}
"#;
    let ex = extract(src);
    assert!(
        ex.calls.iter().any(|c| c.name == "Calc"),
        "calls: {:?}",
        ex.calls
    );
}

/// A field or parameter *type* is a real structural dependency even when no
/// method of it is called by name — this is the edge that makes
/// dependency-injected Java code visible to the graph at all.
#[test]
fn field_and_parameter_types_are_reads() {
    let ex = extract(&fixture("src/main/java/com/example/report/Report.java"));
    assert!(
        ex.reads.iter().any(|r| r.name == "Calc"),
        "reads: {:?}",
        ex.reads
    );
}

#[test]
fn injected_type_is_seen_even_with_no_call() {
    let src = r#"package com.foo;
class Service {
    private final Repo repo;
    Service(Repo repo) { this.repo = repo; }
}
"#;
    let ex = extract(src);
    assert!(
        ex.reads.iter().any(|r| r.name == "Repo"),
        "reads: {:?}",
        ex.reads
    );
}

/// Refs must attribute to the enclosing method, not to the class or to
/// `<module>`, or impact would widen to every test in the file.
#[test]
fn refs_attribute_to_the_enclosing_method() {
    let ex = extract(&fixture("src/test/java/com/example/calc/CalcTest.java"));
    let multiplies = ex
        .defs
        .iter()
        .position(|d| d.name == "CalcTest.multiplies")
        .unwrap();
    let mul_call = ex
        .calls
        .iter()
        .find(|c| c.name == "mul")
        .expect("mul call missing");
    assert_eq!(mul_call.from_def, multiplies);
}

#[test]
fn nested_class_method_refs_attribute_to_the_nested_method() {
    let ex = extract(&fixture("src/test/java/com/example/calc/CalcTest.java"));
    let nested = ex
        .defs
        .iter()
        .position(|d| d.name == "WhenDoubling.doublesTheSum")
        .unwrap();
    let call = ex
        .calls
        .iter()
        .find(|c| c.name == "addTwice")
        .expect("addTwice call missing");
    assert_eq!(call.from_def, nested);
}

// --- resolve_import ------------------------------------------------------

#[test]
fn resolves_same_module_import_to_the_class_file() {
    let lang = JavaLanguage::default();
    let resolved = lang.resolve_import(
        Path::new("src/test/java/com/example/report/ReportTest.java"),
        "com.example.calc.Calc",
        Path::new(FIXTURE_ROOT),
    );
    assert_eq!(
        resolved,
        Some(PathBuf::from("src/main/java/com/example/calc/Calc.java"))
    );
}

/// The importing file's own source root is tried first, but a class that
/// only exists in the *other* root must still resolve — that cross-root
/// hop is how every test reaches the code it tests.
#[test]
fn resolves_across_source_roots() {
    let lang = JavaLanguage::default();
    let resolved = lang.resolve_import(
        Path::new("src/main/java/com/example/report/Report.java"),
        "com.example.calc.Calc",
        Path::new(FIXTURE_ROOT),
    );
    assert_eq!(
        resolved,
        Some(PathBuf::from("src/main/java/com/example/calc/Calc.java"))
    );
}

#[test]
fn resolves_wildcard_import_to_the_package_directory() {
    let lang = JavaLanguage::default();
    let resolved = lang.resolve_import(
        Path::new("src/test/java/com/example/report/ReportTest.java"),
        "com.example.calc.*",
        Path::new(FIXTURE_ROOT),
    );
    assert_eq!(
        resolved,
        Some(PathBuf::from("src/main/java/com/example/calc"))
    );
}

/// `import static a.b.C.member` names a member, so the last segment has to
/// be dropped before the file probe can hit `C.java`.
#[test]
fn resolves_static_member_import_to_its_owning_class() {
    let lang = JavaLanguage::default();
    let resolved = lang.resolve_import(
        Path::new("src/test/java/com/example/report/ReportTest.java"),
        "com.example.calc.Calc.add",
        Path::new(FIXTURE_ROOT),
    );
    assert_eq!(
        resolved,
        Some(PathBuf::from("src/main/java/com/example/calc/Calc.java"))
    );
}

#[test]
fn external_imports_do_not_resolve() {
    let lang = JavaLanguage::default();
    for raw in [
        "java.util.List",
        "org.junit.jupiter.api.Test",
        "com.nope.Missing",
    ] {
        assert_eq!(
            lang.resolve_import(
                Path::new("src/test/java/com/example/calc/CalcTest.java"),
                raw,
                Path::new(FIXTURE_ROOT),
            ),
            None,
            "{raw} should not resolve"
        );
    }
}
