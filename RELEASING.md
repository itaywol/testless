# Releasing testless

## Normal flow (automated)

1. Merge conventional-commit PRs to `main`. release-please opens a release PR.
2. Merge the release PR. Tags `testless-v<version>` are created.
3. The `release.yml` workflow (dispatched on the tag) builds binaries, uploads
   them to the GitHub release, publishes the five crates to crates.io, and
   publishes the six npm packages.

npm publishing is OIDC-first (npm trusted publishing, with automatic
provenance) and falls back to `NPM_TOKEN` when OIDC is not configured.

## Secrets

| Secret | Purpose |
|---|---|
| `CARGO_REGISTRY_TOKEN` | crates.io publish (crates.io has no OIDC yet) |

npm publishing uses trusted publishing (OIDC) with automatic provenance and
needs no secret.
