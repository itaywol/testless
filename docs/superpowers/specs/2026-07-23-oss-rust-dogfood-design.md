# testless — OSS Readiness + Rust Language + Dogfood (Spec Addendum)

Date: 2026-07-23
Status: approved
Parent spec: `2026-07-23-testless-design.md` (all engine rules there still govern)

## Goal

Make the repo publicly presentable (modeled on `itaywol/adeptability`'s setup),
add Rust as the third language plugin, and dogfood `testless index` on this
repository in CI. Full select-dogfood upgrades automatically once Plans 2-4 land.

## 1. OSS scaffolding

| File | Content |
|---|---|
| `LICENSE` | MIT, copyright itay wolmarans |
| `README.md` | What/why, honest status table (index ✅ / select 🚧 Plans 2-4), quickstart (`cargo install testless` + `testless index`), architecture sketch (core + language plugins), language support table, badges: CI, license, crates.io |
| `CONTRIBUTING.md` | Build/test/clippy/fmt commands, ground rules (TDD, over-approximation principle, conventional commits — release-please depends on them), "adding a language" section describing the 5-item `Language` contract |
| `SECURITY.md` | Report via GitHub private security advisories, no SLA promises |
| `.editorconfig` | rust 4-space; toml/yaml/md 2-space; lf; final newline |
| `flake.nix` | Dev shell only: rustc, cargo, clippy, rustfmt, rust-analyzer. No package output yet |
| `rust-toolchain.toml` | `channel = "stable"` |

Not now (YAGNI): mkdocs site, Dockerfile, cargo-deny/audit, issue templates.

## 2. CI + release pipeline

`.github/workflows/ci.yml` (push to main + PRs):
- `fmt`: `cargo fmt --check`
- `lint`: `cargo clippy --workspace --all-targets -- -D warnings`
- `test`: `cargo test --workspace`
- `dogfood`: see §4

`.github/workflows/release-please.yml`: release-please, rust releaser,
cargo-workspace plugin — maintains versions + CHANGELOG from conventional
commits, opens release PR; merging it tags the release.

`.github/workflows/release.yml` (on release tag):
- cargo-dist: build + attach binaries (x86_64/aarch64, linux + macos)
- `cargo publish` in dependency order: testless-core → testless-lang-ts →
  testless-lang-go → testless-lang-rust → testless (needs `CARGO_REGISTRY_TOKEN`
  secret — user adds manually; workflow skips gracefully if absent)

Crate metadata required for publish: description, license, repository on all
five crates.

## 3. lang-rust plugin

Same 5-item `Language` contract, `id() == "rust"`, extensions `["rs"]`,
tree-sitter-rust grammar.

Defs:
- `function_item` → Function (or Method when inside `impl` block: name `Type.method`)
- `struct_item` / `enum_item` / `trait_item` → Class kind
- `mod name { ... }` inline modules: defs inside carry the mod chain in their
  test-id (tests) but remain flat defs (parent = enclosing item where natural)
- One `<module>` ModuleInit per file, whole-file span

Tests:
- fn with any attribute whose last path segment is `test` (`#[test]`,
  `#[tokio::test]`, `#[rstest]`, `#[async_std::test]`) → TestCase
- `#[bench]` → TestCase (bench kind not distinguished yet)
- test_id chain = inline-mod nesting + fn name, e.g. `["tests", "add_works"]`
  in a `#[cfg(test)] mod tests`; integration test file top-level fn → `["fn_name"]`
- `computed_name` unused for Rust tier 1 (no dynamic test names statically visible)

Imports (tier 1, over-approximate, documented):
- `mod foo;` declaration → import edge to `foo.rs` or `foo/mod.rs` (relative to
  declaring file's dir; `main.rs`/`lib.rs`/`mod.rs` are dir roots)
- `use crate::a::b::c` → best-effort map `a/b.rs`, `a/b/mod.rs`, `a.rs` (longest
  prefix that exists, from src root = dir of nearest `lib.rs`/`main.rs`) — first
  hit wins; miss → skip (ModuleInit widening covers)
- `use super::x` → parent dir mapping, same best-effort
- External crates (`use serde::...`) → None
- `include!`/`path` attributes → out of scope, documented limitation

Fixture `fixtures/rust-app`: small lib crate — `src/lib.rs`, `src/math.rs`
(mod with fns + `#[cfg(test)] mod tests`), `src/fmt.rs` (uses crate::math),
`tests/integration.rs` (integration test importing the lib), one
`#[tokio::test]`-shaped attribute case (attribute only; no tokio dep — fixture
is data, never compiled).

## 4. Dogfood (CI job)

- Build release binary, run `testless index` at repo root
- Assert: exit 0; JSON `files > 0`, `defs > 0`, `tests > 0` (proves lang-rust
  extracts our own `#[test]` fns)
- Run `testless index` again: assert `parsed == 0` (cache path works in CI)
- Run `testless stats`: exit 0
- Uses jq for assertions

When Plan 4 lands, this job gains: `testless select --from origin/main` and
pipes the selection into `cargo test` — free upgrade, not built now.

## Out of scope

Select-based dogfooding, docs site, Docker, issue templates, cargo-deny,
Windows builds (add on demand), crates.io squat-publish before the release
pipeline does it properly.
