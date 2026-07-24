#!/usr/bin/env node
"use strict";

/**
 * Shim that resolves and executes the platform-specific `testless` binary.
 *
 * The real binary ships in one of the `testless-<platform>-<arch>` optional
 * dependencies (see package.json). This file has no dependencies of its own
 * so it works even if optional dependency resolution partially failed.
 */

const { spawnSync } = require("node:child_process");

const PLATFORM_PACKAGES = {
  "linux-x64": "testless-linux-x64",
  "linux-arm64": "testless-linux-arm64",
  "darwin-x64": "testless-darwin-x64",
  "darwin-arm64": "testless-darwin-arm64",
};

function unsupported(message) {
  console.error(
    [
      `testless: ${message}`,
      "",
      "No prebuilt binary is available for this platform/architecture combination.",
      "You can still use testless by:",
      "  - installing the Rust toolchain and running: cargo install testless",
      "  - downloading a prebuilt binary from https://github.com/itaywol/testless/releases",
    ].join("\n"),
  );
  process.exit(1);
}

function main() {
  const key = `${process.platform}-${process.arch}`;
  const pkgName = PLATFORM_PACKAGES[key];

  if (!pkgName) {
    unsupported(`unsupported platform "${key}"`);
    return;
  }

  let binPath;
  try {
    binPath = require.resolve(`${pkgName}/bin/testless`);
  } catch {
    unsupported(
      `could not locate the "${pkgName}" package (optional dependency failed to install)`,
    );
    return;
  }

  const result = spawnSync(binPath, process.argv.slice(2), { stdio: "inherit" });

  if (result.error) {
    console.error(`testless: failed to run binary at ${binPath}: ${result.error.message}`);
    process.exit(1);
    return;
  }

  if (result.signal) {
    // Die from the same signal the child did, rather than inventing an exit code.
    process.kill(process.pid, result.signal);
    return;
  }

  process.exit(result.status === null ? 1 : result.status);
}

main();
