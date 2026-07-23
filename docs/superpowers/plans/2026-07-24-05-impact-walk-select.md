# testless Plan 4: Impact Walk + `select` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `testless select [--from rev] [--format json|text|args]` — the tool's entire point: from change seeds, walk the graph to the impacted tests and emit per-test selections runners can consume.

**Architecture:** Reverse reachability in `core::walk`: seeds → callers/readers/parents (reverse Calls/Reads/Contains), `Unknown(name)` widening (tests calling an unresolvable name matching an impacted def select too), ModuleInit seeds → transitive importer files' tests. CLI `select` = changes pipeline + walk + output formats. Failure posture: run_all passthrough (exit 2).

**Tech Stack:** existing workspace, no new deps.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-23-testless-design.md` §"Impact walk" + §CLI/JSON output — authoritative
- Over-approximation: widening rules are mandatory (Unknown-by-name, computed_name, module_init transitive importers). Zero silent drops
- Exit codes: 0 selection (incl. empty), 2 run_all, 1 hard error. Machine stdout / human stderr split maintained
- Conventional commits; branch `plan-4-select` off main; fmt/clippy/tests CI-enforced

## File Structure

```
crates/core/src/walk.rs        # impacted_tests reverse reachability
crates/cli/src/main.rs         # select subcommand + output formats
crates/cli/src/format.rs       # runner arg generation (vitest/go test/cargo test)
crates/cli/tests/select.rs     # e2e across three languages
README.md                      # select ✅, Quickstart gains select
.github/workflows/ci.yml       # dogfood select steps
```

---

### Task 1: Impact walker

**Files:**
- Create: `crates/core/src/walk.rs`; Modify: `crates/core/src/lib.rs`

**Interfaces:**

```rust
/// All TestCase defs impacted by `seeds`, per the spec's reverse-reachability
/// rules. Deterministic order (ascending DefId).
pub fn impacted_tests(graph: &Graph, seeds: &[Seed]) -> Vec<DefId>;
```

Algorithm:
1. Build reverse indexes once: `callers: HashMap<DefId, Vec<DefId>>` (from `Edge::Calls{from, to: Resolved(t)}` → callers[t].push(from)); `readers` likewise from Reads; `parents_of_child⁻¹`: from `Contains{parent, child}` → `container[child] = parent`... reverse reach means: child impacted → parent impacted (parent's behavior embeds child) → `containers: HashMap<DefId, DefId>` child→parent; `unknown_by_name: HashMap<&str, Vec<DefId>>` (Calls Unknown(name) → callers-by-name); `importers: HashMap<FileId, Vec<FileId>>` (Imports{from,to} → importers[to].push(from)).
2. Worklist BFS over DefIds starting from all `seeds[].def` (visited set):
   - visit D: if `graph.def(D).kind == TestCase` → collect (and CONTINUE walking — a test helper called by other tests keeps propagating via callers).
   - enqueue `callers[D]`, `readers[D]`, `containers[D]` (child→parent).
   - enqueue `unknown_by_name[short_name(D)]` — any def calling an unresolvable name matching D's short name (methods: last `.` segment).
   - if D is a ModuleInit def: for every file in the transitive `importers` closure of D's file (including D's own file): enqueue that file's ModuleInit AND collect all TestCase defs of that file (tests run import side effects).
3. Deterministic: sort result ascending.

Note: `Seed.kind` is not consulted in v1 — Body/Signature/Added walk identically (reverse edges already reach referencers); kinds exist for future precision and output.

- [ ] **Step 1: Failing unit tests** (walk.rs, hand-built graphs — no parsing):

```rust
// helpers: g() builds Graph via add_file/add_def/add_edge; def(name, kind, file)
#[test]
fn caller_chain_reaches_test() {
    // add <- calculate <- test "t1"; seed add(Body) → [t1]
}
#[test]
fn unknown_name_widens_to_matching_callers() {
    // test t2 has Calls{Unknown("add")}; seed def add → t2 selected
}
#[test]
fn module_init_seed_selects_transitive_importer_tests() {
    // file A (module_init M) <- imports - file B <- imports - file C(test t3)
    // seed M → t3 selected (via importer closure), tests in A too
}
#[test]
fn reads_edge_reaches_test() { /* test reads CONST; seed CONST def → test */ }
#[test]
fn contains_reverse_reaches_parent() {
    // class C contains method m; test calls... m's parent C: seed m → C enqueued;
    // test calling C (constructor) → selected
}
#[test]
fn test_helper_propagation() {
    // helper TestCase h called by test t; seed something reaching h → both h and t collected
}
#[test]
fn deterministic_order_and_no_dupes() { /* two seeds reaching same test → once, sorted */ }
```

Write each with a small explicit graph (module_init def per file at minimum where the scenario needs it). Every test must fail before implementation (RED via `todo!()` body).

- [ ] **Step 2:** Implement, pass; workspace + clippy + fmt.
- [ ] **Step 3:**

```bash
git add -A && git commit -m "feat(core): reverse-reachability impact walk with widening rules"
```

---

### Task 2: `select` command (JSON/text)

**Files:**
- Modify: `crates/cli/src/main.rs`; Create: `crates/cli/tests/select.rs`

**Behavior:**
- `Select { from: String (default HEAD), to: Option<String> (reject like changes), format: Format (json|text|args, default auto: json piped / text TTY) }`.
- Flow: index (incremental+cache, diff-before-save ordering like changes) → changed_files → classify → match mode:
  - RunAll{reason} → output `{"version":1,"mode":"run_all","reason":...}` exit 2
  - Selection(seeds) → `walk::impacted_tests` → per-test entries:

```json
{"version":1,"mode":"selection","tests":[
  {"file":"src/math.test.ts","name":["add","handles negatives"],"runner":"vitest","lang":"ts","computed":false}
],"stats":{"total_known":N,"selected":N,"seeds":N,"changed_files":N}}
```

  - `name` = test_id chain; `computed` = computed_name (consumer must widen pattern). `runner`: lang "ts"→"vitest", "go"→"gotest", "rust"→"cargo". `file`: the def's file path (Go: the PACKAGE DIR path of the file — emit file path as-is; runner formatting handles dir). `total_known` = all TestCase defs in graph.
- text format: one line per test `file :: name-joined-with-" > "` + summary footer line on stderr.
- Exit 0 for selection (incl. empty).

- [ ] **Step 1: Failing e2e** (select.rs, git-tempdir TS repo like changes.rs tests): commit fixture with `add` + `format` + tests; edit `add` body → select JSON: mode selection, tests include math.test.ts entries whose name[0]=="add" AND format.test.ts's "formats" (calls fmt→add), NOT an unrelated test (include an `unrelated.test.ts` with a test not touching add — assert absent). Comment-only edit → tests:[] exit 0. package.json edit → run_all exit 2.
- [ ] **Step 2:** Verify fail, implement, pass; workspace + clippy + fmt.
- [ ] **Step 3:**

```bash
git add -A && git commit -m "feat(cli): select command — impacted-test selection output"
```

---

### Task 3: `--format args` runner invocations

**Files:**
- Create: `crates/cli/src/format.rs`; Modify: `crates/cli/src/main.rs`, `crates/cli/tests/select.rs`

**Behavior:** group selected tests per (runner, file/pkg) and emit newline-separated commands:
- vitest: `vitest run <file> -t "<name joined with ' > '>"` — multiple tests same file → one invocation per test (simple v1). computed=true → drop `-t` (run whole file — widened).
- gotest: file path → package dir (parent of _test.go file); `go test ./<dir> -run '^Root$/^sub$'` (escape regex metachars in segments; computed segment `<computed>` → `.*`).
- cargo: `cargo test '<chain joined with ::>' -- --exact` ; computed (rare) → drop `--exact`.
- Pure string generation in format.rs — unit tests over hand-built selections (all three runners, computed cases, regex escaping `TestAdd$weird`). e2e: one assertion that `--format args` on the TS scenario emits `vitest run` lines.

- [ ] **Step 1:** Failing unit + e2e tests → implement → pass; workspace + clippy + fmt.
- [ ] **Step 2:**

```bash
git add -A && git commit -m "feat(cli): args format — runner-consumable invocations"
```

---

### Task 4: Go + Rust e2e select scenarios

**Files:**
- Modify: `crates/cli/tests/select.rs`

Scenarios (git-tempdir, mirroring Task 2's TS shape):
- Go: edit `Add` body → selection includes `TestAdd` subtests + `TestFmt` (cross-package caller), excludes an unrelated test in a third package. `--format args` emits `go test` line with `-run '^TestAdd$'`-style pattern.
- Rust: edit `add` body → selection includes `tests::add_works` and fmt's test via cross-module call; comment-only Rust edit → empty.
- Module-init scenario (any lang, TS simplest): edit top-level `console.log` line → all importer-file tests selected.

- [ ] **Step 1:** Failing tests → verify → pass (walker/CLI already built; failures here mean real bugs — fix in this task); workspace + clippy + fmt.
- [ ] **Step 2:**

```bash
git add -A && git commit -m "test: cross-language select scenarios incl. module-init widening"
```

---

### Task 5: Docs + CI dogfood + release polish

**Files:**
- Modify: `README.md`, `.github/workflows/ci.yml`, `crates/cli/src/main.rs` (after_help)

- README: status row "Impact walk + `select`" 🚧 → ✅; Quickstart gains `testless select --from origin/main | jq` and `testless select --from origin/main --format args`; tagline section updated ("run 5 tests instead of 500" now literal).
- CI dogfood: `./target/release/testless select --from HEAD | jq -e '.mode=="selection" and (.tests|length)==0'` (clean tree) + a REAL selection smoke: `git diff HEAD~1 --quiet || ./target/release/testless select --from HEAD~1 | jq -e '.mode=="selection" or .mode=="run_all"'` (prior commit's changes produce valid output either way).
- after_help: select examples.
- actionlint clean.

- [ ] **Step 1:** Implement; run dogfood steps locally verbatim; actionlint; workspace green.
- [ ] **Step 2:**

```bash
git add -A && git commit -m "docs+ci: select in README quickstart and dogfood"
```

---

## Done criteria (Plan 4)

- `testless select --from <rev>` end-to-end in all three languages: precise per-test selection including cross-file/package/module callers; unrelated tests excluded; comment-only → empty; config → run_all exit 2
- Widening verified: Unknown-name callers selected, computed tests widen patterns, module-init changes select transitive importer tests
- `--format args` emits valid vitest/go test/cargo test invocations (regex-escaped)
- README truthful; CI dogfoods select; workspace/clippy/fmt/actionlint clean
- THE ORIGINAL PITCH WORKS: edit one function → get the handful of tests that matter
