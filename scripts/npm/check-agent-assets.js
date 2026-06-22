#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");

const ASSETS = [
  ["skills/codetrail/SKILL.md", "npm/assets/codetrail/SKILL.md"]
];

function assetMappings(root = path.resolve(__dirname, "..", "..")) {
  return ASSETS.map(([source, destination]) => ({
    source: path.join(root, source),
    destination: path.join(root, destination)
  }));
}

function readUtf8(file) {
  return fs.readFileSync(file, "utf8");
}

function assertAgentAssetsSynced(root = path.resolve(__dirname, "..", "..")) {
  const mismatches = [];
  for (const mapping of assetMappings(root)) {
    if (!fs.existsSync(mapping.source)) {
      mismatches.push(`missing source: ${mapping.source}`);
      continue;
    }
    if (!fs.existsSync(mapping.destination)) {
      mismatches.push(`missing asset: ${mapping.destination}`);
      continue;
    }
    if (readUtf8(mapping.source) !== readUtf8(mapping.destination)) {
      mismatches.push(`${mapping.destination} is out of sync with ${mapping.source}`);
    }
  }

  if (mismatches.length > 0) {
    throw new Error(
      [
        "npm agent assets are out of sync with source skills.",
        "Run: node scripts/npm/check-agent-assets.js --write",
        ...mismatches
      ].join("\n")
    );
  }
}

function syncAgentAssets(root = path.resolve(__dirname, "..", "..")) {
  for (const mapping of assetMappings(root)) {
    fs.mkdirSync(path.dirname(mapping.destination), { recursive: true });
    fs.copyFileSync(mapping.source, mapping.destination);
  }
}

if (require.main === module) {
  if (process.argv.includes("--write")) {
    syncAgentAssets();
    console.log("synced npm agent assets");
  } else {
    assertAgentAssetsSynced();
    console.log("npm agent assets are synced");
  }
}

module.exports = {
  assetMappings,
  assertAgentAssetsSynced,
  syncAgentAssets
};
