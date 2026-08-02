# Contributing

## Dev setup

```bash
nix develop        # or: rustup toolchain install stable
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt
```

All three (test, clippy, fmt) must pass clean before a PR is opened.

## Ground rules

- **TDD.** Write a failing test first, then make it pass. No production code
  without a red test that justifies it.
- **Over-approximation is the contract.** When static analysis can't resolve
  something (dynamic dispatch, reflection, a mock), widen the selected set:
  never narrow it to look tidy. A false positive costs CI minutes; a false
  negative ships a bug.
- **Conventional commits, required.** `feat:`, `fix:`, `docs:`, `chore:`,
  etc.: release-please parses these to cut releases and changelogs, so
  malformed subjects break automation, not just style.

## Adding a language

A language plugin implements the `Language` trait (`crates/core/src/language.rs`).
Six things per language, everything else is shared:

1. **Grammar**: the tree-sitter `Language` for the file's extension(s).
2. **Extraction queries**: walk the tree and emit `ExtractedDef`s (functions,
   methods, tests) plus `ImportRef`s.
3. **Import resolution**: turn a raw import specifier into a repo-relative
   path, or `None` if it's external/unresolvable. Returning a *directory*
   fans out to every indexed file under it (Go packages, Java wildcards).
4. **Test-ID construction**: build the dotted/segmented ID a test runner
   would recognize (including subtests, e.g. Go's `t.Run` chains).
5. **Package scope** (`package_key`): return a key when files sharing it see
   each other with no import statement. Default `None` means file-scoped
   (TS, Rust). Go keys on the directory; Java keys on the package with the
   source root stripped, so `src/main/java/com/foo` and
   `src/test/java/com/foo` are one scope.
6. **Over-approximation triggers**: the specific shapes in this language
   that are ambiguous enough to widen selection rather than guess narrowly.

If the language has more than one test runner (Java's Maven vs Gradle), the
mapping lives in `crates/cli/src/runner.rs`, not in the plugin: it depends on
build files, not on syntax.

`crates/lang-go/src/lib.rs` is the smallest reference implementation, read
it before starting a new one.

Each new language needs:

- A fixture app under `fixtures/<lang>-app/` (small, real source; see
  `fixtures/go-app/` for the shape: a couple of packages, plain functions,
  and at least one test file with a nested subtest).
- Extraction tests asserting the exact `Extraction` (defs + imports) produced
  for that fixture: names, kinds, spans, parent links, and test IDs, not
  just "it didn't panic."
