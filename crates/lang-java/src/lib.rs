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
use std::sync::{Arc, Mutex};

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

/// Annotations that say something to the *compiler* and nothing to any
/// runtime container, so they must not be read as "this member is invoked
/// reflectively" (see the containment rule in `handle_type_declaration`).
///
/// `@Override` is the one that matters: it sits on a large fraction of all
/// Java methods, and counting it would parent nearly every method to its
/// class, which is precisely the blanket widening the annotation rule
/// exists to avoid. Measured on google/gson, treating `@Override` as
/// reflective took a leaf-utility edit from 6 selected tests to 1501.
/// Nullability annotations are here for the same reason: ubiquitous,
/// purely static.
const INERT_ANNOTATIONS: &[&str] = &[
    "Override",
    "SuppressWarnings",
    "Deprecated",
    "SafeVarargs",
    "FunctionalInterface",
    "Nullable",
    "NonNull",
    "Nonnull",
    "NotNull",
    "CheckForNull",
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
    /// repo root -> its fully-qualified-name index. Built once per repo and
    /// reused; see [`JavaLanguage::repo_index`] for why this can't be a
    /// per-import filesystem probe.
    index: Mutex<HashMap<PathBuf, Arc<RepoIndex>>>,
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
        // (owning class, field name) for every field this file declares, so
        // the refs pass can resolve a bare field name inside its own class
        // instead of against every same-named field in the package.
        let mut field_owners: HashSet<(String, String)> = HashSet::new();

        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if TYPE_DECLARATIONS.contains(&child.kind()) {
                handle_type_declaration(
                    child,
                    src_bytes,
                    &mut defs,
                    &mut scope_of,
                    &mut def_name_ids,
                    &mut field_owners,
                    None,
                    &[],
                    package.as_deref(),
                );
            }
        }

        let mut imports = Vec::new();
        collect_imports(root, src_bytes, &mut imports, &mut def_name_ids);

        let known_names = build_known_names(&defs, &imports);

        let def_class = def_classes(&defs);
        let ctx = RefCtx {
            src: src_bytes,
            scope_of: &scope_of,
            def_name_ids: &def_name_ids,
            known_names: &known_names,
            def_class: &def_class,
            field_owners: &field_owners,
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
    /// `static org.junit.jupiter.api.Assertions.assertEquals`) looked up in
    /// the repo's fully-qualified-name index.
    ///
    /// Three shapes are tried, in order:
    ///
    /// 1. `com/foo/core/Calc` as a *type* — an ordinary single-type import,
    ///    the precise and overwhelmingly common case.
    /// 2. `com/foo/util` as a *package* — a wildcard import, which the
    ///    indexer fans out to every indexed file under the directory (the
    ///    same dir-fanout Go package imports use).
    /// 3. `org/junit/jupiter/api/Assertions` — a static member import,
    ///    where the last segment names a member rather than a type.
    ///
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
        let index = self.repo_index(repo_root);

        if wildcard {
            return index
                .packages
                .get(&as_path)
                .and_then(|c| pick(c, from_file));
        }
        if let Some(hit) = index.classes.get(&as_path).and_then(|c| pick(c, from_file)) {
            return Some(hit);
        }
        if let Some(hit) = index
            .packages
            .get(&as_path)
            .and_then(|c| pick(c, from_file))
        {
            return Some(hit);
        }
        as_path
            .parent()
            .and_then(|owner| index.classes.get(owner))
            .and_then(|c| pick(c, from_file))
    }
}

impl JavaLanguage {
    /// The repo's fully-qualified-name index, built once and reused.
    ///
    /// This has to be an index, not a probe loop. `resolve_import` runs
    /// once per import per file; spring-boot is ~8.7k Java files across
    /// ~460 Gradle modules, so ~900 source roots. Stat-probing every root
    /// for every import is O(imports x roots) filesystem syscalls — on that
    /// repo, hundreds of millions, which never finishes. One walk up front
    /// turns each import into a hashmap lookup.
    ///
    /// A poisoned lock (another thread panicked mid-scan) degrades to an
    /// uncached rebuild rather than propagating the panic: import
    /// resolution is best-effort infrastructure, not a place to take the
    /// whole index down.
    fn repo_index(&self, repo_root: &Path) -> Arc<RepoIndex> {
        if let Ok(cache) = self.index.lock() {
            if let Some(hit) = cache.get(repo_root) {
                return Arc::clone(hit);
            }
        }
        let built = Arc::new(build_repo_index(repo_root));
        if let Ok(mut cache) = self.index.lock() {
            cache.insert(repo_root.to_path_buf(), Arc::clone(&built));
        }
        built
    }
}

/// Every type and package the repo declares, keyed by fully-qualified name
/// as a path (`com/foo/Calc`, `com/foo`). Values are repo-relative paths,
/// and there may be more than one: the same FQN can legitimately appear in
/// a `main` and a `test` source root, or in two unrelated modules.
#[derive(Default)]
struct RepoIndex {
    /// `com/foo/Calc` -> the `.java` files declaring it.
    classes: HashMap<PathBuf, Vec<PathBuf>>,
    /// `com/foo` -> the package directories holding it.
    packages: HashMap<PathBuf, Vec<PathBuf>>,
}

/// Choose among several files/directories declaring the same FQN: prefer
/// the importing file's own module (a multi-module repo can define the same
/// class name twice, and the local one is what the compiler would bind),
/// then a `src/main/java` root over a test root, then whatever came first.
fn pick(candidates: &[PathBuf], from_file: &Path) -> Option<PathBuf> {
    let own_module = module_of(from_file);
    candidates
        .iter()
        .min_by_key(|c| {
            let same_module = own_module.is_some() && module_of(c) == own_module;
            (!same_module, !is_main_root(c))
        })
        .cloned()
}

/// The build module a repo-relative path sits in: its source root minus the
/// trailing `src/<sourceset>/java`. `None` outside a conventional layout.
fn module_of(path: &Path) -> Option<PathBuf> {
    let root = source_root_of(path)?;
    Some(
        root.components()
            .take(root.components().count().saturating_sub(3))
            .collect(),
    )
}

fn is_main_root(path: &Path) -> bool {
    source_root_of(path)
        .map(|r| r.ends_with("main/java"))
        .unwrap_or(false)
}

/// Walk every source root once, recording each `.java` file under its
/// root-relative fully-qualified name and each package directory under its
/// root-relative package path.
fn build_repo_index(repo_root: &Path) -> RepoIndex {
    let mut index = RepoIndex::default();
    for root in scan_source_roots(repo_root) {
        collect_types(repo_root, &root, Path::new(""), &mut index);
    }
    index
}

fn collect_types(repo_root: &Path, root: &Path, pkg: &Path, index: &mut RepoIndex) {
    let Ok(entries) = std::fs::read_dir(repo_root.join(root).join(pkg)) else {
        return;
    };
    let mut has_types = false;
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                subdirs.push(name);
            }
            continue;
        }
        if let Some(stem) = name.strip_suffix(".java") {
            has_types = true;
            index
                .classes
                .entry(pkg.join(stem))
                .or_default()
                .push(root.join(pkg).join(&name));
        }
    }
    if has_types {
        index
            .packages
            .entry(pkg.to_path_buf())
            .or_default()
            .push(root.join(pkg));
    }
    for sub in subdirs {
        collect_types(repo_root, root, &pkg.join(sub), index);
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
    field_owners: &mut HashSet<(String, String)>,
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
                field_owners,
                Some(class_idx),
                &chain,
                package,
            );
            continue;
        }
        // Fields get defs of their own, one per declarator. Without them a
        // very common Java shape loses its chain entirely: a JUnit class
        // builds its fixture in a field initializer
        //
        //     private final ApplicationContextRunner contextRunner =
        //         new ApplicationContextRunner().withConfiguration(
        //             AutoConfigurations.of(JacksonAutoConfiguration.class));
        //
        // and every test method then works through `this.contextRunner`.
        // The dependency on `JacksonAutoConfiguration` lives on the *field*,
        // so with no field def the initializer's refs land on the class,
        // `Contains` only walks child -> parent, and the test methods are
        // never reached. Giving the field a def puts a real `Reads` edge
        // between each method and the fixture it uses.
        if member.kind() == "field_declaration" {
            let annotated = container_invocable(member, src);
            let mut fields = member.walk();
            let declarators: Vec<Node> = member
                .children(&mut fields)
                .filter(|c| c.kind() == "variable_declarator")
                .collect();
            for declarator in declarators {
                let Some(field_name_node) = declarator.child_by_field_name("name") else {
                    continue;
                };
                let Ok(field_name) = field_name_node.utf8_text(src) else {
                    continue;
                };
                def_name_ids.insert(field_name_node.id());
                field_owners.insert((type_name.to_string(), field_name.to_string()));
                let idx = push_def(
                    declarator,
                    // `Class#field`, not `Class.field`. The indexer keys defs
                    // on the segment after the last `.`, so `Class.field`
                    // would be indexed as bare `field` and every same-named
                    // field in the package would resolve to it — two test
                    // classes that each hold a `gson` field would cross-link,
                    // and on google/gson that alone dragged a peripheral edit
                    // out to 1439 of 1534 tests. A `#` keeps the whole
                    // qualified string as the key, which matches Java: a bare
                    // field name is resolved in its own class, never a
                    // sibling's.
                    format!("{type_name}#{field_name}"),
                    DefKind::Method,
                    src,
                    defs,
                    // An injected field (`@Autowired`, `@Mock`, `@Value`) is
                    // populated by the container, same reasoning as an
                    // annotated method below.
                    annotated.then_some(class_idx),
                    None,
                );
                // The initializer's refs belong to the field, not the class.
                scope_of.insert(declarator.id(), idx);
            }
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

        let annotations = annotation_names(member, src);
        let (kind, test_id) = if annotations
            .iter()
            .any(|n| TEST_ANNOTATIONS.contains(&n.as_str()))
        {
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
        // Whether this member hangs off its class in the graph, which
        // decides how far a change to it widens. `walk` reverse-propagates
        // `Contains{parent, child}` as behavioral embedding: parenting a
        // method to its class means "this method body changed" => "the
        // class changed" => every test that merely writes `new Calc()` or
        // holds a `Calc` field is impacted.
        //
        // Doing that unconditionally is very wide (in Java *all* code lives
        // in a class). Never doing it is *unsound*, which is worse and not
        // hypothetical: a Spring `@Bean` method is invoked by the container,
        // never by name, so with no containment edge a change to it reaches
        // no test at all — a silent under-select, which the selection
        // contract forbids. Verified on spring-boot: editing a `@Bean` body
        // selected zero of 17,720 tests.
        //
        // So: a member carrying *any* annotation is treated as reachable
        // without being named, and parents to its class. That covers the
        // realistic reflective entry points — `@Bean`, `@PostConstruct`,
        // `@EventListener`, `@Scheduled`, `@RequestMapping`, JUnit's own
        // lifecycle hooks — because framework-invoked Java is
        // annotation-driven essentially by construction. A plain, unannotated
        // method is reached by name or not at all: ordinary calls and
        // interface dispatch both go through `Calls` (short-name matching
        // already widens across implementations), and a class running its
        // own method from a field initializer or static block records that
        // as a `Calls` ref attributed to the class def. Parentless defs get
        // wired to the file's `ModuleInit`, which `walk` declines to widen
        // through.
        //
        // Residual gap, accepted and named: `Class.forName(..).getMethod
        // ("plainName")` against an *unannotated* method. That's the same
        // class of dynamic-reflection risk every language plugin here
        // carries, and `testless.toml`'s `always-run` is the escape hatch.
        let idx = push_def(
            member,
            format!("{type_name}.{member_name}"),
            kind,
            src,
            defs,
            container_invocable(member, src).then_some(class_idx),
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

/// Whether `node` carries an annotation implying something other than the
/// compiler invokes it: a Spring `@Bean`, a `@PostConstruct`, an injected
/// `@Autowired` field. Purely static annotations ([`INERT_ANNOTATIONS`])
/// don't count.
fn container_invocable(node: Node, src: &[u8]) -> bool {
    annotation_names(node, src)
        .iter()
        .any(|n| !INERT_ANNOTATIONS.contains(&n.as_str()))
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
    /// Owning class name per def index, so a ref can be resolved relative to
    /// the class it appears in.
    def_class: &'a [Option<String>],
    /// (class, field) pairs this file declares; see `field_ref`.
    field_owners: &'a HashSet<(String, String)>,
}

impl RefCtx<'_> {
    /// How to record a reference to the bare name `name` seen inside def
    /// `def`.
    ///
    /// When `name` is a field of the very class the reference sits in, it
    /// resolves to that class's field def and nothing else — that is Java's
    /// own rule, and keeping it exact is what stops two classes' identically
    /// named fields from linking together.
    ///
    /// Otherwise it falls back to the plain name gated by `known_names`: an
    /// *inherited* field (declared by a superclass, so absent from
    /// `field_owners`) still has to reach its declaration, and matching by
    /// bare name over-approximates rather than missing it.
    fn field_ref(&self, def: usize, name: &str) -> Option<String> {
        if let Some(Some(class)) = self.def_class.get(def) {
            if self
                .field_owners
                .contains(&(class.clone(), name.to_string()))
            {
                return Some(format!("{class}#{name}"));
            }
        }
        self.known_names.contains(name).then(|| name.to_string())
    }
}

/// The class each def belongs to, derived from the `Class#field` /
/// `Class.member` naming above; a type's own def maps to itself, so refs in
/// a static initializer resolve against that type's fields.
fn def_classes(defs: &[ExtractedDef]) -> Vec<Option<String>> {
    defs.iter()
        .map(|d| match d.kind {
            DefKind::ModuleInit => None,
            DefKind::Class => Some(d.name.clone()),
            _ => d
                .name
                .split_once('#')
                .or_else(|| d.name.split_once('.'))
                .map(|(class, _)| class.to_string()),
        })
        .collect()
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
                match object {
                    // `contextRunner.run()`: the receiver names a field or
                    // an imported type, and that *is* a dependency of this
                    // method — the qualifier alone carries no edge, since
                    // resolution matches on `name`. Record it as a read so
                    // a method using a fixture field links to that field.
                    Some(object) if object.kind() == "identifier" => {
                        let text = node_text(object, ctx.src);
                        if let Some(name) = ctx.field_ref(def, text) {
                            reads.push(ExtractedRef {
                                from_def: def,
                                name,
                                qualifier: None,
                                line,
                            });
                        }
                    }
                    // Anything richer (`a().b()`, `new Foo().bar()`) can
                    // hide further refs, so recurse into it.
                    Some(object) => walk_refs(object, ctx, def, calls, reads),
                    None => {}
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
                match object.kind() {
                    "identifier" => {
                        let obj_name = node_text(object, ctx.src).to_string();
                        let field_name = node_text(field, ctx.src).to_string();
                        if ctx.known_names.contains(&obj_name)
                            || ctx.known_names.contains(&field_name)
                        {
                            reads.push(ExtractedRef {
                                from_def: def,
                                name: field_name,
                                qualifier: Some(obj_name),
                                line,
                            });
                        }
                    }
                    // `this.contextRunner`: unqualified access to one of
                    // this class's own fields. Without this arm the whole
                    // `this.`-prefixed style — which is how a lot of Java
                    // reads its own state — contributes no edges at all.
                    "this" => {
                        let field_name = node_text(field, ctx.src);
                        if let Some(name) = ctx.field_ref(def, field_name) {
                            reads.push(ExtractedRef {
                                from_def: def,
                                name,
                                qualifier: None,
                                line,
                            });
                        }
                    }
                    _ => walk_refs(object, ctx, def, calls, reads),
                }
            }
        }
        "identifier" => {
            if !ctx.def_name_ids.contains(&node.id()) {
                let text = node_text(node, ctx.src);
                if let Some(name) = ctx.field_ref(def, text) {
                    reads.push(ExtractedRef {
                        from_def: def,
                        name,
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
