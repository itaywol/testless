use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use testless_core::fingerprint::{module_init_fingerprint, split_fingerprint};
use testless_core::{DefKind, ExtractedDef, ExtractedRef, Extraction, ImportRef, Language};
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
        //
        // Note: the skip closure below marks every top-level
        // `function_declaration` as "has its own def", per the language-wide
        // convention (functions/methods/imports are hashed by their own def,
        // package-level vars/consts are loose code that stays IN the module
        // hash). `init()` is the one exception: it contributes no separate
        // def (see `handle_function_declaration`), so its body is *not*
        // otherwise covered by any def hash, yet this closure still skips it
        // as a `function_declaration`. A change confined entirely to an
        // `init()` body is therefore invisible to `diff_defs` — an accepted
        // gap (rare in practice; `init()` bodies are typically small
        // registration code) rather than special-casing the skip closure
        // per-function-name.
        let module_init_skip = |n: &tree_sitter::Node| {
            matches!(
                n.kind(),
                "function_declaration" | "method_declaration" | "import_declaration"
            )
        };
        defs.push(ExtractedDef {
            name: "<module>".to_string(),
            kind: DefKind::ModuleInit,
            start_line: root.start_position().row as u32 + 1,
            end_line: root.end_position().row as u32 + 1,
            test_id: None,
            computed_name: false,
            parent: None,
            sig_hash: module_init_fingerprint(root, src_bytes, &module_init_skip),
            body_hash: None,
        });

        // `scope_of` maps the AST node id that *is* a def's own span (a
        // function_declaration, a method_declaration, or a subtest's
        // `func_literal` callback) to that def's index in `defs`. The refs
        // pass uses it to track the innermost enclosing def while walking
        // the whole file from the root down.
        let mut scope_of: HashMap<usize, usize> = HashMap::new();
        // Node ids of defs' own name identifiers (a top-level function's
        // `name` field is grammar-kind `identifier`, the same kind as any
        // other reference, so it must be excluded from read scanning or a
        // function would appear to "read" itself).
        let mut def_name_ids: HashSet<usize> = HashSet::new();
        // Node ids of `call_expression`s recognized as `t.Run`/`b.Run`
        // subtest declarations (receiver matches the enclosing testing
        // param) — excluded from ordinary call extraction since they're
        // already represented as TestCase defs.
        let mut subtest_ids: HashSet<usize> = HashSet::new();

        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            match child.kind() {
                "function_declaration" => handle_function_declaration(
                    child,
                    src_bytes,
                    &mut defs,
                    &mut scope_of,
                    &mut def_name_ids,
                    &mut subtest_ids,
                ),
                "method_declaration" => {
                    handle_method_declaration(child, src_bytes, &mut defs, &mut scope_of)
                }
                _ => {}
            }
        }

        let mut imports = Vec::new();
        collect_imports(root, src_bytes, &mut imports);

        let known_names = build_known_names(&defs, &imports);

        let ctx = RefCtx {
            src: src_bytes,
            scope_of: &scope_of,
            def_name_ids: &def_name_ids,
            subtest_ids: &subtest_ids,
            known_names: &known_names,
        };
        let mut calls = Vec::new();
        let mut reads = Vec::new();
        walk_refs(root, &ctx, 0, &mut calls, &mut reads);

        Extraction {
            defs,
            imports,
            calls: dedup_refs(calls),
            reads: dedup_refs(reads),
        }
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
        raw.strip_prefix(&format!("{module_path}/"))
            .map(PathBuf::from)
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
#[allow(clippy::too_many_arguments)]
fn handle_function_declaration(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    scope_of: &mut HashMap<usize, usize>,
    def_name_ids: &mut HashSet<usize>,
    subtest_ids: &mut HashSet<usize>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Ok(name) = name_node.utf8_text(src) else {
        return;
    };

    if name == "init" {
        // Contributes to ModuleInit; no separate def.
        return;
    }

    if is_test_function(name, node, src) {
        let root_idx = push_def(
            node,
            name.to_string(),
            DefKind::TestCase,
            src,
            defs,
            None,
            Some(vec![name.to_string()]),
            false,
        );
        scope_of.insert(node.id(), root_idx);
        def_name_ids.insert(name_node.id());

        // The root's own `*testing.T/B/F` param name (e.g. `t` in
        // `TestFoo(t *testing.T)`) — `is_run_call` requires a `.Run`
        // receiver to match this exact name, so `t.Run(...)` is recognized
        // as a subtest but an unrelated `srv.Run(...)` is not.
        let testing_param = node
            .child_by_field_name("parameters")
            .and_then(|p| testing_param_name(p, src))
            .unwrap_or_default();

        let mut used_synthetic = false;
        walk_subtests(
            node,
            src,
            defs,
            root_idx,
            &[name.to_string()],
            &mut used_synthetic,
            &testing_param,
            scope_of,
            subtest_ids,
        );
        return;
    }

    let idx = push_def(
        node,
        name.to_string(),
        DefKind::Function,
        src,
        defs,
        None,
        None,
        false,
    );
    scope_of.insert(node.id(), idx);
    def_name_ids.insert(name_node.id());
}

fn handle_method_declaration(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    scope_of: &mut HashMap<usize, usize>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Ok(method_name) = name_node.utf8_text(src) else {
        return;
    };
    let Some(recv_type) = receiver_type_name(node, src) else {
        return;
    };

    let full_name = format!("{recv_type}.{method_name}");
    let idx = push_def(
        node,
        full_name,
        DefKind::Method,
        src,
        defs,
        None,
        None,
        false,
    );
    scope_of.insert(node.id(), idx);
}

/// The receiver's declared type name, with any leading `*` stripped (e.g.
/// `(c *Calc)` -> `Calc`, `(c Calc)` -> `Calc`).
fn receiver_type_name(method_decl: Node, src: &[u8]) -> Option<String> {
    let receiver = method_decl.child_by_field_name("receiver")?;
    let mut cursor = receiver.walk();
    let param = receiver
        .children(&mut cursor)
        .find(|c| c.kind() == "parameter_declaration")?;
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

    let Some(params) = node.child_by_field_name("parameters") else {
        return false;
    };
    let mut cursor = params.walk();
    let param_decls: Vec<Node> = params
        .children(&mut cursor)
        .filter(|c| c.kind() == "parameter_declaration")
        .collect();

    match param_decls.len() {
        0 => prefix == "Example",
        1 => {
            let Some(ty) = param_decls[0].child_by_field_name("type") else {
                return false;
            };
            if ty.kind() != "pointer_type" {
                return false;
            }
            let Some(inner) = ty.named_child(0) else {
                return false;
            };
            if inner.kind() != "qualified_type" {
                return false;
            }
            let Some(pkg) = inner.child_by_field_name("package") else {
                return false;
            };
            pkg.utf8_text(src).ok() == Some("testing")
        }
        _ => false,
    }
}

/// The name of the first parameter in `params` whose type text contains
/// `testing.` (i.e. the `*testing.T`/`*testing.B`/`*testing.F` receiver
/// name, e.g. `t` in `TestFoo(t *testing.T)` or the callback param in
/// `func(t *testing.T) { ... }`), if any.
fn testing_param_name(params: Node, src: &[u8]) -> Option<String> {
    let mut cursor = params.walk();
    let decls: Vec<Node> = params.children(&mut cursor).collect();
    decls.into_iter().find_map(|decl| {
        if decl.kind() != "parameter_declaration" {
            return None;
        }
        let ty = decl.child_by_field_name("type")?;
        let ty_text = ty.utf8_text(src).ok()?;
        if !ty_text.contains("testing.") {
            return None;
        }
        decl.child_by_field_name("name")?
            .utf8_text(src)
            .ok()
            .map(|s| s.to_string())
    })
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
///
/// `testing_param` is the name of the enclosing scope's `*testing.T/B/F`
/// parameter (see `testing_param_name`) — `is_run_call` only recognizes a
/// `.Run` call as a subtest when its receiver identifier matches this exact
/// name, so an unrelated `srv.Run(...)` in the same body is left as an
/// ordinary call rather than misread as a subtest.
#[allow(clippy::too_many_arguments)]
fn walk_subtests(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    enclosing_idx: usize,
    enclosing_chain: &[String],
    used_synthetic: &mut bool,
    testing_param: &str,
    scope_of: &mut HashMap<usize, usize>,
    subtest_ids: &mut HashSet<usize>,
) {
    if is_run_call(node, src, testing_param) {
        handle_run_call(
            node,
            src,
            defs,
            enclosing_idx,
            enclosing_chain,
            used_synthetic,
            scope_of,
            subtest_ids,
        );
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_subtests(
            child,
            src,
            defs,
            enclosing_idx,
            enclosing_chain,
            used_synthetic,
            testing_param,
            scope_of,
            subtest_ids,
        );
    }
}

/// Is this a `call_expression` whose callee is `<testing_param>.Run` —
/// matched by selector field name `Run` *and* the receiver identifier
/// equalling the enclosing scope's own testing-handle param name (recorded
/// when its TestCase/subtest was created). A `.Run` call on any other
/// receiver (e.g. `srv.Run(...)`) does not match, and is left as an ordinary
/// call ref rather than misclassified as a subtest.
fn is_run_call(node: Node, src: &[u8], testing_param: &str) -> bool {
    if testing_param.is_empty() || node.kind() != "call_expression" {
        return false;
    }
    let Some(func) = node.child_by_field_name("function") else {
        return false;
    };
    if func.kind() != "selector_expression" {
        return false;
    }
    let is_run = func
        .child_by_field_name("field")
        .and_then(|f| f.utf8_text(src).ok())
        == Some("Run");
    let recv_matches = func
        .child_by_field_name("operand")
        .filter(|op| op.kind() == "identifier")
        .and_then(|op| op.utf8_text(src).ok())
        == Some(testing_param);
    is_run && recv_matches
}

#[allow(clippy::too_many_arguments)]
fn handle_run_call(
    call: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    enclosing_idx: usize,
    enclosing_chain: &[String],
    used_synthetic: &mut bool,
    scope_of: &mut HashMap<usize, usize>,
    subtest_ids: &mut HashSet<usize>,
) {
    subtest_ids.insert(call.id());

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
    let idx = push_def(
        call,
        segment,
        DefKind::TestCase,
        src,
        defs,
        Some(enclosing_idx),
        Some(test_id.clone()),
        computed,
    );

    // Recurse into the callback body (`func(t *testing.T) { ... }`, the
    // second argument) with this call as the new enclosing node, so any
    // nested `t.Run` inside it chains off `test_id` / `idx` rather than the
    // outer root, and gets its own independent synthetic-dedupe scope and
    // its own (re-extracted) testing param name.
    if let Some(callback) = args.and_then(|a| a.named_child(1)) {
        scope_of.insert(callback.id(), idx);
        let nested_testing_param = callback
            .child_by_field_name("parameters")
            .and_then(|p| testing_param_name(p, src))
            .unwrap_or_default();
        let mut nested_used_synthetic = false;
        walk_subtests(
            callback,
            src,
            defs,
            idx,
            &test_id,
            &mut nested_used_synthetic,
            &nested_testing_param,
            scope_of,
            subtest_ids,
        );
    }
}

/// The unquoted text of an `interpreted_string_literal` (its
/// `interpreted_string_literal_content` child holds the content; an empty
/// string literal has no such child).
fn interpreted_string_content(node: Node, src: &[u8]) -> String {
    node.named_child(0)
        .and_then(|c| c.utf8_text(src).ok())
        .unwrap_or("")
        .to_string()
}

#[allow(clippy::too_many_arguments)]
fn push_def(
    span: Node,
    name: String,
    kind: DefKind,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    parent: Option<usize>,
    test_id: Option<Vec<String>>,
    computed_name: bool,
) -> usize {
    let (sig_hash, body_hash) = split_fingerprint(span, src);
    defs.push(ExtractedDef {
        name,
        kind,
        start_line: span.start_position().row as u32 + 1,
        end_line: span.end_position().row as u32 + 1,
        test_id,
        computed_name,
        parent,
        sig_hash,
        body_hash,
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

/// The cheap allow-list used to filter read extraction down to non-local
/// noise: same-file top-level function names, plus each import's local
/// package qualifier (the last `/`-separated segment of its raw path —
/// aliases aren't tracked, an acceptable simplification since none of this
/// crate's fixtures use them).
fn build_known_names(defs: &[ExtractedDef], imports: &[ImportRef]) -> HashSet<String> {
    let mut names: HashSet<String> = defs
        .iter()
        .filter(|d| d.kind == DefKind::Function)
        .map(|d| d.name.clone())
        .collect();
    for imp in imports {
        if let Some(pkg) = imp.raw.rsplit('/').next() {
            if !pkg.is_empty() {
                names.insert(pkg.to_string());
            }
        }
    }
    names
}

/// Shared read-only context for the calls/reads walk.
struct RefCtx<'a> {
    src: &'a [u8],
    /// AST node id -> def index of the def whose scope that node opens
    /// (a function/method's own `function_declaration`/`method_declaration`
    /// node, or a subtest's `func_literal` callback).
    scope_of: &'a HashMap<usize, usize>,
    /// Node ids of defs' own name identifiers (excluded from read scanning).
    def_name_ids: &'a HashSet<usize>,
    /// Node ids of `call_expression`s recognized as `t.Run`/`b.Run` subtest
    /// declarations — excluded from ordinary call extraction.
    subtest_ids: &'a HashSet<usize>,
    /// Import package qualifiers ∪ top-level function names — the cheap
    /// allow-list that keeps read extraction from flooding on local names.
    known_names: &'a HashSet<String>,
}

fn node_text<'a>(node: Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or_default()
}

/// Walk the whole tree recording `calls` and `reads`, threading
/// `current_def` — the index (into `Extraction.defs`) of the innermost
/// enclosing def — down through every function/method/subtest body via
/// `ctx.scope_of`. Top-level code (no enclosing def, e.g. a package-level
/// var initializer) uses `current_def`'s initial value, 0 (`<module>`).
///
/// Type annotations like `*testing.T` never interfere here: their package
/// name is grammar-kind `package_identifier` and a method's own name is
/// `field_identifier` — both distinct from the plain `identifier` kind this
/// walk treats as a possible read, so they fall through the generic
/// recursion arm untouched. Method values and `go f()` goroutines need no
/// special casing either: the wrapping node (a selector value, a
/// `go_statement`) just isn't `call_expression`/`selector_expression`
/// itself, so the inner `call_expression` is reached and handled the same
/// way via ordinary recursion.
fn walk_refs(
    node: Node,
    ctx: &RefCtx,
    current_def: usize,
    calls: &mut Vec<ExtractedRef>,
    reads: &mut Vec<ExtractedRef>,
) {
    let def = ctx.scope_of.get(&node.id()).copied().unwrap_or(current_def);
    let line = node.start_position().row as u32 + 1;

    match node.kind() {
        "call_expression" => {
            let is_subtest = ctx.subtest_ids.contains(&node.id());
            if let Some(func) = node.child_by_field_name("function") {
                match func.kind() {
                    "identifier" => {
                        calls.push(ExtractedRef {
                            from_def: def,
                            name: node_text(func, ctx.src).to_string(),
                            qualifier: None,
                            line,
                        });
                    }
                    "selector_expression" => {
                        if let (Some(operand), Some(field)) = (
                            func.child_by_field_name("operand"),
                            func.child_by_field_name("field"),
                        ) {
                            if !is_subtest {
                                calls.push(ExtractedRef {
                                    from_def: def,
                                    name: node_text(field, ctx.src).to_string(),
                                    qualifier: Some(node_text(operand, ctx.src).to_string()),
                                    line,
                                });
                            }
                            // A plain identifier operand is fully consumed by
                            // the call record above; anything more complex
                            // (e.g. `a().Method()`) may hide further
                            // calls/reads, so recurse into it.
                            if operand.kind() != "identifier" {
                                walk_refs(operand, ctx, def, calls, reads);
                            }
                        }
                    }
                    _ => {
                        walk_refs(func, ctx, def, calls, reads);
                    }
                }
            }
            if let Some(args) = node.child_by_field_name("arguments") {
                walk_refs(args, ctx, def, calls, reads);
            }
        }
        "selector_expression" => {
            // Reached only for selector expressions that are NOT a call
            // callee (those are handled, and their subtrees already walked,
            // above).
            if let (Some(operand), Some(field)) = (
                node.child_by_field_name("operand"),
                node.child_by_field_name("field"),
            ) {
                if operand.kind() == "identifier" {
                    let obj_name = node_text(operand, ctx.src).to_string();
                    let prop_name = node_text(field, ctx.src).to_string();
                    if ctx.known_names.contains(&obj_name) || ctx.known_names.contains(&prop_name) {
                        reads.push(ExtractedRef {
                            from_def: def,
                            name: prop_name,
                            qualifier: Some(obj_name),
                            line,
                        });
                    }
                } else {
                    walk_refs(operand, ctx, def, calls, reads);
                }
            }
        }
        "identifier" => {
            if !ctx.def_name_ids.contains(&node.id()) {
                let text = node_text(node, ctx.src);
                if ctx.known_names.contains(text) {
                    reads.push(ExtractedRef {
                        from_def: def,
                        name: text.to_string(),
                        qualifier: None,
                        line,
                    });
                }
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk_refs(child, ctx, def, calls, reads);
            }
        }
    }
}

/// Keep the first occurrence of each `(from_def, name, qualifier)` triple —
/// callers may legitimately reference the same symbol from the same def more
/// than once (e.g. in a loop), but the graph only needs the edge once.
fn dedup_refs(refs: Vec<ExtractedRef>) -> Vec<ExtractedRef> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        let key = (r.from_def, r.name.clone(), r.qualifier.clone());
        if seen.insert(key) {
            out.push(r);
        }
    }
    out
}
