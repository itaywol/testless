use std::path::{Path, PathBuf};
use testless_core::{DefKind, Language};
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

#[test]
fn extracts_classes_and_methods() {
    let ex = extract(&fixture("src/main/java/com/example/calc/Calc.java"));
    let names: Vec<(&str, DefKind)> = ex.defs.iter().map(|d| (d.name.as_str(), d.kind)).collect();
    assert!(names.contains(&("Calc", DefKind::Class)));
    assert!(names.contains(&("Calc.add", DefKind::Method)));
    assert!(names.contains(&("Calc.addTwice", DefKind::Method)));
    assert!(names.contains(&("<module>", DefKind::ModuleInit)));
    // Nothing in a non-test class should be a TestCase.
    assert!(!names.iter().any(|(_, k)| *k == DefKind::TestCase));
}

#[test]
fn constructors_are_methods() {
    let ex = extract(&fixture("src/main/java/com/example/report/Report.java"));
    let names: Vec<&str> = ex.defs.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"Report.Report"));
}

#[test]
fn extracts_junit_tests_with_fqcn_test_ids() {
    let ex = extract(&fixture("src/test/java/com/example/calc/CalcTest.java"));
    let ids: Vec<Vec<String>> = ex
        .defs
        .iter()
        .filter(|d| d.kind == DefKind::TestCase)
        .map(|d| d.test_id.clone().unwrap())
        .collect();

    assert!(ids.contains(&vec![
        "com.example.calc.CalcTest".to_string(),
        "addsNegatives".to_string()
    ]));
    assert!(ids.contains(&vec![
        "com.example.calc.CalcTest".to_string(),
        "multiplies".to_string()
    ]));
    // A @ParameterizedTest is still filterable by method name.
    assert!(ids.contains(&vec![
        "com.example.calc.CalcTest".to_string(),
        "addsZeroIdentity".to_string()
    ]));
    // A @Nested class contributes its simple name as a middle segment.
    assert!(ids.contains(&vec![
        "com.example.calc.CalcTest".to_string(),
        "WhenDoubling".to_string(),
        "doublesTheSum".to_string()
    ]));
}

/// Java test identity is always a static method name — there is no
/// template-literal equivalent — so nothing should ever come back
/// `computed_name`, including the parameterized and nested cases.
#[test]
fn java_tests_are_never_computed() {
    let ex = extract(&fixture("src/test/java/com/example/calc/CalcTest.java"));
    assert!(ex
        .defs
        .iter()
        .filter(|d| d.kind == DefKind::TestCase)
        .all(|d| !d.computed_name));
}

#[test]
fn fully_qualified_test_annotation_still_counts() {
    let src = r#"package com.foo;
class BarTest {
    @org.junit.jupiter.api.Test
    void works() {}
}
"#;
    let ex = extract(src);
    let ids: Vec<Vec<String>> = ex
        .defs
        .iter()
        .filter(|d| d.kind == DefKind::TestCase)
        .map(|d| d.test_id.clone().unwrap())
        .collect();
    assert_eq!(ids, vec![vec!["com.foo.BarTest", "works"]]);
}

/// An unannotated method in a test class is a plain `Method`, not a test:
/// selecting it as a runnable test would print a command that matches
/// nothing.
#[test]
fn helper_methods_in_test_classes_are_not_tests() {
    let src = r#"package com.foo;
import org.junit.jupiter.api.Test;
class BarTest {
    private int helper() { return 1; }
    @Test
    void works() {}
}
"#;
    let ex = extract(src);
    let tests: Vec<&str> = ex
        .defs
        .iter()
        .filter(|d| d.kind == DefKind::TestCase)
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(tests, vec!["BarTest.works"]);
    assert!(ex
        .defs
        .iter()
        .any(|d| d.name == "BarTest.helper" && d.kind == DefKind::Method));
}

#[test]
fn default_package_test_id_has_no_prefix() {
    let src = r#"import org.junit.jupiter.api.Test;
class BarTest {
    @Test
    void works() {}
}
"#;
    let ex = extract(src);
    let ids: Vec<Vec<String>> = ex
        .defs
        .iter()
        .filter(|d| d.kind == DefKind::TestCase)
        .map(|d| d.test_id.clone().unwrap())
        .collect();
    assert_eq!(ids, vec![vec!["BarTest", "works"]]);
}

#[test]
fn collects_imports_including_wildcard_and_static() {
    let src = r#"package com.foo;

import com.foo.core.Calc;
import com.foo.util.*;
import static org.junit.jupiter.api.Assertions.assertEquals;

class BarTest {}
"#;
    let ex = extract(src);
    let raws: Vec<&str> = ex.imports.iter().map(|i| i.raw.as_str()).collect();
    assert_eq!(
        raws,
        vec![
            "com.foo.core.Calc",
            "com.foo.util.*",
            "org.junit.jupiter.api.Assertions.assertEquals"
        ]
    );
}

/// The `@Test` annotation names a JUnit type, not a symbol this file
/// references; treating it as a read would attach a bogus edge to every
/// test method.
#[test]
fn annotation_names_are_not_reads() {
    let src = r#"package com.foo;
import org.junit.jupiter.api.Test;
class BarTest {
    @Test
    void works() {}
}
"#;
    let ex = extract(src);
    assert!(
        !ex.reads.iter().any(|r| r.name == "Test"),
        "annotation name leaked into reads: {:?}",
        ex.reads
    );
}

/// An annotation's *arguments*, unlike its name, hold real references —
/// `@ExtendWith(MockitoExtension.class)` is a genuine dependency.
#[test]
fn annotation_arguments_are_still_scanned() {
    let src = r#"package com.foo;
import org.junit.jupiter.api.extension.ExtendWith;
class BarTest {
    @ExtendWith(MockitoExtension.class)
    void works() {}
}
"#;
    let ex = extract(src);
    assert!(
        ex.reads.iter().any(|r| r.name == "MockitoExtension"),
        "annotation argument type was dropped: {:?}",
        ex.reads
    );
}

#[test]
fn package_key_spans_parallel_source_roots() {
    let lang = JavaLanguage::default();
    let main = lang.package_key(Path::new("src/main/java/com/example/calc/Calc.java"));
    let test = lang.package_key(Path::new("src/test/java/com/example/calc/CalcTest.java"));
    assert_eq!(main, test);
    assert_eq!(main, Some(PathBuf::from("com/example/calc")));
}

#[test]
fn package_key_keeps_modules_apart() {
    let lang = JavaLanguage::default();
    let billing = lang.package_key(Path::new("services/billing/src/main/java/com/foo/A.java"));
    let ledger = lang.package_key(Path::new("services/ledger/src/main/java/com/foo/A.java"));
    assert_ne!(billing, ledger);
    assert_eq!(billing, Some(PathBuf::from("services/billing/com/foo")));
}

#[test]
fn package_key_falls_back_to_directory_outside_source_roots() {
    let lang = JavaLanguage::default();
    assert_eq!(
        lang.package_key(Path::new("scripts/Tool.java")),
        Some(PathBuf::from("scripts"))
    );
}

// --- containment rule (discovered on spring-boot / gson) -----------------

fn parent_name(ex: &testless_core::Extraction, name: &str) -> Option<String> {
    let d = ex.defs.iter().find(|d| d.name == name)?;
    d.parent.map(|p| ex.defs[p].name.clone())
}

/// A framework-annotated member is invoked by a container, never by name, so
/// it must hang off its class — otherwise a change to it reaches no test at
/// all. Regression for a real spring-boot under-select: editing a `@Bean`
/// body selected 0 of 17,720 tests.
#[test]
fn framework_annotated_members_parent_to_their_class() {
    let src = r#"package com.foo;
class Config {
    @Bean
    JsonFactory jsonFactory() { return new JsonFactory(); }

    @Autowired
    private Helper helper;
}
"#;
    let ex = extract(src);
    assert_eq!(
        parent_name(&ex, "Config.jsonFactory").as_deref(),
        Some("Config")
    );
    assert_eq!(parent_name(&ex, "Config#helper").as_deref(), Some("Config"));
}

/// `@Override` is on a huge share of Java methods and means nothing to any
/// runtime, so it must NOT imply container invocation. Regression for a real
/// gson over-select: counting it took a leaf edit from 6 to 1501 of 1534.
#[test]
fn inert_annotations_do_not_imply_containment() {
    let src = r#"package com.foo;
class Impl {
    @Override
    public String toString() { return "x"; }

    @SuppressWarnings("unchecked")
    void unchecked() {}

    void plain() {}
}
"#;
    let ex = extract(src);
    for m in ["Impl.toString", "Impl.unchecked", "Impl.plain"] {
        assert_eq!(parent_name(&ex, m), None, "{m} should not parent to class");
    }
}

/// Fields become defs, named `Class#field` rather than `Class.field`: the
/// indexer keys on the segment after the last `.`, so the dotted form would
/// be indexed as bare `field` and two classes' same-named fields would
/// cross-link.
#[test]
fn fields_become_class_scoped_defs() {
    let src = r#"package com.foo;
class T {
    private final Runner contextRunner = new Runner();
    private int a = 1, b = 2;
}
"#;
    let ex = extract(src);
    let names: Vec<&str> = ex.defs.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"T#contextRunner"), "{names:?}");
    // One def per declarator, not per statement.
    assert!(names.contains(&"T#a"), "{names:?}");
    assert!(names.contains(&"T#b"), "{names:?}");
}
