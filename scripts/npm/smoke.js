#!/usr/bin/env node
const { assertVersionsMatch } = require("./check-versions");
const { packageNameForPlatform, binaryNameForPlatform } = require("../../npm/lib/core-binary");

function main() {
  const version = assertVersionsMatch();
  const packageName = packageNameForPlatform(process.platform, process.arch);
  const binaryName = binaryNameForPlatform(process.platform);
  console.log(JSON.stringify({ version, packageName, binaryName }));
}

main();
