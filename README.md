# testless

> Run 5 tests instead of 500. — test-impact analysis that reads your code, not your coverage.

[![CI](https://github.com/itaywol/testless/actions/workflows/ci.yml/badge.svg)](https://github.com/itaywol/testless/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/testless.svg)](https://crates.io/crates/testless)
[![npm](https://img.shields.io/npm/v/testless)](https://www.npmjs.com/package/testless)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

![demo](assets/demo.gif)

## Install

```bash
npm i -D testless        # or: npx testless
cargo install testless
```

Prebuilt binaries: [Releases](https://github.com/itaywol/testless/releases).
This is a Nix flake repo — `nix develop` gets you a dev shell with everything
needed to `cargo build --release -p testless`.

## Use

```bash
testless index                                    # builds .testless/graph.bin
testless select --from origin/main                # tests impacted by your changes
testless select --from origin/main --format args  # ready-to-run vitest/go test/cargo test lines
```

`index`/`select` print JSON when piped, human-readable text on a TTY.
`--format args` prints nothing on stdout when testless falls back to running
everything (exit code 2) — branch on the exit code, not on empty output:

```bash
testless select --from origin/main --format args > cmds.txt || RUN_ALL=1
```

## How it works

tree-sitter parses your code into a graph of defs, tests, and imports; a
structural AST diff (comments and formatting cost nothing) finds what actually
changed; a reverse impact walk finds every test that can reach it; results
print as your test runner's own CLI invocations.

**The selected set is always a superset of the truly impacted set** — when
static analysis can't resolve something (dynamic dispatch, mocks, reflection),
testless widens the selection rather than risk skipping a test that could fail.

## Languages

| Language | Test runners | |
|---|---|---|
| TypeScript / JavaScript | vitest, jest | ✅ |
| Go | go test | ✅ |
| Rust | cargo test | ✅ |

A language is ~1 plugin file. See [CONTRIBUTING](CONTRIBUTING.md).

## Status

Young project: the selection engine works end-to-end on TS/Go/Rust today, and
precision keeps improving release by release. Design details and roadmap:
[spec](docs/superpowers/specs/2026-07-23-testless-design.md).

[MIT](LICENSE)
