use std::path::{Component, Path, PathBuf};

use testless_core::{DefKind, ExtractedDef, Extraction, ImportRef, Language};
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

        let mut stack: Vec<(String, bool)> = Vec::new();
        walk_tests(root, src_bytes, &mut defs, &mut stack);

        let mut imports = Vec::new();
        collect_imports(root, src_bytes, &mut imports);

        Extraction { defs, imports }
    }

    fn resolve_import(&self, from_file: &Path, raw: &str, repo_root: &Path) -> Option<PathBuf> {
        if !(raw.starts_with("./") || raw.starts_with("../")) {
            return None;
        }

        let from_dir = from_file.parent().unwrap_or_else(|| Path::new(""));
        let base = normalize_path(&from_dir.join(raw));
        let base_str = base.to_string_lossy().to_string();

        let known_exts = [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx"];
        let raw_has_known_ext = known_exts.iter().any(|ext| raw.ends_with(ext));

        let mut candidates: Vec<PathBuf> = vec![base.clone()];
        if !raw_has_known_ext {
            for ext in known_exts {
                candidates.push(PathBuf::from(format!("{base_str}{ext}")));
            }
        }
        if let Some(stripped) = base_str.strip_suffix(".js") {
            candidates.push(PathBuf::from(format!("{stripped}.ts")));
        } else if let Some(stripped) = base_str.strip_suffix(".jsx") {
            candidates.push(PathBuf::from(format!("{stripped}.tsx")));
        }
        candidates.push(PathBuf::from(format!("{base_str}/index.ts")));
        candidates.push(PathBuf::from(format!("{base_str}/index.tsx")));

        candidates.into_iter().find(|c| repo_root.join(c).exists())
    }
}

/// Lexically collapse `.` and `..` components (no filesystem access), so
/// joined relative-import paths compare equal to their canonical form.
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Extract the raw text of a `string` node (its `string_fragment` child holds
/// the content with quotes already stripped).
fn string_literal_text(node: Node, src: &[u8]) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    Some(
        node.named_child(0)
            .and_then(|f| f.utf8_text(src).ok())
            .unwrap_or("")
            .to_string(),
    )
}

/// Recursively collect `import_statement` sources and `export_statement`
/// re-export sources (`export * from "..."`, `export { x } from "..."`).
/// Over-approximation is fine: a superfluous ImportRef only widens impact,
/// never narrows it.
fn collect_imports(node: Node, src: &[u8], imports: &mut Vec<ImportRef>) {
    if matches!(node.kind(), "import_statement" | "export_statement") {
        if let Some(source) = node.child_by_field_name("source") {
            if let Some(raw) = string_literal_text(source, src) {
                imports.push(ImportRef {
                    raw,
                    line: node.start_position().row as u32 + 1,
                });
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_imports(child, src, imports);
    }
}

enum TestCallKind {
    Describe,
    Leaf,
}

/// A `call_expression` recognized as a test-framework declaration.
struct TestCallMatch {
    kind: TestCallKind,
    /// For curried `it.each(table)("name", fn)` / `describe.each(...)(...)`
    /// forms, the id of the inner call node (`it.each(table)`) — it must be
    /// skipped during generic recursion so it isn't independently
    /// re-classified as its own (spurious) leaf/describe call.
    skip_child_id: Option<usize>,
    /// Curried `.each(...)` forms: the outer call's first argument is a
    /// printf-style template (e.g. `"adds %i and %i"`), not a stable literal
    /// id, so the segment is always forced to computed.
    curried: bool,
}

/// Classify a callee node (an `identifier` or `member_expression`) as
/// `describe` / `it` / `test`, covering member-expression forms like
/// `describe.skip`, `it.each`, `test.only`, etc.
fn callee_kind(callee: Node, src: &[u8]) -> Option<TestCallKind> {
    let base_name = match callee.kind() {
        "identifier" => callee.utf8_text(src).ok()?,
        "member_expression" => {
            let object = callee.child_by_field_name("object")?;
            if object.kind() != "identifier" {
                return None;
            }
            object.utf8_text(src).ok()?
        }
        _ => return None,
    };
    match base_name {
        "describe" => Some(TestCallKind::Describe),
        "it" | "test" => Some(TestCallKind::Leaf),
        _ => None,
    }
}

/// Classify a `call_expression` as a `describe`/`it`/`test` declaration,
/// including the curried `it.each(table)("name", fn)` form where the callee
/// itself is a `call_expression` (`it.each(table)`).
fn match_test_call(node: Node, src: &[u8]) -> Option<TestCallMatch> {
    let func = node.child_by_field_name("function")?;
    if let Some(kind) = callee_kind(func, src) {
        return Some(TestCallMatch {
            kind,
            skip_child_id: None,
            curried: false,
        });
    }
    if func.kind() == "call_expression" {
        let inner_func = func.child_by_field_name("function")?;
        let kind = callee_kind(inner_func, src)?;
        return Some(TestCallMatch {
            kind,
            skip_child_id: Some(func.id()),
            curried: true,
        });
    }
    None
}

/// The chain segment contributed by this call's first argument: the literal
/// string content, or `("<computed>", true)` for template literals / any
/// non-string-literal first argument.
fn first_arg_segment(node: Node, src: &[u8]) -> (String, bool) {
    let arg = node
        .child_by_field_name("arguments")
        .and_then(|a| a.named_child(0));
    match arg {
        Some(n) if n.kind() == "string" => (string_literal_text(n, src).unwrap_or_default(), false),
        _ => ("<computed>".to_string(), true),
    }
}

/// Walk the whole tree tracking a describe-stack, emitting a `TestCase` def
/// for each `it`/`test` leaf with its full `test_id` chain. If any segment in
/// the chain is computed (template literal / non-literal first arg),
/// `computed_name` is set and the chain is truncated right after the last
/// literal segment (i.e. it stops at the first computed segment).
fn walk_tests(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    stack: &mut Vec<(String, bool)>,
) {
    if node.kind() == "call_expression" {
        if let Some(m) = match_test_call(node, src) {
            let seg = if m.curried {
                ("<computed>".to_string(), true)
            } else {
                first_arg_segment(node, src)
            };
            match m.kind {
                TestCallKind::Describe => {
                    stack.push(seg);
                    walk_children_skipping(node, m.skip_child_id, src, defs, stack);
                    stack.pop();
                    return;
                }
                TestCallKind::Leaf => {
                    let mut full = stack.clone();
                    full.push(seg);
                    let computed_name = full.iter().any(|(_, computed)| *computed);
                    let test_id: Vec<String> = full
                        .iter()
                        .take_while(|(_, computed)| !*computed)
                        .map(|(name, _)| name.clone())
                        .collect();
                    defs.push(ExtractedDef {
                        name: full.last().unwrap().0.clone(),
                        kind: DefKind::TestCase,
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        test_id: Some(test_id),
                        computed_name,
                        parent: None,
                    });
                    walk_children_skipping(node, m.skip_child_id, src, defs, stack);
                    return;
                }
            }
        }
    }
    walk_children_skipping(node, None, src, defs, stack);
}

/// Recurse into every child of `node`, except the child (if any) matching
/// `skip_id` by node id — used to avoid re-classifying the inner call of a
/// curried `it.each(table)("name", fn)` form as its own spurious leaf/describe.
fn walk_children_skipping(
    node: Node,
    skip_id: Option<usize>,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    stack: &mut Vec<(String, bool)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if Some(child.id()) == skip_id {
            continue;
        }
        walk_tests(child, src, defs, stack);
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
                let Some(value) = declarator.child_by_field_name("value") else {
                    continue;
                };
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
