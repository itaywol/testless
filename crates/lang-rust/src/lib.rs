use std::path::{Path, PathBuf};

use testless_core::{DefKind, ExtractedDef, Extraction, ImportRef, Language};
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
        let mut imports = Vec::new();
        walk_items(root, src_bytes, &mut defs, &mut mod_stack, &mut imports);

        Extraction { defs, imports }
    }

    /// Tier-1 (best-effort, over-approximating) resolution: only `mod x;`
    /// declarations and `use crate::.../use super::.../use self::...` paths
    /// resolve to a file; anything shaped like an external crate (`use
    /// std::...`, `use serde::...`, `use rust_app::...` from an integration
    /// test, etc.) returns `None` and falls through to the `ModuleInit`
    /// over-approximation upstream (every def in the changed file is
    /// impacted, so an unresolved import never causes a missed test).
    fn resolve_import(&self, from_file: &Path, raw: &str, repo_root: &Path) -> Option<PathBuf> {
        if let Some(mod_name) = raw.strip_prefix("mod ") {
            return resolve_mod_decl(from_file, mod_name, repo_root);
        }
        if let Some(path_text) = raw.strip_prefix("use ") {
            return resolve_use_path(from_file, path_text, repo_root);
        }
        None
    }
}

/// `mod X;` (no inline body): resolves relative to the *module* directory of
/// `from_file` -- which is `from_file`'s own dir when `from_file` is a module
/// root (`lib.rs`/`main.rs`/`mod.rs`), or `from_file`'s stem-dir otherwise
/// (`a/b.rs` -> `a/b/`, since `b`'s submodules live under `a/b/`).
fn resolve_mod_decl(from_file: &Path, mod_name: &str, repo_root: &Path) -> Option<PathBuf> {
    let file_name = from_file.file_name()?.to_str()?;
    let parent = from_file.parent().unwrap_or_else(|| Path::new(""));
    let dir: PathBuf = if matches!(file_name, "lib.rs" | "main.rs" | "mod.rs") {
        parent.to_path_buf()
    } else {
        parent.join(from_file.file_stem()?.to_str()?)
    };

    let file_candidate = dir.join(format!("{mod_name}.rs"));
    let mod_candidate = dir.join(mod_name).join("mod.rs");
    [file_candidate, mod_candidate]
        .into_iter()
        .find(|c| repo_root.join(c).exists())
}

/// `use <path>` resolution. Only `crate::`/`super::`/`self::`-rooted paths
/// are ours to resolve; anything else (an external crate, `std::...`, or an
/// integration test's `use rust_app::...`) is left to `None`. Grouped uses
/// (`use crate::{a, b}`) are handled best-effort: we only ever collected one
/// ImportRef for the whole group (see `handle_use_declaration`), so here we
/// just resolve the longest prefix that precedes the `{`.
fn resolve_use_path(from_file: &Path, path_text: &str, repo_root: &Path) -> Option<PathBuf> {
    let effective = match path_text.find('{') {
        Some(idx) => path_text[..idx].trim_end_matches("::"),
        None => path_text,
    };

    let mut segments = effective.split("::");
    let head = segments.next()?;
    let rest: Vec<&str> = segments.collect();

    let start_dir = match head {
        "crate" => find_crate_root(from_file, repo_root)?,
        // `super`/`self` both resolve relative to the enclosing file's own
        // module directory. This is a simplification for `mod.rs`-shaped
        // files (whose true parent module lives one level up) -- untested,
        // best-effort, and only ever widens (never wrongly narrows) impact
        // since an unresolved import falls back to the ModuleInit
        // over-approximation.
        "super" | "self" => from_file
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf(),
        _ => return None,
    };

    resolve_segments(&start_dir, &rest, repo_root)
}

/// Find the nearest ancestor directory of `from_file` (walking up toward
/// `repo_root`, inclusive of `from_file`'s own dir) that contains a
/// `lib.rs` or `main.rs` -- that's the `crate::` root.
fn find_crate_root(from_file: &Path, repo_root: &Path) -> Option<PathBuf> {
    let mut dir = from_file.parent().unwrap_or_else(|| Path::new(""));
    loop {
        if repo_root.join(dir).join("lib.rs").exists()
            || repo_root.join(dir).join("main.rs").exists()
        {
            return Some(dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent,
            _ => return None,
        }
    }
}

/// Longest-prefix-first file mapping: for segments `[seg1, seg2, ..., segN]`,
/// try (from the longest prefix down to just `seg1`) `root/seg1/.../segK.rs`
/// then `root/seg1/.../segK/mod.rs`; the first that exists wins. This
/// naturally skips trailing item names (`crate::math::add` finds
/// `root/math.rs` once the `add` segment fails to resolve as a file).
fn resolve_segments(root: &Path, segments: &[&str], repo_root: &Path) -> Option<PathBuf> {
    for take in (1..=segments.len()).rev() {
        let mut base = root.to_path_buf();
        for seg in &segments[..take] {
            base.push(seg);
        }
        let mut file_candidate = base.as_os_str().to_os_string();
        file_candidate.push(".rs");
        let file_candidate = PathBuf::from(file_candidate);
        let mod_candidate = base.join("mod.rs");
        if let Some(found) = [file_candidate, mod_candidate]
            .into_iter()
            .find(|c| repo_root.join(c).exists())
        {
            return Some(found);
        }
    }
    None
}

/// Single dispatch point for the walker: every item kind this extractor
/// cares about is matched here, so later tasks (test-case detection,
/// imports) extend this match rather than rewriting the walk.
///
/// `attribute_item` nodes are siblings that precede the item they
/// annotate (not children of it), so we accumulate them in `pending_attrs`
/// as we walk and hand them to the next real item. `mod_stack` carries the
/// chain of enclosing inline-`mod` names, used to build `test_id`s.
fn walk_items(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    mod_stack: &mut Vec<String>,
    imports: &mut Vec<ImportRef>,
) {
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
                handle_mod_item(child, src, defs, mod_stack, imports);
                pending_attrs.clear();
            }
            "use_declaration" => {
                handle_use_declaration(child, src, imports);
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
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Ok(name) = name_node.utf8_text(src) else {
        return;
    };

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
    let Some(attribute) = attr_item.named_child(0) else {
        return false;
    };
    let Some(path_node) = attribute.named_child(0) else {
        return false;
    };
    let Ok(path) = path_node.utf8_text(src) else {
        return false;
    };
    path == "rstest" || matches!(path.rsplit("::").next(), Some("test") | Some("bench"))
}

/// `struct_item` / `enum_item` / `trait_item` -> Class, named by the `name`
/// field.
fn handle_type_item(node: Node, src: &[u8], defs: &mut Vec<ExtractedDef>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Ok(name) = name_node.utf8_text(src) else {
        return;
    };
    push_def(node, name.to_string(), DefKind::Class, defs);
}

/// `impl_item`: each `function_item` in its `declaration_list` body becomes
/// a Method named `Type.method`, where `Type` is the impl's `type` field
/// text with any generic args (`<...>`) stripped.
fn handle_impl_item(node: Node, src: &[u8], defs: &mut Vec<ExtractedDef>) {
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let Ok(type_text) = type_node.utf8_text(src) else {
        return;
    };
    let type_name = strip_generics(type_text);

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() == "function_item" {
            let Some(name_node) = child.child_by_field_name("name") else {
                continue;
            };
            let Ok(method_name) = name_node.utf8_text(src) else {
                continue;
            };
            let full_name = format!("{type_name}.{method_name}");
            push_def(child, full_name, DefKind::Method, defs);
        }
    }
}

/// `mod_item` with an inline body: recurse into its `declaration_list` so
/// defs nested in inline modules are extracted flat. Pushes its name onto
/// `mod_stack` for the duration of the recursion so nested test fns get the
/// full enclosing-mod chain in their `test_id`. `mod x;` without a body is a
/// module *declaration* rather than a definition -- an import, collected as
/// an `ImportRef` (`"mod x"`) instead of recursed into.
fn handle_mod_item(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    mod_stack: &mut Vec<String>,
    imports: &mut Vec<ImportRef>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Ok(name) = name_node.utf8_text(src) else {
        return;
    };

    let Some(body) = node.child_by_field_name("body") else {
        imports.push(ImportRef {
            raw: format!("mod {name}"),
            line: node.start_position().row as u32 + 1,
        });
        return;
    };

    mod_stack.push(name.to_string());
    walk_items(body, src, defs, mod_stack, imports);
    mod_stack.pop();
}

/// `use_declaration`'s `argument` field holds the source text of everything
/// after `use ` (before the trailing `;`) verbatim -- covers simple paths
/// (`crate::math::add`) and grouped paths (`crate::{a, b}`) alike. A grouped
/// use gets a single ImportRef for the whole group; `resolve_use_path`
/// treats that as best-effort (longest resolvable prefix before the `{`).
fn handle_use_declaration(node: Node, src: &[u8], imports: &mut Vec<ImportRef>) {
    let Some(arg) = node.child_by_field_name("argument") else {
        return;
    };
    let Ok(text) = arg.utf8_text(src) else { return };
    imports.push(ImportRef {
        raw: format!("use {text}"),
        line: node.start_position().row as u32 + 1,
    });
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
