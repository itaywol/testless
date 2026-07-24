#!/usr/bin/env bash
# One-time npm bootstrap: publish all five packages for an existing GitHub
# release so trusted publishers can be configured afterwards. See RELEASING.md.
# Usage: NPM_TOKEN=... scripts/npm-first-publish.sh 0.5.0
set -euo pipefail
version="${1:?usage: npm-first-publish.sh <version>}"
tag="testless-v${version}"
cd "$(git rev-parse --show-toplevel)"

npmrc=""
if [ -n "${NPM_TOKEN:-}" ]; then
  npmrc=$(mktemp)
  printf '//registry.npmjs.org/:_authToken=%s\n' "$NPM_TOKEN" > "$npmrc"
  export NPM_CONFIG_USERCONFIG="$npmrc"
  trap 'rm -f "$npmrc"' EXIT
fi

workdir=$(mktemp -d)
gh release download "$tag" --dir "$workdir" --pattern 'testless-*.tar.gz'

declare -A targets=(
  [x86_64-unknown-linux-gnu]=testless-linux-x64
  [aarch64-unknown-linux-gnu]=testless-linux-arm64
  [x86_64-apple-darwin]=testless-darwin-x64
  [aarch64-apple-darwin]=testless-darwin-arm64
)

for target in "${!targets[@]}"; do
  pkg="${targets[$target]}"
  tar xzf "$workdir/testless-${target}.tar.gz" -C "npm/${pkg}/bin" testless
  chmod +x "npm/${pkg}/bin/testless"
  npm pkg set version="$version" --prefix "./npm/${pkg}"
done

npm pkg set version="$version" --prefix ./npm/testless
node -e '
  const fs = require("node:fs");
  const p = "npm/testless/package.json";
  const pkg = JSON.parse(fs.readFileSync(p, "utf8"));
  for (const dep of Object.keys(pkg.optionalDependencies || {})) {
    pkg.optionalDependencies[dep] = process.argv[1];
  }
  fs.writeFileSync(p, JSON.stringify(pkg, null, 2) + "\n");
' "$version"

for pkg in testless-linux-x64 testless-linux-arm64 testless-darwin-x64 testless-darwin-arm64 testless; do
  npm publish --access public "./npm/${pkg}"
done

git checkout -q npm/
rm -f npm/*/bin/testless
echo "published ${version}; now configure trusted publishers on npmjs.com (see RELEASING.md)"
