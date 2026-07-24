# testless

> test less. — function-level test impact analysis via static AST.

[![CI](https://github.com/itaywol/testless/actions/workflows/ci.yml/badge.svg)](https://github.com/itaywol/testless/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![crates.io](https://img.shields.io/crates/v/testless.svg)](https://crates.io/crates/testless)

`testless` analyzes your code changes at the AST level and tells your test
runner which tests are actually impacted — run 5 tests instead of 500.
No runtime instrumentation, no coverage collection, no paid platform.

**Guarantee:** over-approximation. The selected set is always a superset of the
truly impacted set. When static analysis can't resolve something (dynamic
dispatch, mocks, reflection), testless widens the selection — it never silently
skips a test that could fail.

## Status

| Capability | State |
|---|---|
| Index: function-level graph of defs, tests, imports, calls/reads | ✅ |
| Incremental re-index (content-hash gated cache) | ✅ |
| Languages: TypeScript/JavaScript, Go, Rust | ✅ |
| Change detection (structural AST diff) | ✅ |
| Impact walk + `select` (the point of the tool) | ✅ |
| SCIP-sharpened call resolution | planned |

## Quickstart

    cargo install testless        # or: nix develop; cargo run -p testless
    cd your-repo
    testless index                # builds .testless/graph.bin
    testless stats                # what it knows
    testless select --from origin/main            # what to run right now
    testless select --from origin/main --format args
    testless completion zsh > ~/.zsh/completions/_testless

`index`/`select` output is JSON when piped, human-readable on a TTY. Machine
output on stdout, progress on stderr.

`select --format args` prints one ready-to-run test-runner invocation per
selected test (`vitest run ...`, `go test ...`, `cargo test ...`). It's meant
to be read, copied, or fed to `xargs -I{} sh -c '{}'` — piping it straight to
`sh` runs whatever the selection computed with no chance to look first, so
treat that shortcut with the same caution as any other `| sh`.

`--format args` prints nothing on stdout when testless falls back to
run-all (exit code 2) — consumers must branch on the exit code, not on
empty output, e.g.:

    testless select --from origin/main --format args > cmds.txt || RUN_ALL=1

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
