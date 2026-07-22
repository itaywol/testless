use std::path::{Path, PathBuf};

use pick_a_test_core::{DefKind, ExtractedDef, Extraction, ImportRef, Language};
use tree_sitter::Node;

pub struct GoLanguage;

impl Language for GoLanguage {
    fn id(&self) -> &'static str {
        "go"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["go"]
    }

    fn grammar(&self, _path: &Path) -> tree_sitter::Language {
        tree_sitter_go::LANGUAGE.into()
    }

    fn extract(&self, src: &str, tree: &tree_sitter::Tree) -> Extraction {
        let root = tree.root_node();
        let src_bytes = src.as_bytes();
        let mut defs = Vec::new();

        // One ModuleInit `<module>` per file, always, spanning the whole
        // file (covers package-level vars and any `init()` functions, which
        // are folded into this rather than getting their own def).
        defs.push(ExtractedDef {
            name: "<module>".to_string(),
            kind: DefKind::ModuleInit,
            start_line: root.start_position().row as u32 + 1,
            end_line: root.end_position().row as u32 + 1,
            test_id: None,
            computed_name: false,
            parent: None,
        });

        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            match child.kind() {
                "function_declaration" => handle_function_declaration(child, src_bytes, &mut defs),
                "method_declaration" => handle_method_declaration(child, src_bytes, &mut defs),
                _ => {}
            }
        }

        let mut imports = Vec::new();
        collect_imports(root, src_bytes, &mut imports);

        Extraction { defs, imports }
    }

    fn resolve_import(&self, _from_file: &Path, raw: &str, repo_root: &Path) -> Option<PathBuf> {
        let go_mod = std::fs::read_to_string(repo_root.join("go.mod")).ok()?;
        let first_line = go_mod.lines().next()?;
        let module_path = first_line.strip_prefix("module ")?.trim();

        // Only the `<module>/<dir>` sub-package form resolves to a directory;
        // an import equal to the module path exactly (the module's own root
        // package) has no caller/fixture needing it here, so it's left
        // unhandled (falls through to `None`) rather than speculatively
        // guessed at.
        raw.strip_prefix(&format!("{module_path}/")).map(PathBuf::from)
    }
}

/// Handle a top-level `function_declaration`: `init` is folded into the
/// always-present `<module>` def (no separate Function def emitted); a
/// `Test*/Benchmark*/Example*/Fuzz*` function whose single parameter is
/// `*testing.T/B/F` (or a parameterless `Example*`) becomes a TestCase root,
/// with its body walked for `t.Run`/`b.Run` subtests; everything else is a
/// plain Function.
///
/// Note on test-file detection: the `Language::extract` contract takes no
/// file path, so we can't check the `_test.go` filename suffix directly.
/// Instead we detect test functions structurally, by name-prefix +
/// `*testing.T/B/F` parameter signature. This is a sound over-approximation:
/// a non-test file defining a function with this exact shape is vanishingly
/// rare, and treating it as a TestCase root when it isn't one is harmless
/// (it just makes that def eligible for selection, never excluded).
fn handle_function_declaration(node: Node, src: &[u8], defs: &mut Vec<ExtractedDef>) {
    let Some(name_node) = node.child_by_field_name("name") else { return };
    let Ok(name) = name_node.utf8_text(src) else { return };

    if name == "init" {
        // Contributes to ModuleInit; no separate def.
        return;
    }

    if is_test_function(name, node, src) {
        let root_idx = push_def(
            node,
            name.to_string(),
            DefKind::TestCase,
            defs,
            None,
            Some(vec![name.to_string()]),
            false,
        );
        let mut used_synthetic = false;
        walk_subtests(node, src, defs, root_idx, &[name.to_string()], &mut used_synthetic);
        return;
    }

    push_def(node, name.to_string(), DefKind::Function, defs, None, None, false);
}

fn handle_method_declaration(node: Node, src: &[u8], defs: &mut Vec<ExtractedDef>) {
    let Some(name_node) = node.child_by_field_name("name") else { return };
    let Ok(method_name) = name_node.utf8_text(src) else { return };
    let Some(recv_type) = receiver_type_name(node, src) else { return };

    let full_name = format!("{recv_type}.{method_name}");
    push_def(node, full_name, DefKind::Method, defs, None, None, false);
}

/// The receiver's declared type name, with any leading `*` stripped (e.g.
/// `(c *Calc)` -> `Calc`, `(c Calc)` -> `Calc`).
fn receiver_type_name(method_decl: Node, src: &[u8]) -> Option<String> {
    let receiver = method_decl.child_by_field_name("receiver")?;
    let mut cursor = receiver.walk();
    let param = receiver.children(&mut cursor).find(|c| c.kind() == "parameter_declaration")?;
    let ty = param.child_by_field_name("type")?;
    let type_ident = if ty.kind() == "pointer_type" {
        ty.named_child(0)?
    } else {
        ty
    };
    type_ident.utf8_text(src).ok().map(|s| s.to_string())
}

/// Does this top-level function's name + signature match Go's test-function
/// convention (`TestXxx(t *testing.T)`, `BenchmarkXxx(b *testing.B)`,
/// `FuzzXxx(f *testing.F)`, or parameterless `ExampleXxx()`)?
fn is_test_function(name: &str, node: Node, src: &[u8]) -> bool {
    let prefix = ["Test", "Benchmark", "Example", "Fuzz"]
        .iter()
        .find(|p| name.starts_with(**p));
    let Some(&prefix) = prefix else { return false };
    let rest = &name[prefix.len()..];
    if !(rest.is_empty() || rest.chars().next().unwrap().is_uppercase()) {
        return false;
    }

    let Some(params) = node.child_by_field_name("parameters") else { return false };
    let mut cursor = params.walk();
    let param_decls: Vec<Node> =
        params.children(&mut cursor).filter(|c| c.kind() == "parameter_declaration").collect();

    match param_decls.len() {
        0 => prefix == "Example",
        1 => {
            let Some(ty) = param_decls[0].child_by_field_name("type") else { return false };
            if ty.kind() != "pointer_type" {
                return false;
            }
            let Some(inner) = ty.named_child(0) else { return false };
            if inner.kind() != "qualified_type" {
                return false;
            }
            let Some(pkg) = inner.child_by_field_name("package") else { return false };
            pkg.utf8_text(src).ok() == Some("testing")
        }
        _ => false,
    }
}

/// Walk a TestCase's subtree for `t.Run(...)` / `b.Run(...)` calls,
/// emitting a child TestCase for each. A literal string first argument
/// contributes its own segment to the chain; anything else contributes a
/// single synthetic `<computed>` child (deduped via `used_synthetic`, scoped
/// to `enclosing_idx` — i.e. max one `<computed>` child per node, not per
/// root). `enclosing_idx`/`enclosing_chain` identify the *immediately*
/// enclosing subtest (which may itself be a nested `t.Run`, not necessarily
/// the top-level root), so chains and parents thread correctly through
/// arbitrary nesting: each `t.Run` match stops the generic recursion and
/// instead recurses into its own callback body with itself as the new
/// enclosing node and a fresh dedup flag, so sibling non-literal `Run`s
/// under different parents don't dedupe against each other.
fn walk_subtests(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    enclosing_idx: usize,
    enclosing_chain: &[String],
    used_synthetic: &mut bool,
) {
    if is_run_call(node, src) {
        handle_run_call(node, src, defs, enclosing_idx, enclosing_chain, used_synthetic);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_subtests(child, src, defs, enclosing_idx, enclosing_chain, used_synthetic);
    }
}

/// Is this a `call_expression` whose callee is `<something>.Run` (covers
/// both `t.Run` and `b.Run` — matched by selector field name alone,
/// regardless of the receiver variable's name).
fn is_run_call(node: Node, src: &[u8]) -> bool {
    node.kind() == "call_expression"
        && node
            .child_by_field_name("function")
            .filter(|f| f.kind() == "selector_expression")
            .and_then(|f| f.child_by_field_name("field"))
            .and_then(|field| field.utf8_text(src).ok())
            == Some("Run")
}

fn handle_run_call(
    call: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    enclosing_idx: usize,
    enclosing_chain: &[String],
    used_synthetic: &mut bool,
) {
    let args = call.child_by_field_name("arguments");
    let first_arg = args.and_then(|a| a.named_child(0));

    let (segment, computed) = match first_arg {
        Some(n) if n.kind() == "interpreted_string_literal" => {
            (interpreted_string_content(n, src), false)
        }
        _ => {
            if *used_synthetic {
                return;
            }
            *used_synthetic = true;
            ("<computed>".to_string(), true)
        }
    };

    let mut test_id = enclosing_chain.to_vec();
    test_id.push(segment.clone());
    let idx =
        push_def(call, segment, DefKind::TestCase, defs, Some(enclosing_idx), Some(test_id.clone()), computed);

    // Recurse into the callback body (`func(t *testing.T) { ... }`, the
    // second argument) with this call as the new enclosing node, so any
    // nested `t.Run` inside it chains off `test_id` / `idx` rather than the
    // outer root, and gets its own independent synthetic-dedupe scope.
    if let Some(callback) = args.and_then(|a| a.named_child(1)) {
        let mut nested_used_synthetic = false;
        walk_subtests(callback, src, defs, idx, &test_id, &mut nested_used_synthetic);
    }
}

/// The unquoted text of an `interpreted_string_literal` (its
/// `interpreted_string_literal_content` child holds the content; an empty
/// string literal has no such child).
fn interpreted_string_content(node: Node, src: &[u8]) -> String {
    node.named_child(0).and_then(|c| c.utf8_text(src).ok()).unwrap_or("").to_string()
}

#[allow(clippy::too_many_arguments)]
fn push_def(
    span: Node,
    name: String,
    kind: DefKind,
    defs: &mut Vec<ExtractedDef>,
    parent: Option<usize>,
    test_id: Option<Vec<String>>,
    computed_name: bool,
) -> usize {
    defs.push(ExtractedDef {
        name,
        kind,
        start_line: span.start_position().row as u32 + 1,
        end_line: span.end_position().row as u32 + 1,
        test_id,
        computed_name,
        parent,
    });
    defs.len() - 1
}

/// Recursively collect `import_declaration` specs: both the single-spec
/// form (`import "fmt"`) and the parenthesized list form (`import (...)`,
/// an `import_spec_list` of `import_spec`).
fn collect_imports(node: Node, src: &[u8], imports: &mut Vec<ImportRef>) {
    if node.kind() == "import_spec" {
        if let Some(path) = node.child_by_field_name("path") {
            if path.kind() == "interpreted_string_literal" {
                let raw = interpreted_string_content(path, src);
                imports.push(ImportRef { raw, line: node.start_position().row as u32 + 1 });
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_imports(child, src, imports);
    }
}
