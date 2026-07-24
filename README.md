<p align="center">
  <br>
  <a href="https://testless.itaywol.tools" target="_blank" rel="noopener noreferrer">
    <img alt="testless" src="assets/logo.svg" height="56">
  </a>
  <br>
</p>

<p align="center">Cut your test time to the minimum. Run 5 tests instead of 500 by reading your code, not your coverage.</p>

<div align="center">

[![CI](https://github.com/itaywol/testless/actions/workflows/ci.yml/badge.svg)](https://github.com/itaywol/testless/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/testless.svg)](https://crates.io/crates/testless)
[![npm](https://img.shields.io/npm/v/testless)](https://www.npmjs.com/package/testless)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

</div>

<p align="center"><a href="https://testless.itaywol.tools"><b>testless.itaywol.tools</b></a></p>

![demo](assets/demo.gif)

testless parses your repo into a function-level graph with tree-sitter, diffs
changes structurally (comments and formatting are free), and walks the graph to
find every test a change could break. It prints your runner's own CLI
invocations, ready to execute.

**Selection is always a superset of the truly impacted tests.** When static
analysis cannot resolve something, testless widens instead of guessing.

## Install

| Ecosystem | Command |
|---|---|
| JS / TS | `npm i -D testless` |
| Go | `curl -fsSL https://testless.itaywol.tools/install.sh \| sh` |
| Rust | `cargo binstall testless` or `cargo install testless` |

Prebuilt binaries on [Releases](https://github.com/itaywol/testless/releases). Nix flake repo: `nix develop`.

## Use

```bash
testless index                                    # build the graph (.testless/)
testless select --from origin/main                # tests impacted by your changes
testless select --from origin/main --format args  # runnable vitest / go test / cargo test lines
```

JSON when piped, human text on a TTY. On fallback-to-everything, `--format args`
prints nothing and exits 2, so branch on the exit code:

```bash
testless select --from origin/main --format args > cmds.txt || RUN_ALL=1
```

## Languages

TypeScript / JavaScript (vitest, jest `-t` patterns) · Go (`go test -run`) · Rust (`cargo test`)

A new language is roughly one plugin file: see [CONTRIBUTING](CONTRIBUTING.md).

## Status

Young project. The selection engine works end to end on all three languages;
precision improves release by release. Roadmap lives in
[issues](https://github.com/itaywol/testless/issues).

[MIT](LICENSE)
