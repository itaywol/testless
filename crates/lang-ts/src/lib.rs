use std::path::{Path, PathBuf};

use pick_a_test_core::{DefKind, ExtractedDef, Extraction, Language};
use tree_sitter::Node;

pub struct TsLanguage;

impl Language for TsLanguage {
    fn id(&self) -> &'static str {
        "ts"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["ts", "tsx", "js", "jsx", "mts", "cts"]
    }

    fn grammar(&self, path: &Path) -> tree_sitter::Language {
        match path.extension().and_then(|e| e.to_str()) {
            Some("tsx") | Some("jsx") => tree_sitter_typescript::LANGUAGE_TSX.into(),
            _ => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
    }

    fn extract(&self, src: &str, tree: &tree_sitter::Tree) -> Extraction {
        let root = tree.root_node();
        let mut defs = Vec::new();

        // Always-present module-level def, spanning the whole file.
        defs.push(ExtractedDef {
            name: "<module>".to_string(),
            kind: DefKind::ModuleInit,
            start_line: root.start_position().row as u32 + 1,
            end_line: root.end_position().row as u32 + 1,
            test_id: None,
            computed_name: false,
            parent: None,
        });

        let src_bytes = src.as_bytes();
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            walk_top_level(child, src_bytes, &mut defs, None);
        }

        Extraction { defs, imports: vec![] }
    }

    fn resolve_import(&self, _from_file: &Path, _raw: &str, _repo_root: &Path) -> Option<PathBuf> {
        None
    }
}

/// Walk a top-level (or class-body) node, extracting defs. `parent` is the
/// index (into `defs`) of the enclosing class, if any. Extension point for
/// future def kinds (test cases, etc.) — add more match arms here rather
/// than rewriting the walk.
fn walk_top_level(node: Node, src: &[u8], defs: &mut Vec<ExtractedDef>, parent: Option<usize>) {
    match node.kind() {
        "export_statement" => {
            // Exports wrap their declaration (`export function f() {}` etc.);
            // recurse into it so exported defs aren't missed.
            if let Some(decl) = node.child_by_field_name("declaration") {
                walk_top_level(decl, src, defs, parent);
            }
        }
        "function_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                push_def(node, name, DefKind::Function, src, defs, parent);
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            let mut cursor = node.walk();
            for declarator in node.children(&mut cursor) {
                if declarator.kind() != "variable_declarator" {
                    continue;
                }
                let Some(value) = declarator.child_by_field_name("value") else { continue };
                if !matches!(value.kind(), "arrow_function" | "function_expression") {
                    continue;
                }
                if let Some(name) = declarator.child_by_field_name("name") {
                    push_def(node, name, DefKind::Function, src, defs, parent);
                }
            }
        }
        "class_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                let class_idx = push_def(node, name, DefKind::Class, src, defs, parent);
                if let Some(body) = node.child_by_field_name("body") {
                    let mut cursor = body.walk();
                    for member in body.children(&mut cursor) {
                        if member.kind() != "method_definition" {
                            continue;
                        }
                        if let Some(name) = member.child_by_field_name("name") {
                            push_def(member, name, DefKind::Method, src, defs, Some(class_idx));
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

fn push_def(
    span: Node,
    name_node: Node,
    kind: DefKind,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    parent: Option<usize>,
) -> usize {
    let name = name_node.utf8_text(src).unwrap_or_default().to_string();
    defs.push(ExtractedDef {
        name,
        kind,
        start_line: span.start_position().row as u32 + 1,
        end_line: span.end_position().row as u32 + 1,
        test_id: None,
        computed_name: false,
        parent,
    });
    defs.len() - 1
}
