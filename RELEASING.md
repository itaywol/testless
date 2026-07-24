# Releasing testless

## Normal flow (automated)

1. Merge conventional-commit PRs to `main`. release-please opens a release PR.
2. Merge the release PR. Tags `testless-v<version>` are created.
3. The `release.yml` workflow (dispatched on the tag) builds binaries, uploads
   them to the GitHub release, publishes the five crates to crates.io, and
   publishes the six npm packages.

npm publishing is OIDC-first (npm trusted publishing, with automatic
provenance) and falls back to `NPM_TOKEN` when OIDC is not configured.

## One-time npm bootstrap

npm requires a package to exist before a trusted publisher can be configured
(npm/cli#8544), so the very first publish of each package needs classic auth:

1. Mint a Granular Access Token at npmjs.com with packages+scopes write access
   and "Bypass two-factor authentication" enabled, or use a logged-in local
   npm session (2FA prompts included).
2. Run `scripts/npm-first-publish.sh <version>` (uses `NPM_TOKEN` env var if
   set, otherwise your local npm auth). It downloads the release binaries for
   that version, stamps package versions, and publishes all five packages.
3. On npmjs.com, for each package (`testless-cli`, `testless-linux-x64`,
   `testless-linux-arm64`, `testless-darwin-x64`, `testless-darwin-arm64`):
   Settings, Trusted Publisher, GitHub Actions, organization `itaywol`,
   repository `testless`, workflow filename `release.yml`.
4. Revoke the bootstrap token. From then on OIDC publishes with provenance and
   no secrets.

## Secrets

| Secret | Purpose | Needed when |
|---|---|---|
| `CARGO_REGISTRY_TOKEN` | crates.io publish | always (crates.io has no OIDC yet) |
| `NPM_TOKEN` | npm fallback | only until trusted publishers are configured |
