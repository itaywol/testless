# Security Policy

## Reporting a vulnerability

Report vulnerabilities privately via [GitHub Security Advisories](https://github.com/itaywol/testless/security/advisories/new)
for this repository. Do not open a public issue for security reports.

Include what you can:

- affected version / commit
- repro steps or a minimal fixture
- impact (what an attacker gains)

## What to expect

This is a best-effort, single-maintainer project. There is no bug bounty.
You'll get an acknowledgment and a fix or mitigation timeline as soon as
reasonably possible — there's no fixed SLA.

## Scope

`testless` performs static analysis (tree-sitter parsing) over source trees
you point it at and writes a local cache (`.testless/`). Relevant reports:
parser crashes/panics on adversarial input, path traversal via cache or
config handling, or anything that executes code from analyzed repos rather
than just reading it.
