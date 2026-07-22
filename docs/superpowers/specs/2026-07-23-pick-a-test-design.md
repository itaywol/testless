# pick-a-test — Design Spec

Date: 2026-07-23
Status: approved pending review

## Problem

Running a full test suite after every change is wasteful. Existing tools select at
file granularity (`jest --findRelatedTests`, `vitest --changed`, Nx `affected`) or
require a paid platform with runtime coverage collection (Wallaby, Datadog Test
Impact Analysis). Nothing free and local selects at **function level** via static
analysis.

`pick-a-test` is a Rust CLI that, given a change (worktree diff or rev range),
outputs the set of individual tests impacted by that change, for test runners to
consume. Launch languages: TypeScript and Go, built simultaneously to keep the
engine honest. New languages must be cheap to add.

## Core decisions

| Decision | Choice | Rationale |
|---|---|---|
| Engine | Static AST analysis (no runtime coverage) | User choice; zero instrumentation, works cold |
| Soundness | **Over-approximate** — never silently skip an impacted test | Trust. Selected set ⊇ truly impacted set, always |
| Parsing | tree-sitter core + thin per-language resolvers | Uniform frontend; new language ≈ grammar + resolver |
| Precision tiers | Tier 1: name/scope heuristics (always). Tier 2: SCIP index sharpens call edges (optional) | SCIP indexers (scip-typescript, scip-go) reuse real compilers' resolution; stack-graphs is archived — not used |
| Change detection | **Structural diff** (old tree vs new tree per file), not line ranges | Formatting/comment-only edits select zero tests; changes map to exact def nodes |
| Output granularity | Per-test IDs, structured JSON (+ text, + runner-arg format) | The point of the tool; file-level already exists elsewhere |
| Failure posture | Any internal error → emit `run_all` selection with reason | Tool failure must never mean tests skipped |
| Storage | In-memory graph, serialized to one cache file. No graph database | Repo-scale graphs are small; reverse BFS is trivial |

## Architecture

Rust workspace:

```
pick-a-test/
├── crates/
│   ├── core/      # language-agnostic engine
│   ├── lang-ts/   # TypeScript resolver plugin
│   ├── lang-go/   # Go resolver plugin
│   └── cli/       # pick-a-test binary
├── fixtures/
│   ├── ts-app/    # nasty-on-purpose TS fixture repo
│   └── go-app/    # nasty-on-purpose Go fixture repo
└── docs/
```

### `core` responsibilities

- File discovery, tree-sitter parse orchestration
- Graph build + incremental update (re-parse only content-hash-changed files)
- Structural differ (old tree vs new tree → changed defs)
- Impact walker (reverse reachability + widening rules)
- Cache persistence (single file under `.pick-a-test/`, bincode/rkyv)
- Output serialization
- Git integration (diff file list, rev range, rename detection)

### `Language` trait (the plugin boundary)

Each language supplies exactly:

1. tree-sitter grammar handle
2. Queries: definitions, call sites, imports, test declarations
3. Import path resolution (tsconfig paths / relative / node_modules ↔ Go module paths)
4. Test-ID construction (`file > describe > it` chain ↔ `TestXxx/subtest`)
5. Over-approximation triggers (`jest.mock`, dynamic `import()`, interface methods, reflection)

Optionally: a SCIP index reader mapping occurrences onto call/read edges (tier 2).

## Graph schema

Nodes (all carry `file`, `span`, `content_hash`):

- `Def` — function | method | class | test_case | `module_init`
  (`module_init` = the file's top-level executable code, one per file)
- `Test` — subtype of `Def` with id chain, e.g. `["math.test.ts", "add", "handles negatives"]`
- `File` — owns defs; hash gates re-parse

Edges:

- `contains` — File → Def, Def → Def (nesting, describe → it)
- `calls` — Def → Def (resolved call site; tier 1 or tier 2). Unresolved → `calls Unknown(name)`
- `imports` — File → File (always recorded, even when calls resolve)
- `reads` — Def → Def (references to non-function symbols: consts, class fields, types)

Schema rules:

- Top-level code change → `module_init` seed → every transitive importer impacted
  (catches side-effects-at-import).
- Type/interface/struct shape change → over-approximate: impacts all `reads` referencers.
- `Unknown(name)` widens to all defs with that name; SCIP tier eliminates most.

## Change classification → impact seeds

| Change (from structural diff) | Impact seeds |
|---|---|
| Def body edited | that def |
| Def signature/type edited | that def + all `reads`/`calls` referencers |
| Def added | its file's `module_init` |
| Def deleted/renamed | old def's referencers (cached graph knows old edges) |
| Top-level code edited | file's `module_init` |
| Comment/formatting only | nothing |
| Config files (`package.json`, `tsconfig*`, `go.mod`, `go.sum`, lockfiles, `.env*`) | **select all** |
| Non-code file imported by code (JSON, CSS, assets) | importers' `module_init` |
| Test file edited | changed test cases directly |

## Impact walk

Reverse BFS from seeds over `calls⁻¹`, `reads⁻¹`, `contains⁻¹`; `module_init`
seeds also follow `imports⁻¹`. Stop at `Test` nodes; collect. Visited-set
cycle-safe (recursion, circular imports are normal).

Widening during walk:

- `Unknown(name)` edge → seed every def named `name`
- Mocked module path (`jest.mock`/`vi.mock`) recorded as `imports` edge from the
  test file → mock target change selects the test
- Go interface method call, unresolved receiver → seed the method on **all**
  implementing types

Escape-hatch config `pickatest.toml`: `always-run` globs (smoke tests), `ignore`
globs (generated code).

## CLI

```
pick-a-test select [--from <rev>] [--to <rev|WORKTREE>] [--format json|text|args]
pick-a-test index  [--full]     # explicit (re)build; select auto-indexes incrementally
pick-a-test why <test-id>       # print edge path from change to this test
pick-a-test stats               # graph size, cache health
pick-a-test completion <shell>  # zsh|bash|fish completions
```

Defaults: `--from HEAD --to WORKTREE`. CI usage: `--from origin/main --to HEAD`.

JSON output:

```json
{
  "version": 1,
  "mode": "selection",
  "tests": [
    { "file": "src/math.test.ts", "name": ["add", "handles negatives"], "runner": "vitest", "lang": "ts" },
    { "file": "pkg/calc", "name": ["TestAdd", "negatives"], "runner": "gotest", "lang": "go" }
  ],
  "stats": { "total_known": 1240, "selected": 17, "widenings": 3 }
}
```

`mode: "run_all"` (+ `reason`) on any internal failure. `--format args` emits
runner-consumable invocations (`vitest run <file> -t "<pattern>"`,
`go test <pkg> -run '^TestAdd$/^negatives$'`) — formatting only, tool never runs tests.

## CLI UX

First-class, not an afterthought. Standard crates, thin glue:

| Concern | Behavior | Crate |
|---|---|---|
| Help | `clap` derive: rich `--help` with an EXAMPLES section per subcommand; typo suggestions ("did you mean `select`?") | clap |
| Completions | `completion zsh\|bash\|fish` generates scripts; completes subcommands, flags, `--format` values | clap_complete |
| Man page | Generated at build time, shipped in release artifacts | clap_mangen |
| Progress | Indexing shows progress bar (files parsed, files/s, ETA) on **stderr**, TTY only; silent when piped | indicatif |
| Output modes | stdout TTY → human format: colored summary table, selection counts, timing footer (`17/1240 tests · 3 widenings · 42ms`). stdout piped → JSON automatically. Explicit `--format` always wins | anstream |
| Color | Respects `NO_COLOR`, `CLICOLOR_FORCE`, `--no-color` | anstream |
| Errors | Actionable, hint-suffixed: `not a git repository — run inside a repo`, `cache corrupt — rebuilt automatically`. Never a bare panic/backtrace | anyhow + custom display |
| `why` output | Colored edge-path tree: change → def → … → test, one hop per line | — |
| `stats` | At-a-glance dashboard: files/defs/edges counts, cache size + freshness, tier-2 (SCIP) availability per language, last index duration | — |
| Verbosity | `-q` (selection only, no footer), `-v` (walk decisions: seeds, widenings and their causes) | — |
| Exit codes | `0` selection produced · `2` run_all fallback (still valid output) · `1` hard error. Documented in `--help` | — |

Principles: machine output (stdout) and human chrome (stderr) never mix; every
degraded state prints *why* and *what the tool did about it*; zero-config first
run (`pick-a-test select` in any repo just works or says exactly what's missing).

## Testing strategy (TDD)

Development is test-first. Every walk rule and every edge case below gets a
failing test before implementation.

1. **Unit tests** — per-language resolver queries (def/call/import/test extraction),
   structural differ (formatting-only → empty diff), walk rules.
2. **Snapshot selection tests** — scripted fixture mutations (patches) → run
   `select` → snapshot the selected set. One scenario per classification rule and
   per edge case below.
3. **Ground-truth oracle** — harness runs the *full* fixture suite before/after a
   mutation with per-test coverage, computes actually-impacted tests, asserts
   `selected ⊇ actually-impacted`. Mechanical proof of the over-approximation
   guarantee.
4. **Real-repo smoke** — CI runs index+select against 1–2 mid-size OSS repos
   (vitest-based TS, a Go module); must complete without `run_all` fallback.

Perf budget: incremental `select` after a 1-file change on a ~100k-line fixture
< 200 ms (CI-checked). Cold `index` measured, not gated.

## Edge-case catalog (fixture requirements)

Each item becomes fixture code + a snapshot/oracle test. **Expected** states the
selection behavior that must hold.

### TypeScript / JavaScript

| # | Case | Expected |
|---|---|---|
| T1 | Barrel file `export * from './a'` chain | Change in `a` selects tests importing via barrel; unrelated barrel siblings not selected (tier 2) or widened (tier 1) |
| T2 | Re-export with rename `export { a as b }` | Rename tracked; consumers of `b` impacted by `a` change |
| T3 | Default export (anonymous fn/class) | Treated as named def per file; consumers impacted |
| T4 | CommonJS `require`/`module.exports` interop | Edges built same as ESM |
| T5 | `import()` with literal path | Import edge recorded |
| T6 | `import()` with computed path | Widen: importer's file → all files matching heuristic, else module_init of all candidates; never silently ignore |
| T7 | `jest.mock('./dep')` / `vi.mock` with factory | Test ↔ mocked path edge; changing `./dep` selects test; factory body change selects test |
| T8 | Partial mock (`requireActual` + override) | Both real and mock edges recorded |
| T9 | `vi.spyOn(obj, 'method')` | `reads` edge to `obj.method`'s def |
| T10 | Higher-order: fn passed as value, called elsewhere | `reads` edge at pass-site; over-approx impact at call-site via Unknown |
| T11 | Class inheritance + method override + `super.x()` | Base method change impacts subclass users; override shadows correctly |
| T12 | Arrow fn assigned to `const`, object-method shorthand | All recognized as defs |
| T13 | Namespace import `import * as ns; ns.fn()` | Resolves through namespace |
| T14 | tsconfig `paths` aliases (`@/lib/*`), `baseUrl` | Aliases resolve |
| T15 | Extensionless / `index.ts` / ESM `.js`-suffix-to-`.ts` imports | All resolve |
| T16 | `import type` / type-only re-export | Type change → `reads` referencers impacted; **no** runtime `module_init` impact |
| T17 | Interface/type alias shape change | All `reads` referencers impacted (over-approx) |
| T18 | Decorators on class/method | Decorator fn change impacts decorated def's dependents |
| T19 | Module-level side effect (registry pattern, singleton) | Top-level change → all transitive importers' tests |
| T20 | Shared mutable module state (exported `let`/object) | Writers and readers linked via `reads`; change to writer impacts reader tests |
| T21 | `test.each` / `it.each` tables | Table row change selects that test id (or whole `each` block if ids computed) |
| T22 | Computed test names (`` it(`handles ${x}`) ``) | Widen to enclosing describe/file — id not statically known |
| T23 | `describe.skip` / `.only` / `.todo` | Still tracked as tests; selection unaffected by skip status |
| T24 | Shared test helpers/utils imported by many test files | Helper change selects all consuming tests |
| T25 | Snapshot file (`.snap`) edited | Owning test file's tests selected |
| T26 | JSON / CSS-module / asset import | Asset change → importers' module_init |
| T27 | Monorepo workspace package dep (`workspace:*`) | Cross-package edges resolve within repo |
| T28 | `eval` / `new Function` | Unresolvable → widen (file-level) |
| T29 | `package.json` conditional exports map | Resolution honors exports map; on doubt, widen |
| T30 | Circular imports | Graph handles cycles; walk terminates; impacts propagate both ways |
| T31 | Getter/setter properties | Treated as method defs |
| T32 | Re-declared symbol names across files (same name, different fn) | Tier 1 Unknown-widening selects both; tier 2 selects one |

### Go

| # | Case | Expected |
|---|---|---|
| G1 | Implicit interface satisfaction | Method change on impl impacts interface call sites; interface call widens to all impls |
| G2 | Embedded struct / method promotion | Promoted method change impacts outer type's users |
| G3 | `init()` functions | Treated as `module_init`; change impacts all package importers |
| G4 | Package-level `var x = f()` initializer | Initializer change = module_init change |
| G5 | Table-driven tests with `t.Run(tc.name, ...)` | Computed subtest name → widen to parent `TestXxx` |
| G6 | `t.Run` with literal name | Precise subtest id |
| G7 | Build tags (`//go:build`) | Both variants indexed; change in either impacts dependents (over-approx across tags) |
| G8 | Generics (type params) | Instantiation-agnostic: generic fn change impacts all callers |
| G9 | Method values / fn values passed around | Same as T10: reads + Unknown widening |
| G10 | `reflect` usage | Unresolvable → widen to file/package level |
| G11 | Closures & goroutines | Contained in parent def; parent change semantics |
| G12 | `TestMain` | Change → all package tests selected |
| G13 | `Example*`, `Benchmark*`, `Fuzz*` funcs | Recognized as tests (selectable), typed by kind |
| G14 | External test package (`pkg_test`) | Edges cross the package pair correctly |
| G15 | Dot imports (`import . "pkg"`) | Names resolve into importing scope; on doubt widen |
| G16 | Generated code (`*.pb.go`, `//go:generate`) | Default ignore globs; changes there → importers' module_init unless ignored |
| G17 | Shared test helpers with `t.Helper()` | Helper change selects all calling tests |
| G18 | `internal/` packages | Normal resolution within module |

### Cross-cutting / repo-level

| # | Case | Expected |
|---|---|---|
| X1 | File renamed/moved (git rename detection) | Defs re-keyed; referencers of old path impacted once (import-path change), no phantom run_all |
| X2 | File deleted while tests import it | Referencing tests selected (they'll fail — correct) |
| X3 | New test file added | All its tests selected |
| X4 | New source file added, not yet imported | No tests selected (nothing depends on it) |
| X5 | Whitespace/CRLF-only change | Zero selection |
| X6 | Comment-only change (incl. code-looking text in comments) | Zero selection |
| X7 | Rev-range spanning merge commit | Diff computed base...head; selection = union of changes |
| X8 | Untracked new source file in worktree mode | Indexed and treated as added |
| X9 | Corrupt/stale cache file | Silent full rebuild, no failure |
| X10 | Symlinked source file | Resolved to real path once; no duplicate nodes |
| X11 | Unparseable changed source file (syntax error mid-edit) | `run_all` with reason — never guess |
| X12 | Mixed-language change (TS + Go in one diff) | Both resolvers run; unified selection output |
| X13 | Change only in `fixtures/`-style ignored glob | Zero selection |
| X14 | Same-name test ids in different files | IDs are file-qualified; no collision |

Catalog grows during implementation — any newly discovered quirk gets a fixture
+ test before its fix.

## Out of scope (v1)

- Running tests (output only; runners consume)
- Runtime coverage collection of any kind
- Languages beyond TS + Go
- Cross-repo dependencies (node_modules/external module internals — boundary
  crossing = module_init impact of the importer via lockfile/config rule)
- Watch mode / daemon (CLI invocations only; cache makes them fast)
- IDE integration
