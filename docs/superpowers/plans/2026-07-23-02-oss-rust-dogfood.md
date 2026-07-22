# testless Plan R: OSS Readiness + Rust Language + Dogfood Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Public-ready repo (LICENSE/README/CONTRIBUTING/SECURITY/CI/release pipeline), `testless-lang-rust` plugin, and a CI dogfood job running `testless index` on this repository.

**Architecture:** OSS files are data tasks. lang-rust mirrors lang-ts/lang-go (5-item `Language` contract). Dogfood = integration test indexing our own repo + CI job doing the same with the release binary.

**Tech Stack:** Existing workspace + tree-sitter-rust; GitHub Actions; release-please (rust releaser); cargo-dist (astral fork) or hand-rolled matrix fallback.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-23-oss-rust-dogfood-design.md` (engine rules from `2026-07-23-testless-design.md` still govern: over-approximate, never drop defs silently)
- Conventional commits MANDATORY from now on (release-please parses them)
- NixOS host: missing tools via `nix-shell -p <pkg> --run '...'` (actionlint, jq), never installed imperatively
- Deps via `cargo add`; bincode stays pinned 1.x
- Crate names: `testless-core`, `testless-lang-ts`, `testless-lang-go`, `testless-lang-rust`, `testless` (bin)
- TDD for all extraction code; data/config tasks verify by running the relevant tool (`nix flake show`, `actionlint`, `cargo publish --dry-run`)

## File Structure

```
LICENSE  README.md  CONTRIBUTING.md  SECURITY.md  .editorconfig
flake.nix  rust-toolchain.toml
.github/workflows/{ci.yml,release-please.yml,release.yml}
release-please-config.json  .release-please-manifest.json
crates/lang-rust/{Cargo.toml,src/lib.rs,tests/extract.rs}
fixtures/rust-app/{Cargo.toml,src/lib.rs,src/math.rs,src/fmt.rs,tests/integration.rs}
crates/cli/src/main.rs            # register RustLanguage
crates/cli/tests/{index.rs,dogfood.rs}
```

---

### Task 1: OSS document set

**Files:**
- Create: `LICENSE`, `README.md`, `CONTRIBUTING.md`, `SECURITY.md`, `.editorconfig`

**Interfaces:**
- Produces: public-facing docs. README status table is load-bearing honesty — do not claim select works.

- [ ] **Step 1: LICENSE** — standard MIT text, `Copyright (c) 2026 itay wolmarans`.

- [ ] **Step 2: README.md**

```markdown
# testless

> test less. — function-level test impact analysis via static AST.

[![CI](https://github.com/itaywol/testless/actions/workflows/ci.yml/badge.svg)](https://github.com/itaywol/testless/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![crates.io](https://img.shields.io/crates/v/testless.svg)](https://crates.io/crates/testless)

`testless` analyzes your code changes at the AST level and tells your test
runner which tests are actually impacted — so you run 5 tests instead of 500.
No runtime instrumentation, no coverage collection, no paid platform.

**Guarantee:** over-approximation. The selected set is always a superset of the
truly impacted set. When static analysis can't resolve something (dynamic
dispatch, mocks, reflection), testless widens the selection — it never silently
skips a test that could fail.

## Status

| Capability | State |
|---|---|
| Index: function-level graph of defs, tests, imports | ✅ |
| Incremental re-index (content-hash gated cache) | ✅ |
| Languages: TypeScript/JavaScript, Go, Rust | ✅ |
| Change detection (structural AST diff) | 🚧 |
| Impact walk + `select` (the point of the tool) | 🚧 |
| SCIP-sharpened call resolution | planned |

## Quickstart

    cargo install testless        # or: nix develop; cargo run -p testless
    cd your-repo
    testless index                # builds .testless/graph.bin
    testless stats                # what it knows
    testless completion zsh > ~/.zsh/completions/_testless

`index` output is JSON when piped, human-readable on a TTY. Machine output on
stdout, progress on stderr.

## How it works

    ┌────────────┐   ┌──────────────────────┐   ┌─────────────┐
    │ discovery  │ → │ tree-sitter extract  │ → │ graph cache │
    │ (gitignore │   │ defs / tests / import│   │  .testless/ │
    │  aware)    │   │ per-language plugin  │   │  (bincode)  │
    └────────────┘   └──────────────────────┘   └─────────────┘

A language plugin supplies five things: grammar, def/test/import queries,
import-path resolution, test-ID construction, and over-approximation triggers.
Everything else is language-agnostic. See [CONTRIBUTING](CONTRIBUTING.md).

## License

MIT
```

- [ ] **Step 3: CONTRIBUTING.md** — sections: Dev setup (`nix develop` or rustup stable; `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt`), Ground rules (TDD — failing test first; over-approximation principle with one-line explanation; conventional commits required, enforced by release-please), Adding a language (list the 5 `Language` trait items with one line each, point at `crates/lang-go/src/lib.rs` as the smallest reference implementation, fixture + extraction-test expectations).

- [ ] **Step 4: SECURITY.md** — report vulnerabilities via GitHub private security advisories on this repo; no bounty; best-effort response.

- [ ] **Step 5: .editorconfig**

```ini
root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true

[*.rs]
indent_style = space
indent_size = 4

[*.{toml,yml,yaml,md,json}]
indent_style = space
indent_size = 2
```

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "docs: add LICENSE, README, CONTRIBUTING, SECURITY, editorconfig"
```

---

### Task 2: Nix devshell + toolchain pin

**Files:**
- Create: `flake.nix`, `rust-toolchain.toml`

- [ ] **Step 1: rust-toolchain.toml**

```toml
[toolchain]
channel = "stable"
components = ["clippy", "rustfmt", "rust-analyzer"]
```

- [ ] **Step 2: flake.nix**

```nix
{
  description = "testless dev shell";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.flake-utils.url = "github:numtide/flake-utils";
  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let pkgs = nixpkgs.legacyPackages.${system}; in {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [ rustc cargo clippy rustfmt rust-analyzer ];
        };
      });
}
```

- [ ] **Step 3: Verify**

Run: `nix flake show path:. 2>&1 | tail -5`
Expected: devShells listed, no eval errors. (`path:.` avoids dirty-git warnings.)

- [ ] **Step 4: Commit**

```bash
git add flake.nix flake.lock rust-toolchain.toml && git commit -m "build: nix devshell and stable toolchain pin"
```

---

### Task 3: Crate publish metadata

**Files:**
- Modify: root `Cargo.toml`, all five `crates/*/Cargo.toml`

- [ ] **Step 1:** In root `[workspace.package]` add:

```toml
description = "Function-level test impact analysis via static AST"
repository = "https://github.com/itaywol/testless"
```

In each crate's `[package]`: add `description.workspace = true`, `repository.workspace = true`, `license.workspace = true` (license already in workspace.package). Give the four lib crates their own one-line `description` overrides (e.g. "testless graph core", "TypeScript/JavaScript language plugin for testless", etc. — override = plain `description = "..."` instead of workspace inheritance).

- [ ] **Step 2: Verify**

Run: `cargo publish --dry-run -p testless-core --allow-dirty 2>&1 | tail -3`
Expected: "warning: aborting upload due to dry run" (packaging succeeds). Note: dependent crates can't fully dry-run until core is published — only verify core packages cleanly + `cargo package --list -p testless -q | head` shows no unexpected files.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "build: crate metadata for crates.io publishing"
```

---

### Task 4: rust-app fixture seed

**Files:**
- Create: `fixtures/rust-app/{Cargo.toml,src/lib.rs,src/math.rs,src/fmt.rs,tests/integration.rs}`

Fixture is DATA — never compiled, excluded from workspace (root workspace `members` doesn't glob fixtures; verify `cargo metadata` doesn't pick it up).

- [ ] **Step 1: Write files**

`fixtures/rust-app/Cargo.toml`:

```toml
[package]
name = "rust-app"
version = "0.1.0"
edition = "2021"
```

`src/lib.rs`:

```rust
pub mod math;
pub mod fmt;

pub static REGISTRY: &str = "side effect analog";
```

`src/math.rs`:

```rust
pub fn add(a: i64, b: i64) -> i64 { a + b }

pub struct Calc { pub total: i64 }

impl Calc {
    pub fn push(&mut self, n: i64) { self.total = add(self.total, n); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_works() { assert_eq!(add(2, 2), 4); }

    #[test]
    fn calc_push() {
        let mut c = Calc { total: 0 };
        c.push(5);
        assert_eq!(c.total, 5);
    }
}
```

`src/fmt.rs`:

```rust
use crate::math::add;

pub fn fmt(a: i64, b: i64) -> String { format!("{}", add(a, b)) }

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn fmt_async_style() { assert_eq!(crate::fmt::fmt(1, 2), "3"); }
}
```

`tests/integration.rs`:

```rust
use rust_app::math::add;

#[test]
fn integration_add() { assert_eq!(add(1, 1), 2); }
```

- [ ] **Step 2: Commit**

```bash
git add fixtures && git commit -m "test: seed rust-app fixture"
```

---

### Task 5: lang-rust definitions extraction

**Files:**
- Create: `crates/lang-rust/Cargo.toml`, `crates/lang-rust/src/lib.rs`, `crates/lang-rust/tests/extract.rs`
- Modify: root `Cargo.toml` members

**Interfaces:**
- Consumes: `Language` trait, `Extraction`, `ExtractedDef`, `DefKind` from `testless-core` (same as lang-ts/lang-go)
- Produces: `pub struct RustLanguage;`, `id() == "rust"`, `extensions() == ["rs"]`

- [ ] **Step 1: Crate scaffold + deps**

Add `"crates/lang-rust"` to workspace members. `crates/lang-rust/Cargo.toml` mirrors lang-go's (name `testless-lang-rust`, dep `testless-core` by path, metadata like Task 3). Run: `cargo add -p testless-lang-rust tree-sitter tree-sitter-rust`

- [ ] **Step 2: Failing test** (`crates/lang-rust/tests/extract.rs`)

```rust
use testless_core::{DefKind, Language};
use testless_lang_rust::RustLanguage;
use std::path::Path;

fn extract(src: &str) -> testless_core::Extraction {
    let lang = RustLanguage;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang.grammar(Path::new("x.rs"))).unwrap();
    let tree = parser.parse(src, None).unwrap();
    lang.extract(src, &tree)
}

#[test]
fn extracts_fns_methods_types_module_init() {
    let src = std::fs::read_to_string("../../fixtures/rust-app/src/math.rs").unwrap();
    let ex = extract(&src);
    let names: Vec<(&str, DefKind)> = ex.defs.iter().map(|d| (d.name.as_str(), d.kind)).collect();
    assert!(names.contains(&("<module>", DefKind::ModuleInit)));
    assert!(names.contains(&("add", DefKind::Function)));
    assert!(names.contains(&("Calc", DefKind::Class)));
    assert!(names.contains(&("Calc.push", DefKind::Method)));
}
```

- [ ] **Step 3: Verify fail, implement**

Node kinds: `function_item` → Function (name field `name`); inside `impl_item` → Method named `Type.method` (impl's `type` field text, strip generics `<...>`); `struct_item`/`enum_item`/`trait_item` → Class; unconditional `<module>` ModuleInit spanning file. Walk `mod_item` bodies recursively (defs inside inline mods extracted flat). Grammar: `tree_sitter_rust::LANGUAGE.into()`. `resolve_import` stub `None` for now (Task 7).

- [ ] **Step 4: Pass + commit**

Run: `cargo test -p testless-lang-rust`
```bash
git add -A && git commit -m "feat(lang-rust): definition extraction"
```

---

### Task 6: lang-rust test extraction

**Files:**
- Modify: `crates/lang-rust/src/lib.rs`; Create: `crates/lang-rust/tests/tests.rs`

- [ ] **Step 1: Failing tests**

```rust
// same extract() helper copied in
use testless_core::DefKind;

#[test]
fn extracts_cfg_test_mod_tests_with_chain() {
    let src = std::fs::read_to_string("../../fixtures/rust-app/src/math.rs").unwrap();
    let ex = extract(&src);
    let ids: Vec<_> = ex.defs.iter().filter(|d| d.kind == DefKind::TestCase)
        .filter_map(|d| d.test_id.clone()).collect();
    assert!(ids.contains(&vec!["tests".into(), "add_works".into()]));
    assert!(ids.contains(&vec!["tests".into(), "calc_push".into()]));
}

#[test]
fn attribute_variants_and_integration_tests() {
    let src = std::fs::read_to_string("../../fixtures/rust-app/src/fmt.rs").unwrap();
    let ex = extract(&src);
    let ids: Vec<_> = ex.defs.iter().filter(|d| d.kind == DefKind::TestCase)
        .filter_map(|d| d.test_id.clone()).collect();
    assert!(ids.contains(&vec!["tests".into(), "fmt_async_style".into()])); // #[tokio::test]

    let src = std::fs::read_to_string("../../fixtures/rust-app/tests/integration.rs").unwrap();
    let ex = extract(&src);
    let ids: Vec<_> = ex.defs.iter().filter(|d| d.kind == DefKind::TestCase)
        .filter_map(|d| d.test_id.clone()).collect();
    assert!(ids.contains(&vec!["integration_add".into()]));
}
```

- [ ] **Step 2: Verify fail, implement**

A `function_item` is a TestCase when any of its attributes (`attribute_item` siblings preceding it) has a path whose LAST segment is `test` or `bench` (`#[test]`, `#[tokio::test]`, `#[rstest]` counts only if last segment is `test` — `rstest`'s last segment is `rstest`, so ALSO accept exact `rstest`). test_id = chain of enclosing `mod_item` names + fn name. Maintain mod-stack during walk.

- [ ] **Step 3: Pass + commit**

```bash
git add -A && git commit -m "feat(lang-rust): attribute-based test extraction with mod chains"
```

---

### Task 7: lang-rust imports + resolution

**Files:**
- Modify: `crates/lang-rust/src/lib.rs`; Create: `crates/lang-rust/tests/imports.rs`

- [ ] **Step 1: Failing tests**

```rust
use testless_core::Language;
use testless_lang_rust::RustLanguage;
use std::path::{Path, PathBuf};

#[test]
fn collects_mod_decls_and_use_paths() {
    // extract() helper copied in
    let src = std::fs::read_to_string("../../fixtures/rust-app/src/lib.rs").unwrap();
    let ex = extract(&src);
    let raws: Vec<&str> = ex.imports.iter().map(|i| i.raw.as_str()).collect();
    assert!(raws.contains(&"mod math"));
    assert!(raws.contains(&"mod fmt"));

    let src = std::fs::read_to_string("../../fixtures/rust-app/src/fmt.rs").unwrap();
    let ex = extract(&src);
    let raws: Vec<&str> = ex.imports.iter().map(|i| i.raw.as_str()).collect();
    assert!(raws.contains(&"use crate::math::add"));
}

#[test]
fn resolves_mod_decl_use_crate_and_external() {
    let root = Path::new("../../fixtures/rust-app");
    let l = RustLanguage;
    assert_eq!(l.resolve_import(Path::new("src/lib.rs"), "mod math", root),
               Some(PathBuf::from("src/math.rs")));
    assert_eq!(l.resolve_import(Path::new("src/fmt.rs"), "use crate::math::add", root),
               Some(PathBuf::from("src/math.rs")));
    assert_eq!(l.resolve_import(Path::new("tests/integration.rs"), "use rust_app::math::add", root),
               None); // external-crate-shaped: tier 1 skips (ModuleInit widening covers)
    assert_eq!(l.resolve_import(Path::new("src/fmt.rs"), "use serde::Serialize", root), None);
}
```

- [ ] **Step 2: Verify fail, implement**

ImportRef raw formats: `mod <name>` for `mod name;` declarations (no body); `use <path>` for `use_declaration` (path text without trailing `::{...}` group — for grouped uses emit one ImportRef per leaf, raw `use crate::a::B` style; over-approx acceptable: one ref for the common prefix is also fine — pick one behavior and test it; the given test uses simple non-grouped paths).

`resolve_import`:
- `mod X` from file F: try `F_dir/X.rs`, `F_dir/X/mod.rs` (where F is `lib.rs`/`main.rs`/`mod.rs` → F_dir = its dir; otherwise F `a/b.rs` → F_dir = `a/b/`)
- `use crate::seg1::seg2::...`: src root = dir of `from_file`'s nearest ancestor containing `lib.rs` or `main.rs` (walk up from from_file's dir toward repo root, first dir containing either wins). Try longest-prefix file mapping: `seg1/seg2.rs`, `seg1/seg2/mod.rs`, `seg1.rs`, `seg1/mod.rs` — first existing wins
- `use super::...`: same mapping from parent dir
- anything else (`use extern_crate::...`, `use std::...`) → None

- [ ] **Step 3: Pass + commit**

```bash
git add -A && git commit -m "feat(lang-rust): mod/use import collection and tier-1 resolution"
```

---

### Task 8: Register Rust in CLI + self-index integration test

**Files:**
- Modify: `crates/cli/Cargo.toml` (dep `testless-lang-rust`), `crates/cli/src/main.rs` (registry), `crates/cli/tests/index.rs` (registry helper)
- Create: `crates/cli/tests/dogfood.rs`

- [ ] **Step 1: Failing test** (`crates/cli/tests/dogfood.rs`)

```rust
use testless_core::{indexer::index_repo, DefKind, Registry};

fn registry() -> Registry {
    Registry::new(vec![
        Box::new(testless_lang_ts::TsLanguage),
        Box::new(testless_lang_go::GoLanguage),
        Box::new(testless_lang_rust::RustLanguage),
    ])
}

#[test]
fn indexes_rust_fixture() {
    let g = index_repo(std::path::Path::new("../../fixtures/rust-app"), &registry()).unwrap();
    assert!(g.defs.iter().any(|d| d.name == "Calc.push"));
    assert!(g.defs.iter().any(|d| d.kind == DefKind::TestCase));
    let fmt = g.files.iter().position(|f| f.path.ends_with("fmt.rs")).unwrap() as u32;
    let math = g.files.iter().position(|f| f.path.ends_with("math.rs")).unwrap() as u32;
    assert!(g.edges.contains(&(fmt, testless_core::EdgeKind::Imports, math)));
}

#[test]
fn dogfood_indexes_own_repo() {
    // repo root is two levels up from crates/cli
    let g = index_repo(std::path::Path::new("../.."), &registry()).unwrap();
    // our own Rust source is extracted
    assert!(g.defs.iter().any(|d| d.name == "<module>"));
    assert!(g.defs.iter().filter(|d| d.kind == DefKind::TestCase).count() > 20,
            "should find our own #[test] fns plus fixture tests");
}
```

- [ ] **Step 2: Implement**

Add `Box::new(testless_lang_rust::RustLanguage)` to the CLI's registry in `main.rs`. Update the existing `registry()` helper in `crates/cli/tests/index.rs` the same way (keep existing assertions passing).

- [ ] **Step 3: Pass + commit**

Run: `cargo test -p testless` then `cargo test --workspace`
```bash
git add -A && git commit -m "feat(cli): register Rust language; dogfood self-index test"
```

---

### Task 9: CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Write workflow**

```yaml
name: CI
on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always

jobs:
  fmt:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: rustfmt }
      - run: cargo fmt --check

  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: clippy }
      - uses: Swatinem/rust-cache@v2
      - run: cargo clippy --workspace --all-targets -- -D warnings

  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --workspace

  dogfood:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Build
        run: cargo build --release -p testless
      - name: Index ourselves
        run: |
          out=$(./target/release/testless index)
          echo "$out"
          echo "$out" | jq -e '.files > 0 and .defs > 0 and .tests > 0'
      - name: Warm cache re-index
        run: ./target/release/testless index | jq -e '.parsed == 0'
      - name: Stats
        run: ./target/release/testless stats | jq -e '.tests > 0'
```

- [ ] **Step 2: Make existing code fmt-clean**

Run: `cargo fmt` — if it changes files, that's expected (first enforcement). Then `cargo fmt --check` → clean, `cargo test --workspace` → still green. Include the formatting changes in this task's commit (or a separate `style: cargo fmt` commit first if the diff is large).

- [ ] **Step 3: Verify**

Run: `nix-shell -p actionlint --run 'actionlint .github/workflows/ci.yml'`
Expected: no output (clean). Also run the dogfood steps locally verbatim (build release, the three jq assertions) — all pass.

- [ ] **Step 3: Commit**

```bash
git add .github && git commit -m "ci: fmt, clippy, test, and self-index dogfood jobs"
```

---

### Task 10: Release pipeline

**Files:**
- Create: `.github/workflows/release-please.yml`, `release-please-config.json`, `.release-please-manifest.json`, `.github/workflows/release.yml`

- [ ] **Step 1: release-please config**

`release-please-config.json`:

```json
{
  "$schema": "https://raw.githubusercontent.com/googleapis/release-please/main/schemas/config.json",
  "release-type": "rust",
  "packages": {
    ".": {
      "release-type": "rust",
      "package-name": "testless",
      "include-component-in-tag": false
    }
  },
  "plugins": [{ "type": "cargo-workspace" }]
}
```

`.release-please-manifest.json`:

```json
{ ".": "0.1.0" }
```

`.github/workflows/release-please.yml`:

```yaml
name: release-please
on:
  push:
    branches: [main]
permissions:
  contents: write
  pull-requests: write
jobs:
  release-please:
    runs-on: ubuntu-latest
    steps:
      - uses: googleapis/release-please-action@v4
        with:
          config-file: release-please-config.json
          manifest-file: .release-please-manifest.json
```

- [ ] **Step 2: release.yml** — binaries + crates.io

Try cargo-dist first: `nix-shell -p cargo-dist --run 'dist init --yes'` (accept generated `.github/workflows/release.yml` + `dist-workspace.toml`; targets: x86_64/aarch64 linux+macos). If cargo-dist unavailable/unworkable, hand-roll:

```yaml
name: release
on:
  push:
    tags: ["v*"]
permissions:
  contents: write
jobs:
  build:
    strategy:
      matrix:
        include:
          - { os: ubuntu-latest, target: x86_64-unknown-linux-gnu }
          - { os: ubuntu-latest, target: aarch64-unknown-linux-gnu, cross: true }
          - { os: macos-latest, target: x86_64-apple-darwin }
          - { os: macos-latest, target: aarch64-apple-darwin }
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { targets: "${{ matrix.target }}" }
      - if: matrix.cross
        run: cargo install cross --locked
      - name: Build
        run: |
          if [ "${{ matrix.cross }}" = "true" ]; then cross build --release -p testless --target ${{ matrix.target }}
          else cargo build --release -p testless --target ${{ matrix.target }}; fi
      - name: Package
        run: tar czf testless-${{ matrix.target }}.tar.gz -C target/${{ matrix.target }}/release testless
      - uses: softprops/action-gh-release@v2
        with: { files: "testless-*.tar.gz" }

  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Publish crates in dependency order
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
        run: |
          if [ -z "$CARGO_REGISTRY_TOKEN" ]; then echo "no token; skipping crates.io publish"; exit 0; fi
          for c in testless-core testless-lang-ts testless-lang-go testless-lang-rust testless; do
            cargo publish -p "$c" --no-verify || true
            sleep 30
          done
```

(`|| true` + sleep: tolerate already-published versions and index propagation. Crude but adequate; tighten later.)

- [ ] **Step 3: Verify**

`nix-shell -p actionlint --run 'actionlint .github/workflows/release-please.yml .github/workflows/release.yml'` → clean. Config JSONs validate: `jq . release-please-config.json .release-please-manifest.json`.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "ci: release-please and binary/crates.io release pipeline"
```

---

## Done criteria (Plan R)

- `cargo test --workspace` green (incl. lang-rust suites + dogfood self-index test)
- `cargo clippy -D warnings` + `cargo fmt --check` clean (CI will enforce — run locally)
- actionlint clean on all three workflows
- Dogfood steps verified locally: release binary indexes this repo, `tests > 0`, warm run `parsed == 0`
- README status table matches reality
