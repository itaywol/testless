//! Java plugin: JUnit 5 tests, Maven/Gradle source layouts.
//!
//! Two things make Java different from the other plugins:
//!
//! - **Package scope.** Same-package classes see each other with no import
//!   statement, much like Go — but a Java package is *not* a directory: the
//!   same package spans `src/main/java` and `src/test/java`, which is
//!   exactly where a class and its unit test live. [`Language::package_key`]
//!   is what lets `indexer`/`classify` treat those two directories as one
//!   scope.
//! - **Import resolution has to be real.** `walk`'s `Unknown(name)`
//!   widening is scoped to the *forward transitive import closure*, so a
//!   cross-module reference whose import failed to resolve doesn't widen —
//!   it silently drops, which is an under-select, the one failure mode
//!   testless doesn't accept. So `resolve_import` genuinely locates the
//!   target file rather than giving up on anything outside the current
//!   module: it discovers every `*/src/*/java` source root in the repo
//!   (memoized per repo root, see [`JavaLanguage::source_roots`]) and
//!   probes the fully-qualified name against each.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use testless_core::fingerprint::{module_init_fingerprint, split_fingerprint};
use testless_core::{DefKind, ExtractedDef, ExtractedRef, Extraction, ImportRef, Language};
use tree_sitter::Node;

/// Annotations that mark a method as a JUnit 5 test. Matched on the
/// annotation's *simple* name, so a fully-qualified `@org.junit.jupiter.
/// api.Test` counts the same as a plain `@Test`.
///
/// `@ParameterizedTest`/`@RepeatedTest`/`@TestFactory`/`@TestTemplate` are
/// included and, unlike vitest's template-literal titles or Go's non-literal
/// `t.Run` argument, they are *not* marked `computed_name`: their generated
/// per-invocation display names vary, but both `mvn -Dtest=Class#method` and
/// `gradle --tests Class.method` filter on the *method* name, which is
/// statically known here. Widening those to the whole class would only
/// over-select.
const TEST_ANNOTATIONS: &[&str] = &[
    "Test",
    "ParameterizedTest",
    "RepeatedTest",
    "TestFactory",
    "TestTemplate",
];

/// Node kinds that declare a type and therefore become a `DefKind::Class`.
const TYPE_DECLARATIONS: &[&str] = &[
    "class_declaration",
    "interface_declaration",
    "enum_declaration",
    "record_declaration",
    "annotation_type_declaration",
];

/// Directory names never worth descending into when hunting for source
/// roots: build outputs and VCS/tooling metadata, none of which hold
/// indexable sources.
const SKIP_DIRS: &[&str] = &[
    "target",
    "build",
    "out",
    "bin",
    ".git",
    ".gradle",
    ".idea",
    "node_modules",
];

/// How deep the source-root scan descends below the repo root before giving
/// up. A conventional root is at depth 3 (`src/main/java`) and a
/// multi-module one at 4-5 (`services/billing/src/main/java`); this leaves
/// headroom for deeply grouped monorepos while keeping a pathological tree
/// from being walked in full.
const MAX_ROOT_SCAN_DEPTH: usize = 8;

#[derive(Default)]
pub struct JavaLanguage {
    /// repo root -> its `*/src/*/java` source roots, repo-relative. Built
    /// once per repo and reused: `resolve_import` runs per import per file,
    /// so rescanning the tree each time would be quadratic.
    roots: Mutex<HashMap<PathBuf, Vec<PathBuf>>>,
}

impl Language for JavaLanguage {
    fn id(&self) -> &'static str {
        "java"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["java"]
    }

    /// The package path with the source root stripped, so that
    /// `src/main/java/com/foo/Calc.java` and
    /// `src/test/java/com/foo/CalcTest.java` share the key `com/foo` and
    /// resolve each other's names without an import — which is exactly how
    /// Java unit tests are laid out, and the single most important edge in
    /// the whole graph. A file outside a conventional source root falls
    /// back to its directory.
    ///
    /// Multi-module repos are handled by keeping the module prefix: a key
    /// is `<module>/<package>`, so `com.foo` in `services/billing` doesn't
    /// silently merge with an unrelated `com.foo` in `services/ledger`.
    /// Cross-module references go through real imports instead (see
    /// `resolve_import`).
    fn package_key(&self, file: &Path) -> Option<PathBuf> {
        let dir = file.parent().unwrap_or(Path::new(""));
        let Some(root) = source_root_of(file) else {
            return Some(dir.to_path_buf());
        };
        // `<module>/src/<set>/java` -> `<module>`; three components off the
        // end of the root, whatever the module prefix is.
        let module: PathBuf = root
            .components()
            .take(root.components().count().saturating_sub(3))
            .collect();
        let package = dir.strip_prefix(&root).unwrap_or(dir);
        Some(module.join(package))
    }

    fn grammar(&self, _path: &Path) -> tree_sitter::Language {
        tree_sitter_java::LANGUAGE.into()
    }

    fn extract(&self, src: &str, tree: &tree_sitter::Tree) -> Extraction {
        let root = tree.root_node();
        let src_bytes = src.as_bytes();
        let mut defs = Vec::new();

        // One `<module>` per file, always. Unlike Go or TS there is very
        // little loose top-level code in Java (everything lives inside a
        // type), so this hash is near-constant — but the def still earns
        // its keep as the seed target for import-level and deletion-level
        // changes, which is what `classify` and `walk` reach for.
        let module_init_skip = |n: &tree_sitter::Node| {
            TYPE_DECLARATIONS.contains(&n.kind())
                || matches!(n.kind(), "import_declaration" | "package_declaration")
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

        let package = package_name(root, src_bytes);

        // `scope_of` maps a def's own AST node to its index in `defs`, so
        // the refs pass can track the innermost enclosing def. `def_name_ids`
        // holds identifiers that merely *name* something (a class, a method,
        // a parameter, an annotation) and so must never be scanned as reads.
        let mut scope_of: HashMap<usize, usize> = HashMap::new();
        let mut def_name_ids: HashSet<usize> = HashSet::new();

        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if TYPE_DECLARATIONS.contains(&child.kind()) {
                handle_type_declaration(
                    child,
                    src_bytes,
                    &mut defs,
                    &mut scope_of,
                    &mut def_name_ids,
                    None,
                    &[],
                    package.as_deref(),
                );
            }
        }

        let mut imports = Vec::new();
        collect_imports(root, src_bytes, &mut imports, &mut def_name_ids);

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

    /// A dotted Java import (`com.foo.core.Calc`, `com.foo.util.*`,
    /// `static org.junit.jupiter.api.Assertions.assertEquals`) resolved
    /// against every source root in the repo.
    ///
    /// Three shapes are probed per root, in order:
    ///
    /// 1. `<root>/com/foo/core/Calc.java` — an ordinary single-type import,
    ///    the precise and overwhelmingly common case.
    /// 2. `<root>/com/foo/util` as a *directory* — a wildcard import, which
    ///    the indexer fans out to every indexed file under it (the same
    ///    dir-fanout Go package imports use).
    /// 3. `<root>/org/junit/jupiter/api/Assertions.java` — a static member
    ///    import, where the last segment names a member rather than a type.
    ///
    /// The importing file's *own* source root is tried first, so a
    /// same-module import doesn't pay for a probe of every sibling module.
    /// Anything that matches nothing (`java.util.List`, a third-party jar)
    /// yields `None`, exactly as an unresolvable import should.
    fn resolve_import(&self, from_file: &Path, raw: &str, repo_root: &Path) -> Option<PathBuf> {
        let raw = raw.trim();
        let wildcard = raw.ends_with(".*");
        let dotted = raw.strip_suffix(".*").unwrap_or(raw);
        if dotted.is_empty() {
            return None;
        }
        let as_path: PathBuf = dotted.split('.').collect();

        for root in self.probe_order(from_file, repo_root) {
            if wildcard {
                if repo_root.join(root.join(&as_path)).is_dir() {
                    return Some(root.join(&as_path));
                }
                continue;
            }
            let file = root.join(as_path.with_extension("java"));
            if repo_root.join(&file).is_file() {
                return Some(file);
            }
            let dir = root.join(&as_path);
            if repo_root.join(&dir).is_dir() {
                return Some(dir);
            }
            if let Some(parent) = as_path.parent() {
                let owner = root.join(parent.with_extension("java"));
                if repo_root.join(&owner).is_file() {
                    return Some(owner);
                }
            }
        }
        None
    }
}

impl JavaLanguage {
    /// The source roots to probe for an import from `from_file`, best
    /// candidate first: `src/main/java` roots ahead of everything else,
    /// then the importing file's own root, then the rest.
    ///
    /// Ordering only decides anything when *two* roots hold the same
    /// package, and then production code is the right answer: an
    /// `import com.foo.thing.*` from another package means the classes
    /// `com/foo/thing` publishes, not a same-named test-only package.
    ///
    /// ponytail: a wildcard import resolves to one directory, so a package
    /// genuinely split across `src/main/java` *and* `src/test/java` only
    /// contributes its main half. Widening that needs `resolve_import` to
    /// return several paths; until a real repo shows the split mattering,
    /// main-first covers it.
    fn probe_order(&self, from_file: &Path, repo_root: &Path) -> Vec<PathBuf> {
        let own = source_root_of(from_file);
        let mut candidates: Vec<PathBuf> = own.iter().cloned().collect();
        for root in self.source_roots(repo_root) {
            if !candidates.contains(&root) {
                candidates.push(root);
            }
        }
        // Stable, so the own-root-first ordering survives within each group.
        candidates.sort_by_key(|r| !r.ends_with("main/java"));
        candidates
    }

    /// Every `*/src/*/java` directory in `repo_root`, repo-relative, found
    /// once and cached. A poisoned lock (another thread panicked mid-scan)
    /// degrades to an uncached rescan rather than propagating the panic:
    /// import resolution is best-effort infrastructure, not a place to take
    /// the whole index down.
    fn source_roots(&self, repo_root: &Path) -> Vec<PathBuf> {
        if let Ok(cache) = self.roots.lock() {
            if let Some(hit) = cache.get(repo_root) {
                return hit.clone();
            }
        }
        let found = scan_source_roots(repo_root);
        if let Ok(mut cache) = self.roots.lock() {
            cache.insert(repo_root.to_path_buf(), found.clone());
        }
        found
    }
}

/// The `<...>/src/<sourceset>/java` prefix of a repo-relative Java file, or
/// `None` for a file outside the conventional layout. Searched from the end
/// so a repo path that itself contains a `src` segment can't truncate early.
fn source_root_of(file: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = file.components().collect();
    let idx = (0..parts.len().saturating_sub(2))
        .rev()
        .find(|&i| parts[i].as_os_str() == "src" && parts[i + 2].as_os_str() == "java")?;
    Some(parts[..=idx + 2].iter().collect())
}

/// Depth-bounded directory walk collecting every `src/<sourceset>/java`
/// path under `repo_root`, returned repo-relative. Descent stops at a
/// matched root (nothing below it is another root) and at [`SKIP_DIRS`].
fn scan_source_roots(repo_root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    scan_dir(repo_root, Path::new(""), 0, &mut found);
    found.sort();
    found
}

fn scan_dir(repo_root: &Path, rel: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth > MAX_ROOT_SCAN_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(repo_root.join(rel)) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
            continue;
        }
        let child = rel.join(&name);
        if source_root_of(&child.join("X.java")).as_deref() == Some(child.as_path()) {
            found.push(child);
            continue;
        }
        scan_dir(repo_root, &child, depth + 1, found);
    }
}

/// The file's `package a.b.c;` declaration, if it has one.
fn package_name(root: Node, src: &[u8]) -> Option<String> {
    let mut cursor = root.walk();
    let decl = root
        .children(&mut cursor)
        .find(|c| c.kind() == "package_declaration")?;
    let mut inner = decl.walk();
    let name = decl
        .children(&mut inner)
        .find(|c| matches!(c.kind(), "scoped_identifier" | "identifier"))?;
    name.utf8_text(src).ok().map(|s| s.to_string())
}

/// A type declaration and everything inside it: the type itself becomes a
/// `Class` def, its methods and constructors become `Method` defs (or
/// `TestCase` defs when annotated), and nested types recurse.
///
/// `class_chain` is the simple names of the enclosing types, outermost
/// first; combined with `package` it builds a test's `test_id` as
/// `[fqcn, nested…, method]` — the shape `format`'s Maven/Gradle renderers
/// re-join with `$` to reach the JVM's name for a `@Nested` class.
#[allow(clippy::too_many_arguments)]
fn handle_type_declaration(
    node: Node,
    src: &[u8],
    defs: &mut Vec<ExtractedDef>,
    scope_of: &mut HashMap<usize, usize>,
    def_name_ids: &mut HashSet<usize>,
    parent: Option<usize>,
    class_chain: &[String],
    package: Option<&str>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Ok(type_name) = name_node.utf8_text(src) else {
        return;
    };

    // A type's own hash deliberately excludes its members' *bodies*.
    // `split_fingerprint` would fold the entire class body — every method
    // body included — into the type's `body_hash`, so editing one method
    // would mark the whole type changed and drag in everything that merely
    // holds a reference to it. In TS that's an acceptable over-select
    // because most code is top-level; in Java *all* code lives in a type,
    // so it collapses selection to "every test that touches this class".
    //
    // Instead: `sig_hash` is the declaration minus its body (modifiers,
    // name, type parameters, `extends`/`implements` — a change to any of
    // which really does affect every user of the type), and `body_hash`
    // covers only the members that have no def of their own: field
    // declarations with their initializers, and static/instance initializer
    // blocks. That matches where `scope_of` attributes those refs, and
    // leaves method bodies to the method defs that own them.
    let member_has_own_def = |n: &Node| {
        matches!(n.kind(), "method_declaration" | "constructor_declaration")
            || TYPE_DECLARATIONS.contains(&n.kind())
    };
    let (sig_hash, _) = split_fingerprint(node, src);
    let body_hash = node
        .child_by_field_name("body")
        .map(|body| module_init_fingerprint(body, src, &member_has_own_def));
    defs.push(ExtractedDef {
        name: type_name.to_string(),
        kind: DefKind::Class,
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        test_id: None,
        computed_name: false,
        parent,
        sig_hash,
        body_hash,
    });
    let class_idx = defs.len() - 1;
    def_name_ids.insert(name_node.id());
    // The type's own scope catches refs that don't open a narrower one —
    // field initializers, static blocks — so they attribute here rather
    // than falling through to `<module>`. Method bodies still win, since
    // `walk_refs` re-resolves `scope_of` at every node.
    scope_of.insert(node.id(), class_idx);

    let mut chain = class_chain.to_vec();
    chain.push(type_name.to_string());

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for member in body.children(&mut cursor) {
        if TYPE_DECLARATIONS.contains(&member.kind()) {
            handle_type_declaration(
                member,
                src,
                defs,
                scope_of,
                def_name_ids,
                Some(class_idx),
                &chain,
                package,
            );
            continue;
        }
        if !matches!(
            member.kind(),
            "method_declaration" | "constructor_declaration"
        ) {
            continue;
        }
        let Some(member_name_node) = member.child_by_field_name("name") else {
            continue;
        };
        let Ok(member_name) = member_name_node.utf8_text(src) else {
            continue;
        };
        def_name_ids.insert(member_name_node.id());

        let (kind, test_id) = if is_test_method(member, src) {
            (
                DefKind::TestCase,
                Some(test_id_for(package, &chain, member_name)),
            )
        } else {
            (DefKind::Method, None)
        };
        // Qualified `Class.method`, mirroring Go's `Recv.Method`: the
        // indexer indexes defs under their short (post-`.`) name anyway, so
        // this costs nothing at resolution time and makes `why` output name
        // the owning type.
        //
        // `parent: None` — deliberately *not* the enclosing class. `walk`
        // reverse-propagates `Contains{parent, child}` as genuine
        // behavioral embedding, so parenting a method to its class would
        // mean "any method body changed" => "the class changed" => every
        // test that so much as writes `new Calc()` or holds a `Calc` field
        // is impacted. In a language where all code lives in a class that
        // is nearly every test in the repo, which would leave Java
        // selection technically sound but useless.
        //
        // Nothing real is lost: a class whose *own* code runs a method —
        // a field initializer, a static block — records that as an
        // ordinary `Calls` ref attributed to the class def, so genuine
        // embedding still propagates. What disappears is only the
        // *presumed* embedding of a method the caller never invokes.
        // Parentless defs get wired to the file's `ModuleInit` by the
        // indexer, and `walk` already declines to widen through that.
        let idx = push_def(
            member,
            format!("{type_name}.{member_name}"),
            kind,
            src,
            defs,
            None,
            test_id,
        );
        scope_of.insert(member.id(), idx);
    }
}

/// `[fqcn, nested…, method]`: the outermost class qualified by the file's
/// package, then each nested type's simple name, then the method. A file
/// with no `package` declaration (the default package) contributes no
/// prefix.
fn test_id_for(package: Option<&str>, chain: &[String], method: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(chain.len() + 1);
    match (package, chain.first()) {
        (Some(pkg), Some(outer)) => out.push(format!("{pkg}.{outer}")),
        (None, Some(outer)) => out.push(outer.clone()),
        (_, None) => {}
    }
    out.extend(chain.iter().skip(1).cloned());
    out.push(method.to_string());
    out
}

/// Does this member carry a JUnit 5 test annotation? Matched on the
/// annotation's simple name (see [`TEST_ANNOTATIONS`]).
fn is_test_method(member: Node, src: &[u8]) -> bool {
    annotation_names(member, src)
        .iter()
        .any(|n| TEST_ANNOTATIONS.contains(&n.as_str()))
}

/// The simple names of every annotation in `node`'s `modifiers` child.
/// A qualified `@org.junit.jupiter.api.Test` reduces to `Test`.
fn annotation_names(node: Node, src: &[u8]) -> Vec<String> {
    let mut cursor = node.walk();
    let Some(modifiers) = node.children(&mut cursor).find(|c| c.kind() == "modifiers") else {
        return Vec::new();
    };
    let mut inner = modifiers.walk();
    modifiers
        .children(&mut inner)
        .filter(|c| matches!(c.kind(), "marker_annotation" | "annotation"))
        .filter_map(|a| a.child_by_field_name("name"))
        .filter_map(|n| n.utf8_text(src).ok())
        .map(|n| n.rsplit('.').next().unwrap_or(n).to_string())
        .collect()
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
) -> usize {
    let (sig_hash, body_hash) = split_fingerprint(span, src);
    defs.push(ExtractedDef {
        name,
        kind,
        start_line: span.start_position().row as u32 + 1,
        end_line: span.end_position().row as u32 + 1,
        test_id,
        // Java test identity is the method name, which is always static —
        // there is no template-literal / runtime-string equivalent here.
        computed_name: false,
        parent,
        sig_hash,
        body_hash,
    });
    defs.len() - 1
}

/// Collect every `import_declaration`'s dotted text (wildcards keep their
/// trailing `.*` so `resolve_import` can tell them apart; the `static`
/// keyword is dropped, as it says nothing about *where* the target lives).
///
/// Annotation *name* identifiers are folded into `def_name_ids` in the same
/// pass: `@Test` would otherwise read as a reference to a symbol named
/// `Test`. An annotation's arguments are deliberately left alone, since
/// those hold real references (`@ExtendWith(MockitoExtension.class)`).
fn collect_imports(
    node: Node,
    src: &[u8],
    imports: &mut Vec<ImportRef>,
    def_name_ids: &mut HashSet<usize>,
) {
    match node.kind() {
        "import_declaration" => {
            let mut cursor = node.walk();
            let children: Vec<Node> = node.children(&mut cursor).collect();
            let Some(path) = children
                .iter()
                .find(|c| matches!(c.kind(), "scoped_identifier" | "identifier"))
            else {
                return;
            };
            let Ok(text) = path.utf8_text(src) else {
                return;
            };
            let wildcard = children.iter().any(|c| c.kind() == "asterisk");
            imports.push(ImportRef {
                raw: if wildcard {
                    format!("{text}.*")
                } else {
                    text.to_string()
                },
                line: node.start_position().row as u32 + 1,
            });
            return;
        }
        "marker_annotation" | "annotation" => {
            if let Some(name) = node.child_by_field_name("name") {
                mark_identifiers(name, def_name_ids);
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_imports(child, src, imports, def_name_ids);
    }
}

/// Record `node` and every identifier beneath it as "names something"
/// (a qualified annotation name is a `scoped_identifier` tree, not a
/// single token).
fn mark_identifiers(node: Node, def_name_ids: &mut HashSet<usize>) {
    def_name_ids.insert(node.id());
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        mark_identifiers(child, def_name_ids);
    }
}

/// The allow-list that keeps bare-identifier read extraction from flooding
/// on local variables: this file's own def names (both the qualified
/// `Class.method` form and its short tail) plus each import's last segment,
/// which is the simple name a call site actually writes.
fn build_known_names(defs: &[ExtractedDef], imports: &[ImportRef]) -> HashSet<String> {
    let mut names: HashSet<String> = HashSet::new();
    for d in defs {
        if d.kind == DefKind::ModuleInit {
            continue;
        }
        names.insert(d.name.clone());
        if let Some(short) = d.name.rsplit('.').next() {
            names.insert(short.to_string());
        }
    }
    for imp in imports {
        let raw = imp.raw.strip_suffix(".*").unwrap_or(&imp.raw);
        if let Some(last) = raw.rsplit('.').next() {
            if !last.is_empty() {
                names.insert(last.to_string());
            }
        }
    }
    names
}

struct RefCtx<'a> {
    src: &'a [u8],
    scope_of: &'a HashMap<usize, usize>,
    def_name_ids: &'a HashSet<usize>,
    known_names: &'a HashSet<String>,
}

fn node_text<'a>(node: Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or_default()
}

/// Walk the tree recording `calls` and `reads`, threading `current_def`:
/// the index of the innermost enclosing def, via `ctx.scope_of`.
///
/// `type_identifier` nodes are emitted as reads *unfiltered*, unlike bare
/// identifiers. That's deliberate and it's the edge that makes Java
/// dependency-injection wiring visible: a field, parameter or return type
/// is a genuine structural dependency on that class even when no method of
/// it is ever called by name in this file, and a type position can never be
/// a local variable, so there's no noise to filter out. Types the repo
/// doesn't define (`String`, `List`) simply resolve to nothing.
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
        // A declaration, not a reference: `import a.b.C;` ends in an
        // identifier `C` that would otherwise scan as a read of `C` from
        // `<module>`. The dependency it expresses is already carried by the
        // `Imports` edge `resolve_import` produces.
        "import_declaration" | "package_declaration" => {}
        "method_invocation" => {
            if let Some(name) = node.child_by_field_name("name") {
                let object = node.child_by_field_name("object");
                calls.push(ExtractedRef {
                    from_def: def,
                    name: node_text(name, ctx.src).to_string(),
                    qualifier: object
                        .filter(|o| matches!(o.kind(), "identifier" | "field_access"))
                        .map(|o| node_text(o, ctx.src).to_string()),
                    line,
                });
                // A plain identifier receiver is fully described by the
                // qualifier above; anything richer (`a().b()`, `new
                // Foo().bar()`) can hide further refs, so recurse into it.
                if let Some(object) = object {
                    if object.kind() != "identifier" {
                        walk_refs(object, ctx, def, calls, reads);
                    }
                }
            }
            if let Some(args) = node.child_by_field_name("arguments") {
                walk_refs(args, ctx, def, calls, reads);
            }
        }
        "object_creation_expression" => {
            // `new Calc()` is a call into `Calc`'s constructor and, for our
            // purposes, a dependency on the class itself. Recorded under
            // the type's simple name so it matches the `Class` def.
            if let Some(ty) = node.child_by_field_name("type") {
                calls.push(ExtractedRef {
                    from_def: def,
                    name: simple_type_name(node_text(ty, ctx.src)),
                    qualifier: None,
                    line,
                });
            }
            if let Some(args) = node.child_by_field_name("arguments") {
                walk_refs(args, ctx, def, calls, reads);
            }
        }
        "type_identifier" => {
            reads.push(ExtractedRef {
                from_def: def,
                name: node_text(node, ctx.src).to_string(),
                qualifier: None,
                line,
            });
        }
        "field_access" => {
            if let (Some(object), Some(field)) = (
                node.child_by_field_name("object"),
                node.child_by_field_name("field"),
            ) {
                if object.kind() == "identifier" {
                    let obj_name = node_text(object, ctx.src).to_string();
                    let field_name = node_text(field, ctx.src).to_string();
                    if ctx.known_names.contains(&obj_name) || ctx.known_names.contains(&field_name)
                    {
                        reads.push(ExtractedRef {
                            from_def: def,
                            name: field_name,
                            qualifier: Some(obj_name),
                            line,
                        });
                    }
                } else {
                    walk_refs(object, ctx, def, calls, reads);
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

/// `com.foo.Calc` -> `Calc`, `List<String>` -> `List`: the simple name a
/// `Class` def is indexed under.
fn simple_type_name(text: &str) -> String {
    let base = text.split('<').next().unwrap_or(text);
    base.rsplit('.').next().unwrap_or(base).trim().to_string()
}

/// Keep the first occurrence of each `(from_def, name, qualifier)` triple:
/// a def may reference the same symbol many times, but the graph only needs
/// the edge once.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_root_of_finds_conventional_layout() {
        assert_eq!(
            source_root_of(Path::new(
                "services/billing/src/test/java/com/foo/BarTest.java"
            )),
            Some(PathBuf::from("services/billing/src/test/java"))
        );
        assert_eq!(
            source_root_of(Path::new("src/main/java/com/foo/Bar.java")),
            Some(PathBuf::from("src/main/java"))
        );
        assert_eq!(source_root_of(Path::new("java/com/foo/Bar.java")), None);
    }

    #[test]
    fn simple_type_name_strips_package_and_generics() {
        assert_eq!(simple_type_name("Calc"), "Calc");
        assert_eq!(simple_type_name("com.foo.Calc"), "Calc");
        assert_eq!(simple_type_name("List<String>"), "List");
        assert_eq!(simple_type_name("java.util.List<String>"), "List");
    }

    #[test]
    fn test_id_qualifies_outer_class_and_keeps_nesting() {
        assert_eq!(
            test_id_for(Some("com.foo"), &["BarTest".to_string()], "adds"),
            vec!["com.foo.BarTest", "adds"]
        );
        assert_eq!(
            test_id_for(
                Some("com.foo"),
                &["BarTest".to_string(), "WhenEmpty".to_string()],
                "throws"
            ),
            vec!["com.foo.BarTest", "WhenEmpty", "throws"]
        );
        // Default package: no prefix to qualify with.
        assert_eq!(
            test_id_for(None, &["BarTest".to_string()], "adds"),
            vec!["BarTest", "adds"]
        );
    }
}
