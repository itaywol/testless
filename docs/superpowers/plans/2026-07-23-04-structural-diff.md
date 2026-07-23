# testless Plan 3: Structural Diff + Change Classification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Given `--from <rev>` (and `--to <rev|WORKTREE>`), produce classified change seeds — which defs semantically changed and how — with comment/formatting-only edits producing ZERO seeds. Surfaced as a new `testless changes` command; Plan 4's walk consumes the seeds.

**Architecture:** Structural fingerprints (subtree hash skipping comment nodes) make the differ format/comment-insensitive. Def-level matching (by kind+name/test_id) between old and new extractions classifies each def Added/Removed/BodyChanged/SignatureChanged. Git integration shells out to `git` (no libgit2). A classifier maps file-level changes to seeds or `RunAll` per the spec table.

**Tech Stack:** existing workspace; `git` CLI via `std::process::Command`; no new crate deps (blake3 already present for hashing).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-23-testless-design.md` §"Change classification → impact seeds" is the authoritative table
- **Innermost-def attribution contract (Plan 1 final review):** a changed AST node attributes to its INNERMOST enclosing def. NEVER classify by line-span overlap — the whole-file ModuleInit span would match every edit and break "comment-only → zero"
- Over-approximation: ambiguity → more seeds / RunAll, never fewer
- Failure posture: unparseable changed file, git errors → `RunAll` with reason, never a guess, never a panic
- Conventional commits; branch `plan-3-structural-diff` off main; fmt/clippy/test CI-enforced

## File Structure

```
crates/core/src/fingerprint.rs   # structural subtree hashing
crates/core/src/diffdef.rs       # old/new extraction matching → DefChange list
crates/core/src/gitio.rs         # git CLI wrapper: changed files, show, untracked
crates/core/src/classify.rs      # DefChanges + file statuses → ChangeMode (seeds | run_all)
crates/cli/src/main.rs           # `changes` subcommand
crates/cli/tests/changes.rs      # e2e with tempdir git repos
```

---

### Task 1: Structural fingerprints

**Files:**
- Create: `crates/core/src/fingerprint.rs`; Modify: `crates/core/src/lib.rs`, `crates/core/Cargo.toml` (dev-deps)

**Interfaces:**
- Produces:

```rust
/// Hash of a subtree's STRUCTURE + token text, skipping comments and
/// whitespace: two subtrees fingerprint equal iff they are token-identical
/// modulo comments/formatting.
pub fn fingerprint(node: tree_sitter::Node, src: &[u8]) -> [u8; 32];

/// Fingerprint of `node` excluding its `body` field child (the "signature"),
/// and of the body alone. body=None when the node has no body field.
pub fn split_fingerprint(node: tree_sitter::Node, src: &[u8])
    -> (/*sig*/ [u8; 32], /*body*/ Option<[u8; 32]>);
```

Implementation: walk the subtree with a cursor; feed blake3 hasher with each node's `kind` bytes + a separator; for LEAF nodes also feed `node.utf8_text(src)`; skip any node whose kind contains `"comment"` (covers `comment`, `line_comment`, `block_comment` across all three grammars). Positions/whitespace never hashed. `split_fingerprint`: same walk but when the direct child is the `body` field child, skip it in the sig hash and fingerprint it separately.

- [ ] **Step 1: Add dev-deps for real-grammar tests**

Run: `cargo add -p testless-core --dev tree-sitter-typescript`

- [ ] **Step 2: Failing tests** (in fingerprint.rs `#[cfg(test)]`)

```rust
fn ts_tree(src: &str) -> (tree_sitter::Tree, Vec<u8>) {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    (p.parse(src, None).unwrap(), src.as_bytes().to_vec())
}

#[test]
fn comment_and_formatting_insensitive() {
    let (a, sa) = ts_tree("export function add(a: number, b: number) { return a + b; }");
    let (b, sb) = ts_tree("// docs\nexport function add(a: number,\n    b: number) {\n  /* inner */ return a + b;\n}");
    assert_eq!(fingerprint(a.root_node(), &sa), fingerprint(b.root_node(), &sb));
}

#[test]
fn token_change_changes_fingerprint() {
    let (a, sa) = ts_tree("function f() { return 1; }");
    let (b, sb) = ts_tree("function f() { return 2; }");
    assert_ne!(fingerprint(a.root_node(), &sa), fingerprint(b.root_node(), &sb));
}

#[test]
fn split_separates_signature_from_body() {
    let (a, sa) = ts_tree("function f(x: number) { return 1; }");
    let (b, sb) = ts_tree("function f(x: number) { return 2; }");
    let fa = a.root_node().named_child(0).unwrap();
    let fb = b.root_node().named_child(0).unwrap();
    let (sig_a, body_a) = split_fingerprint(fa, &sa);
    let (sig_b, body_b) = split_fingerprint(fb, &sb);
    assert_eq!(sig_a, sig_b);
    assert_ne!(body_a, body_b);
    assert!(body_a.is_some());
}
```

- [ ] **Step 3:** Verify fail, implement, pass; workspace + clippy + fmt.
- [ ] **Step 4:**

```bash
git add -A && git commit -m "feat(core): structural fingerprints, comment/format-insensitive"
```

---

### Task 2: Def-level diff

**Files:**
- Create: `crates/core/src/diffdef.rs`; Modify: `crates/core/src/lib.rs`, `crates/core/src/language.rs`

**Interfaces:**
- `ExtractedDef` gains two fields (serde; all langs must populate — see Step 1): `sig_hash: [u8;32]`, `body_hash: Option<[u8;32]>` — computed AT EXTRACTION TIME by each language via `fingerprint::split_fingerprint` on the def's node (ModuleInit: `sig_hash` = fingerprint over all top-level non-def, non-comment, non-import children — the "loose code"; body None).
- Produces:

```rust
pub enum DefChange {
    Added { new_idx: usize },
    Removed { old_name: String, old_kind: DefKind },
    BodyChanged { new_idx: usize },
    SigChanged { new_idx: usize },       // signature OR both sig+body
    ModuleInitChanged,                    // top-level loose code differs
}

/// Match old vs new extraction defs by identity key and compare hashes.
pub fn diff_defs(old: &Extraction, new: &Extraction) -> Vec<DefChange>;
```

Matching: identity key = `(kind, test_id.clone().unwrap_or([name]))` — TestCases by chain, others by (kind, name). Same-key collisions (overloads/same-name): pair in order; unpaired remainder → Added/Removed. Unchanged (same sig+body hashes) → no entry. Cache format: `ExtractedDef` shape change → bump magic TST3→TST4.

- [ ] **Step 1:** This task changes `ExtractedDef` — mechanical ripple into all three lang crates: each `push_def`-style helper computes `split_fingerprint(node, src)` for the def node; ModuleInit hash per the rule above (helper in fingerprint.rs: `pub fn module_init_fingerprint(root: Node, src: &[u8], skip: &dyn Fn(&tree_sitter::Node) -> bool) -> [u8;32]` where each lang passes a closure marking its def/import node kinds as skipped). Update Fake language + core tests.
- [ ] **Step 2: Failing tests** (diffdef.rs, using TS grammar dev-dep + TsLanguage? — core can't dep on lang-ts. Use handcrafted `Extraction` values instead — diff_defs operates on Extractions, no parsing needed):

```rust
fn def(name: &str, kind: DefKind, sig: u8, body: Option<u8>) -> ExtractedDef {
    ExtractedDef { name: name.into(), kind, start_line: 1, end_line: 2, test_id: None,
        computed_name: false, parent: None, sig_hash: [sig; 32], body_hash: body.map(|b| [b; 32]) }
}
fn ex(defs: Vec<ExtractedDef>) -> Extraction {
    Extraction { defs, imports: vec![], calls: vec![], reads: vec![] }
}

#[test]
fn classifies_added_removed_body_sig_changes() {
    let old = ex(vec![def("<module>", DefKind::ModuleInit, 0, None),
                      def("a", DefKind::Function, 1, Some(1)),
                      def("b", DefKind::Function, 2, Some(2)),
                      def("gone", DefKind::Function, 3, Some(3))]);
    let new = ex(vec![def("<module>", DefKind::ModuleInit, 0, None),
                      def("a", DefKind::Function, 1, Some(9)),   // body changed
                      def("b", DefKind::Function, 9, Some(2)),   // sig changed
                      def("fresh", DefKind::Function, 4, Some(4))]);
    let changes = diff_defs(&old, &new);
    assert!(changes.iter().any(|c| matches!(c, DefChange::BodyChanged { new_idx } if new[*new_idx].name == "a")));
    // (write the remaining assertions analogously: SigChanged for b, Added for fresh,
    //  Removed for gone, NO ModuleInitChanged, and an unchanged-def yields no entry)
}

#[test]
fn module_init_change_detected() {
    let old = ex(vec![def("<module>", DefKind::ModuleInit, 0, None)]);
    let new = ex(vec![def("<module>", DefKind::ModuleInit, 7, None)]);
    assert!(matches!(diff_defs(&old, &new)[..], [DefChange::ModuleInitChanged]));
}
```

(Adapt indexing sugar as needed — `new[*new_idx]` means `new.defs[*new_idx]`.)

- [ ] **Step 3:** Verify fail, implement, pass. Also one integration-level invariance test in `crates/cli/tests/changes.rs` (created now, grows in Task 5): parse `fixtures/ts-app/src/math.ts` twice — original vs comment-reformatted variant string — through `TsLanguage::extract`, assert `diff_defs` returns `[]`.
- [ ] **Step 4:** Workspace + clippy + fmt; cache magic bumped TST4 with test updated.

```bash
git add -A && git commit -m "feat(core): def-level structural diff with sig/body split"
```

---

### Task 3: Git integration

**Files:**
- Create: `crates/core/src/gitio.rs`; Modify: `crates/core/src/lib.rs`

**Interfaces:**

```rust
pub enum FileStatus { Added, Modified, Deleted, Renamed { old: PathBuf } }
pub struct ChangedFile { pub path: PathBuf, pub status: FileStatus }

/// `git diff --name-status -M --no-renames?? (keep -M)` from..to; to=None → worktree
/// (uncommitted + staged) PLUS untracked files (`git ls-files --others
/// --exclude-standard`) reported as Added.
pub fn changed_files(repo: &Path, from: &str, to: Option<&str>) -> anyhow::Result<Vec<ChangedFile>>;

/// Content of `path` at `rev` (`git show rev:path`); None if absent at rev.
pub fn show_file(repo: &Path, rev: &str, path: &Path) -> anyhow::Result<Option<String>>;
```

All failures (`git` missing, bad rev) → `Err` with context — classifier maps to RunAll.

- [ ] **Step 1: Failing tests** (gitio.rs `#[cfg(test)]`, tempdir): helper `fn git(dir, args)` runs git with `-c user.email=t@t -c user.name=t`; build repo: init, write a.txt, add+commit; modify a.txt + write untracked b.txt. Assert: `changed_files(dir, "HEAD", None)` reports a.txt Modified AND b.txt Added; `show_file(dir, "HEAD", a.txt)` = original content; `show_file` for b.txt at HEAD → None; `changed_files` with bad rev → Err. Also commit a rename (`git mv`) and assert `Renamed { old }` via a rev-range call.
- [ ] **Step 2:** Verify fail, implement (`std::process::Command`, `--name-status -M -z` parsing — `-z` NUL separation avoids path-quoting bugs; R status lines carry two paths), pass; workspace + clippy + fmt.
- [ ] **Step 3:**

```bash
git add -A && git commit -m "feat(core): git changed-file listing and rev file content"
```

---

### Task 4: Change classifier

**Files:**
- Create: `crates/core/src/classify.rs`; Modify: `crates/core/src/lib.rs`

**Interfaces:**

```rust
pub enum SeedKind { Body, Signature, Added, ModuleInit }
pub struct Seed { pub def: DefId, pub kind: SeedKind }
pub enum ChangeMode { Selection(Vec<Seed>), RunAll { reason: String } }

/// Classify per spec. `new_graph` is the freshly indexed worktree/to-rev graph;
/// `old_extraction_of` yields the from-rev extraction for a changed file
/// (parsing old content with the same Language), or Err → RunAll.
pub fn classify(
    repo: &Path,
    new_graph: &Graph,
    registry: &Registry,
    changed: &[ChangedFile],
    old_src_of: &dyn Fn(&Path) -> anyhow::Result<Option<String>>,
) -> ChangeMode;
```

Rules (spec table, in precedence order):
1. Any changed file matching config globs → `RunAll` immediately. Globs: `package.json`, `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`, `tsconfig*.json`, `go.mod`, `go.sum`, `Cargo.toml`, `Cargo.lock`, `.env*` (basename match, any dir).
2. Changed file is INDEXED (in new_graph.files, or was indexable by extension):
   - Deleted/Renamed-away → seed: nothing directly (file gone from new graph); its former importers: new-graph files whose ImportRef… simplest sound rule: for Deleted, seed EVERY module_init of files that currently have `Calls{to: Unknown}` edges? No — deleted file's importers now have unresolved imports; rule: Deleted/Renamed old path → `RunAll`?? too wide. Chosen rule (document it): Deleted code file → seed the module_init of every new-graph file whose raw ImportRefs are no longer resolvable… **Decision for this plan: Deleted/Renamed-away code file → `RunAll { reason: "deleted source file" }`.** Rare event, always sound; precision upgrade later. (Renamed: the NEW path is processed as Added/Modified normally; the disappearance of the old path triggers this rule ONLY for plain Deleted — for `Renamed{old}` treat old-path disappearance as covered by the new path's diff + import re-resolution, do NOT RunAll.)
   - Added → all its TestCase defs seeded `Added`; its module_init seeded `Added` (new exports; spec rule).
   - Modified: parse old content (via `old_src_of` + registry language for path; parse+extract) → `diff_defs(old, new)` → map: `BodyChanged{new_idx}` → Seed Body; `SigChanged` → Seed Signature; `Added` → Seed Added; `ModuleInitChanged` → Seed ModuleInit(file's module_init def); `Removed` → seed file's module_init (Seed ModuleInit) — importers still re-select, sound per import edges. new_idx → DefId via graph def offsets (defs_in_file + extraction order alignment).
3. Changed file NOT indexed and not config: if any indexed file's ImportRef raw text contains the changed file's stem (e.g. `./config.json` import of `config.json`) → seed those files' module_inits; else → zero seeds (README.md case).
4. Any error anywhere (old parse fails, unreadable, git) → `RunAll` with reason.
5. Zero-seed result with nonempty `changed` is VALID (comment-only edits, docs).

- [ ] **Step 1: Failing unit tests** (classify.rs): build a small in-memory graph via `index_repo` over a tempdir TS fixture (core can use `tests_support::Fake`? No — classification needs real language; put integration tests in `crates/cli/tests/changes.rs` where langs link; classify.rs keeps only pure-logic unit tests: config-glob matching table + precedence (config beats everything, error → RunAll with reason text).
- [ ] **Step 2:** cli/tests/changes.rs integration tests (registry available there): tempdir TS project + git; scenarios: (a) body edit of `add` → exactly one Seed Body naming add's def; (b) comment-only edit → `Selection([])`; (c) `package.json` edit → RunAll; (d) new test file → its TestCase seeds Added; (e) delete source file → RunAll; (f) README edit → `Selection([])`.
- [ ] **Step 3:** Verify fail, implement, pass; workspace + clippy + fmt.
- [ ] **Step 4:**

```bash
git add -A && git commit -m "feat(core): change classification to impact seeds"
```

---

### Task 5: `testless changes` command

**Files:**
- Modify: `crates/cli/src/main.rs`, `crates/cli/tests/changes.rs`, `README.md`

**Behavior:**
- `testless changes [--from <rev>] [--to <rev>]` — defaults `--from HEAD`, to=worktree. Flow: incremental index of current tree (existing machinery; NOTE: for `--to <rev>` ≠ worktree, v1 punts: error "not yet supported" — worktree + rev-from covers local loop AND CI base..head? CI wants `--from origin/main --to HEAD` with HEAD checked out = worktree. Acceptable; document).
- Runs gitio.changed_files + classify. Output JSON (piped): `{"version":1,"mode":"selection","seeds":[{"def":"add","file":"src/math.ts","kind":"body"},...],"stats":{...}}` or `{"mode":"run_all","reason":"..."}`. Human (TTY): readable list. Exit codes: 0 selection (incl. empty), 2 run_all, 1 hard error.
- README status: "Change detection (structural AST diff)" 🚧 → ✅.
- after_help gains a `changes` example; completions pick the subcommand up automatically.

- [ ] **Step 1: Failing e2e tests** (changes.rs, `assert_cmd`): git-tempdir TS fixture; (a) edit add's body → JSON mode=selection, seeds contains {def:"add",kind:"body"}, exit 0; (b) comment-only edit → seeds:[] exit 0; (c) package.json edit → mode=run_all exit 2.
- [ ] **Step 2:** Verify fail, implement, pass; workspace + clippy + fmt.
- [ ] **Step 3:**

```bash
git add -A && git commit -m "feat(cli): changes command — classified impact seeds"
```

---

### Task 6: Multi-language change coverage + dogfood

**Files:**
- Modify: `crates/cli/tests/changes.rs`, `.github/workflows/ci.yml`

**Behavior:**
- e2e scenarios for Go + Rust mirrors of Task 5(a)/(b): edit `Add` body in a git-tempdir go-app copy → seed `{def:"Add",kind:"body"}`; comment-only Rust edit → empty seeds.
- CI dogfood job gains: `./target/release/testless changes --from HEAD | jq -e '.mode == "selection"'` (HEAD vs clean worktree = zero changes → selection with empty seeds; proves the command runs on a real repo in CI).

- [ ] **Step 1:** Failing tests → implement → pass; actionlint on ci.yml.
- [ ] **Step 2:**

```bash
git add -A && git commit -m "test: multi-language change scenarios; dogfood changes in CI"
```

---

## Done criteria (Plan 3)

- `testless changes` end-to-end: body edit → precise def seed; comment/format-only edit → zero seeds (the marquee spec behavior); config edit → run_all exit 2
- Innermost-def attribution: diff works at def granularity via extraction matching — no span-overlap logic anywhere
- All three languages covered by e2e scenarios; deletion → RunAll documented decision
- Cache magic TST4; workspace green, clippy/fmt clean; CI (incl. new dogfood step) green
