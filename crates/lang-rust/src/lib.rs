use std::path::{Path, PathBuf};

use testless_core::{DefKind, ExtractedDef, Extraction, Language};
use tree_sitter::Node;

pub struct RustLanguage;

impl Language for RustLanguage {
    fn id(&self) -> &'static str {
        "rust"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn grammar(&self, _path: &Path) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn extract(&self, src: &str, tree: &tree_sitter::Tree) -> Extraction {
        let root = tree.root_node();
        let src_bytes = src.as_bytes();
        let mut defs = Vec::new();

        // One ModuleInit `<module>` per file, always, spanning the whole
        // file.
        defs.push(ExtractedDef {
            name: "<module>".to_string(),
            kind: DefKind::ModuleInit,
            start_line: root.start_position().row as u32 + 1,
            end_line: root.end_position().row as u32 + 1,
            test_id: None,
            computed_name: false,
            parent: None,
        });

        let mut mod_stack: Vec<String> = Vec::new();
        walk_items(root, src_bytes, &mut defs, &mut mod_stack);

        // Imports (`use`/`mod x;`) are handled in a later task; none
        // collected here.
        Extraction { defs, imports: Vec::new() }
    }

    fn resolve_import(&self, _from_file: &Path, _raw: &str, _repo_root: &Path) -> Option<PathBuf> {
        None
    }
}

/// Single dispatch point for the walker: every item kind this extractor
/// cares about is matched here, so later tasks (test-case detection,
/// imports) extend this match rather than rewriting the walk.
///
/// `attribute_item` nodes are siblings that precede the item they
/// annotate (not children of it), so we accumulate them in `pending_attrs`
/// as we walk and hand them to the next real item. `mod_stack` carries the
/// chain of enclosing inline-`mod` names, used to build `test_id`s.
fn walk_items(node: Node, src: &[u8], defs: &mut Vec<ExtractedDef>, mod_stack: &mut Vec<String>) {
    let mut cursor = node.walk();
    let mut pending_attrs: Vec<Node> = Vec::new();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "attribute_item" => pending_attrs.push(child),
            // Comments between an attribute and the item it annotates
            // don't break the association.
            "line_comment" | "block_comment" => {}
            "function_item" => {
                handle_function_item(child, src, defs, mod_stack, &pending_attrs);
                pending_attrs.clear();
            }
            "impl_item" => {
                handle_impl_item(child, src, defs);
                pending_attrs.clear();
            }
            "struct_item" | "enum_item" | "trait_item" => {
                handle_type_item(child, src, defs);
                pending_attrs.clear();
            }
            "mod_item" => {
                handle_mod_item(child, src, defs, mod_stack);
                pending_attrs.clear();
            }
            _ => pending_attrs.clear(),
        }
    }
}

fn handle_function_item(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    mod_stack: &[String],
    attrs: &[Node],
) {
    let Some(name_node) = node.child_by_field_name("name") else { return };
    let Ok(name) = name_node.utf8_text(src) else { return };

    if attrs.iter().any(|attr| is_test_attribute(*attr, src)) {
        let mut test_id = mod_stack.to_vec();
        test_id.push(name.to_string());
        push_test_def(node, name.to_string(), test_id, defs);
    } else {
        push_def(node, name.to_string(), DefKind::Function, defs);
    }
}

/// Whether an `attribute_item` node marks its function as a test: its
/// path's last `::` segment is `test` or `bench` (covers `#[test]`,
/// `#[tokio::test]`, `#[bench]`), or the whole path is exactly `rstest`
/// (whose own last segment is `rstest`, not `test`).
fn is_test_attribute(attr_item: Node, src: &[u8]) -> bool {
    let Some(attribute) = attr_item.named_child(0) else { return false };
    let Some(path_node) = attribute.named_child(0) else { return false };
    let Ok(path) = path_node.utf8_text(src) else { return false };
    path == "rstest" || matches!(path.rsplit("::").next(), Some("test") | Some("bench"))
}

/// `struct_item` / `enum_item` / `trait_item` -> Class, named by the `name`
/// field.
fn handle_type_item(node: Node, src: &[u8], defs: &mut Vec<ExtractedDef>) {
    let Some(name_node) = node.child_by_field_name("name") else { return };
    let Ok(name) = name_node.utf8_text(src) else { return };
    push_def(node, name.to_string(), DefKind::Class, defs);
}

/// `impl_item`: each `function_item` in its `declaration_list` body becomes
/// a Method named `Type.method`, where `Type` is the impl's `type` field
/// text with any generic args (`<...>`) stripped.
fn handle_impl_item(node: Node, src: &[u8], defs: &mut Vec<ExtractedDef>) {
    let Some(type_node) = node.child_by_field_name("type") else { return };
    let Ok(type_text) = type_node.utf8_text(src) else { return };
    let type_name = strip_generics(type_text);

    let Some(body) = node.child_by_field_name("body") else { return };
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() == "function_item" {
            let Some(name_node) = child.child_by_field_name("name") else { continue };
            let Ok(method_name) = name_node.utf8_text(src) else { continue };
            let full_name = format!("{type_name}.{method_name}");
            push_def(child, full_name, DefKind::Method, defs);
        }
    }
}

/// `mod_item` with an inline body: recurse into its `declaration_list` so
/// defs nested in inline modules are extracted flat (`mod x;` without a
/// body is an import, handled in a later task, and skipped here since it
/// has no `body` field to recurse into). Pushes its name onto `mod_stack`
/// for the duration of the recursion so nested test fns get the full
/// enclosing-mod chain in their `test_id`.
fn handle_mod_item(node: Node, src: &[u8], defs: &mut Vec<ExtractedDef>, mod_stack: &mut Vec<String>) {
    let Some(body) = node.child_by_field_name("body") else { return };
    let Some(name_node) = node.child_by_field_name("name") else { return };
    let Ok(name) = name_node.utf8_text(src) else { return };

    mod_stack.push(name.to_string());
    walk_items(body, src, defs, mod_stack);
    mod_stack.pop();
}

/// Strip generic parameters from a type's text, e.g. `Calc<T>` -> `Calc`.
fn strip_generics(type_text: &str) -> &str {
    match type_text.find('<') {
        Some(idx) => &type_text[..idx],
        None => type_text,
    }
}

fn push_def(span: Node, name: String, kind: DefKind, defs: &mut Vec<ExtractedDef>) {
    defs.push(ExtractedDef {
        name,
        kind,
        start_line: span.start_position().row as u32 + 1,
        end_line: span.end_position().row as u32 + 1,
        test_id: None,
        computed_name: false,
        parent: None,
    });
}

fn push_test_def(span: Node, name: String, test_id: Vec<String>, defs: &mut Vec<ExtractedDef>) {
    defs.push(ExtractedDef {
        name,
        kind: DefKind::TestCase,
        start_line: span.start_position().row as u32 + 1,
        end_line: span.end_position().row as u32 + 1,
        test_id: Some(test_id),
        computed_name: false,
        parent: None,
    });
}
