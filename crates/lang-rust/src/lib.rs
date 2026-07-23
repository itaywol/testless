use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use testless_core::{DefKind, ExtractedDef, ExtractedRef, Extraction, ImportRef, Language};
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

        // `scope_of` maps the AST node id that *is* a def's own span (a
        // `function_item`, whether a plain fn, an impl method, or a
        // TestCase's fn) to that def's index in `defs`. The refs pass uses
        // it to track the innermost enclosing def while walking the whole
        // file from the root down. Struct/enum/trait items intentionally
        // get no entry: they aren't executable scopes (a trait's default
        // method body, the one exception, falls back to whatever encloses
        // the trait -- an accepted simplification, since trait items don't
        // get per-method Method defs of their own either).
        let mut scope_of: HashMap<usize, usize> = HashMap::new();
        // Node ids of defs' own name identifiers (a function/method's
        // `name` field is grammar-kind `identifier`, the same kind as any
        // other reference, so it must be excluded from read scanning or a
        // function would appear to "read" itself).
        let mut def_name_ids: HashSet<usize> = HashSet::new();

        let mut mod_stack: Vec<String> = Vec::new();
        let mut imports = Vec::new();
        walk_items(
            root,
            src_bytes,
            &mut defs,
            &mut mod_stack,
            &mut imports,
            &mut scope_of,
            &mut def_name_ids,
        );

        let known_names = build_known_names(&defs, &imports);

        let ctx = RefCtx {
            src: src_bytes,
            scope_of: &scope_of,
            def_name_ids: &def_name_ids,
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
#[allow(clippy::too_many_arguments)]
fn walk_items(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    mod_stack: &mut Vec<String>,
    imports: &mut Vec<ImportRef>,
    scope_of: &mut HashMap<usize, usize>,
    def_name_ids: &mut HashSet<usize>,
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
                handle_function_item(
                    child,
                    src,
                    defs,
                    mod_stack,
                    &pending_attrs,
                    scope_of,
                    def_name_ids,
                );
                pending_attrs.clear();
            }
            "impl_item" => {
                handle_impl_item(child, src, defs, scope_of, def_name_ids);
                pending_attrs.clear();
            }
            "struct_item" | "enum_item" | "trait_item" => {
                handle_type_item(child, src, defs, def_name_ids);
                pending_attrs.clear();
            }
            "mod_item" => {
                handle_mod_item(child, src, defs, mod_stack, imports, scope_of, def_name_ids);
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

#[allow(clippy::too_many_arguments)]
fn handle_function_item(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    mod_stack: &[String],
    attrs: &[Node],
    scope_of: &mut HashMap<usize, usize>,
    def_name_ids: &mut HashSet<usize>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Ok(name) = name_node.utf8_text(src) else {
        return;
    };

    let idx = if attrs.iter().any(|attr| is_test_attribute(*attr, src)) {
        let mut test_id = mod_stack.to_vec();
        test_id.push(name.to_string());
        push_test_def(node, name.to_string(), test_id, defs)
    } else {
        push_def(node, name.to_string(), DefKind::Function, defs)
    };
    scope_of.insert(node.id(), idx);
    def_name_ids.insert(name_node.id());
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
/// field. Note: this field's grammar kind is `type_identifier`, not
/// `identifier`, so it can never collide with the refs pass's plain
/// `identifier`-based read scanning -- recording it in `def_name_ids` is
/// therefore a no-op today, kept only for symmetry with the function/method
/// case should that ever change.
fn handle_type_item(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    def_name_ids: &mut HashSet<usize>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Ok(name) = name_node.utf8_text(src) else {
        return;
    };
    push_def(node, name.to_string(), DefKind::Class, defs);
    def_name_ids.insert(name_node.id());
}

/// `impl_item`: each `function_item` in its `declaration_list` body becomes
/// a Method named `Type.method`, where `Type` is the impl's `type` field
/// text with any generic args (`<...>`) stripped. Each method's own
/// `function_item` node is recorded in `scope_of` so calls/reads in its body
/// attribute to the Method def, not to whatever encloses the `impl` block.
fn handle_impl_item(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    scope_of: &mut HashMap<usize, usize>,
    def_name_ids: &mut HashSet<usize>,
) {
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
            let idx = push_def(child, full_name, DefKind::Method, defs);
            scope_of.insert(child.id(), idx);
            def_name_ids.insert(name_node.id());
        }
    }
}

/// `mod_item` with an inline body: recurse into its `declaration_list` so
/// defs nested in inline modules are extracted flat. Pushes its name onto
/// `mod_stack` for the duration of the recursion so nested test fns get the
/// full enclosing-mod chain in their `test_id`. `mod x;` without a body is a
/// module *declaration* rather than a definition -- an import, collected as
/// an `ImportRef` (`"mod x"`) instead of recursed into.
#[allow(clippy::too_many_arguments)]
fn handle_mod_item(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    mod_stack: &mut Vec<String>,
    imports: &mut Vec<ImportRef>,
    scope_of: &mut HashMap<usize, usize>,
    def_name_ids: &mut HashSet<usize>,
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
    walk_items(body, src, defs, mod_stack, imports, scope_of, def_name_ids);
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

fn push_def(span: Node, name: String, kind: DefKind, defs: &mut Vec<ExtractedDef>) -> usize {
    defs.push(ExtractedDef {
        name,
        kind,
        start_line: span.start_position().row as u32 + 1,
        end_line: span.end_position().row as u32 + 1,
        test_id: None,
        computed_name: false,
        parent: None,
    });
    defs.len() - 1
}

fn push_test_def(
    span: Node,
    name: String,
    test_id: Vec<String>,
    defs: &mut Vec<ExtractedDef>,
) -> usize {
    defs.push(ExtractedDef {
        name,
        kind: DefKind::TestCase,
        start_line: span.start_position().row as u32 + 1,
        end_line: span.end_position().row as u32 + 1,
        test_id: Some(test_id),
        computed_name: false,
        parent: None,
    });
    defs.len() - 1
}

/// The cheap allow-list used to filter read extraction down to non-local
/// noise: same-file top-level Function/Class (struct/enum/trait) names, plus
/// each `use` import's leaf name(s) (see `use_leaf_names`). Calls are never
/// filtered through this -- only reads, to keep read-extraction from
/// flooding on arbitrary local field/variable names.
fn build_known_names(defs: &[ExtractedDef], imports: &[ImportRef]) -> HashSet<String> {
    let mut names: HashSet<String> = defs
        .iter()
        .filter(|d| matches!(d.kind, DefKind::Function | DefKind::Class))
        .map(|d| d.name.clone())
        .collect();
    for imp in imports {
        names.extend(use_leaf_names(&imp.raw));
    }
    names
}

/// The leaf name(s) bound by a `use` `ImportRef`'s raw text (e.g. `"use
/// crate::math::add"` -> `["add"]`, `"use crate::{a, b}"` -> `["a", "b"]`).
/// `mod x` entries (module *declarations*, not `use` imports) yield nothing.
/// Glob imports (`use super::*`, `use crate::{a, *}`) contribute no name for
/// the `*` item since there's no specific symbol to allow-list -- an
/// accepted simplification (only widens what gets missed as a read, never
/// wrongly narrows).
fn use_leaf_names(raw: &str) -> Vec<String> {
    let Some(path_text) = raw.strip_prefix("use ") else {
        return Vec::new();
    };
    if let Some(brace_start) = path_text.rfind('{') {
        let Some(brace_end) = path_text.rfind('}') else {
            return Vec::new();
        };
        if brace_end <= brace_start {
            return Vec::new();
        }
        return path_text[brace_start + 1..brace_end]
            .split(',')
            .filter_map(|item| leaf_of(item.trim()))
            .collect();
    }
    leaf_of(path_text).into_iter().collect()
}

/// The bound name of a single `use`-path segment: the text after `as` for an
/// alias (`add as my_add` -> `my_add`), otherwise the last `::`-separated
/// segment (`crate::math::add` -> `add`); `None` for a glob (`*`) segment.
fn leaf_of(item: &str) -> Option<String> {
    if item.is_empty() || item == "*" || item.ends_with("::*") {
        return None;
    }
    let name = match item.rfind(" as ") {
        Some(idx) => &item[idx + 4..],
        None => item.rsplit("::").next().unwrap_or(item),
    };
    let name = name.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Shared read-only context for the calls/reads walk.
struct RefCtx<'a> {
    src: &'a [u8],
    /// AST node id -> def index of the def whose scope that node opens (a
    /// function/method's own `function_item` node).
    scope_of: &'a HashMap<usize, usize>,
    /// Node ids of defs' own name identifiers (excluded from read scanning).
    def_name_ids: &'a HashSet<usize>,
    /// Same-file top-level Function/Class names ∪ `use`-imported leaf names
    /// -- the cheap allow-list that keeps read extraction from flooding on
    /// local names.
    known_names: &'a HashSet<String>,
}

fn node_text<'a>(node: Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or_default()
}

/// Walk the whole tree recording `calls` and `reads`, threading
/// `current_def` -- the index (into `Extraction.defs`) of the innermost
/// enclosing def -- down through every function/method/test body via
/// `ctx.scope_of`. Top-level code (no enclosing def, e.g. a module-level
/// `static`/`const` initializer) uses `current_def`'s initial value, 0
/// (`<module>`).
///
/// Tier-1 known gap: macro invocations (`format!`, `assert_eq!`, `vec!`,
/// ...) are a distinct grammar node (`macro_invocation`, not
/// `call_expression`) whose arguments are an untyped `token_tree` --
/// tree-sitter-rust does not parse `add(a, b)` inside
/// `format!("{}", add(a, b))` as a nested `call_expression`; `add` and
/// `(a, b)` show up as bare sibling tokens (an `identifier` next to a
/// parenthesized `token_tree`). Full expression reconstruction inside a
/// token tree is unreliable (no grammar distinguishes an expression from
/// punctuation), so it's deliberately not attempted -- no reads, no
/// qualified/method calls recovered from inside a macro. The one exception,
/// handled by `scan_macro_call_tokens` below: a bare identifier immediately
/// followed by a `(...)`-shaped `token_tree` sibling is recognized as a
/// plain call, since this is the single most common and most reliably
/// recoverable shape (`assert_eq!(add(2, 2), 4)`, `format!("{}", add(a,
/// b))`). Anything shaped differently inside a macro (qualified calls,
/// method calls, reads) is silently missed; cross-file impact through such a
/// miss is still caught by the coarser ModuleInit/import-edge
/// over-approximation upstream, same-file diffs by containment.
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
                    "scoped_identifier" => {
                        if let (Some(path), Some(name)) = (
                            func.child_by_field_name("path"),
                            func.child_by_field_name("name"),
                        ) {
                            calls.push(ExtractedRef {
                                from_def: def,
                                name: node_text(name, ctx.src).to_string(),
                                qualifier: Some(node_text(path, ctx.src).to_string()),
                                line,
                            });
                        }
                    }
                    "field_expression" => {
                        if let (Some(value), Some(field)) = (
                            func.child_by_field_name("value"),
                            func.child_by_field_name("field"),
                        ) {
                            // Covers both `x.f()` method calls and the rare
                            // fn-valued-field case (`self.callback(x)`) --
                            // tree-sitter-rust doesn't distinguish the two
                            // syntactically, so neither do we.
                            calls.push(ExtractedRef {
                                from_def: def,
                                name: node_text(field, ctx.src).to_string(),
                                qualifier: Some(node_text(value, ctx.src).to_string()),
                                line,
                            });
                            // A plain identifier/`self` value is fully
                            // consumed by the call record above; anything
                            // more complex (e.g. `a().f()`) may hide further
                            // calls/reads, so recurse into it.
                            if !matches!(value.kind(), "identifier" | "self") {
                                walk_refs(value, ctx, def, calls, reads);
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
        "macro_invocation" => {
            // See the tier-1 gap note on this function's doc comment.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "token_tree" {
                    scan_macro_call_tokens(child, ctx, def, calls);
                }
            }
        }
        "field_expression" => {
            // Reached only for field expressions that are NOT a call callee
            // (those are handled, and their subtrees already walked, above).
            if let (Some(value), Some(field)) = (
                node.child_by_field_name("value"),
                node.child_by_field_name("field"),
            ) {
                if matches!(value.kind(), "identifier" | "self") {
                    let obj_name = node_text(value, ctx.src).to_string();
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
                    walk_refs(value, ctx, def, calls, reads);
                }
            }
        }
        "scoped_identifier" => {
            // Reached only for scoped identifiers that are NOT a call callee
            // (those are handled, and recorded, above).
            if let (Some(path), Some(name)) = (
                node.child_by_field_name("path"),
                node.child_by_field_name("name"),
            ) {
                let qualifier = node_text(path, ctx.src).to_string();
                let name_text = node_text(name, ctx.src).to_string();
                if ctx.known_names.contains(&name_text) || ctx.known_names.contains(&qualifier) {
                    reads.push(ExtractedRef {
                        from_def: def,
                        name: name_text,
                        qualifier: Some(qualifier),
                        line,
                    });
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

/// Best-effort scan of a macro's argument `token_tree` for the one call
/// shape reliably recoverable from untyped tokens: a bare `identifier`
/// immediately followed by a `(...)`-shaped `token_tree` sibling (e.g. the
/// `add(a, b)` in `format!("{}", add(a, b))`). Recurses into nested
/// `token_tree`s so calls nested inside other calls/macros within the same
/// argument list are still found. See the `macro_invocation` case in
/// `walk_refs`'s doc comment for the accepted limitations of this
/// heuristic.
fn scan_macro_call_tokens(
    token_tree: Node,
    ctx: &RefCtx,
    def: usize,
    calls: &mut Vec<ExtractedRef>,
) {
    let mut cursor = token_tree.walk();
    let children: Vec<Node> = token_tree.children(&mut cursor).collect();
    for (i, child) in children.iter().enumerate() {
        if child.kind() == "token_tree" {
            scan_macro_call_tokens(*child, ctx, def, calls);
            continue;
        }
        if child.kind() != "identifier" {
            continue;
        }
        let Some(next) = children.get(i + 1) else {
            continue;
        };
        if next.kind() == "token_tree" && node_text(*next, ctx.src).starts_with('(') {
            calls.push(ExtractedRef {
                from_def: def,
                name: node_text(*child, ctx.src).to_string(),
                qualifier: None,
                line: child.start_position().row as u32 + 1,
            });
        }
    }
}

/// Keep the first occurrence of each `(from_def, name, qualifier)` triple --
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
