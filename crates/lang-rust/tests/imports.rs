use std::path::{Path, PathBuf};
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
fn collects_mod_decls_and_use_paths() {
    let src = std::fs::read_to_string("../../fixtures/rust-app/src/lib.rs").unwrap();
    let ex = extract(&src);
    let raws: Vec<&str> = ex.imports.iter().map(|i| i.raw.as_str()).collect();
    assert!(raws.contains(&"mod math"));
    assert!(raws.contains(&"mod fmt"));

    let src = std::fs::read_to_string("../../fixtures/rust-app/src/fmt.rs").unwrap();
    let ex = extract(&src);
    let raws: Vec<&str> = ex.imports.iter().map(|i| i.raw.as_str()).collect();
    assert!(raws.contains(&"use crate::math::add"));
}

#[test]
fn resolves_mod_decl_use_crate_and_external() {
    let root = Path::new("../../fixtures/rust-app");
    let l = RustLanguage;
    assert_eq!(
        l.resolve_import(Path::new("src/lib.rs"), "mod math", root),
        Some(PathBuf::from("src/math.rs"))
    );
    assert_eq!(
        l.resolve_import(Path::new("src/fmt.rs"), "use crate::math::add", root),
        Some(PathBuf::from("src/math.rs"))
    );
    assert_eq!(
        l.resolve_import(
            Path::new("tests/integration.rs"),
            "use rust_app::math::add",
            root
        ),
        None
    ); // external-crate-shaped: tier 1 skips (ModuleInit widening covers)
    assert_eq!(
        l.resolve_import(Path::new("src/fmt.rs"), "use serde::Serialize", root),
        None
    );
}
