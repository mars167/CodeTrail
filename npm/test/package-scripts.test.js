const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { cargoVersionFromToml, assertVersionsMatch } = require("../../scripts/npm/check-versions");
const {
  assetMappings,
  assertAgentAssetsSynced
} = require("../../scripts/npm/check-agent-assets");

test("reads Cargo.toml package version", () => {
  assert.equal(cargoVersionFromToml('[package]\nname = "codetrail"\nversion = "1.2.3"\n'), "1.2.3");
});

test("detects mismatched npm platform package versions", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codetrail-version-"));
  fs.mkdirSync(path.join(root, "npm", "platform", "core-linux-x64"), { recursive: true });
  fs.writeFileSync(path.join(root, "Cargo.toml"), '[package]\nversion = "1.2.3"\n');
  fs.writeFileSync(path.join(root, "npm", "package.json"), '{"version":"1.2.3"}\n');
  fs.writeFileSync(path.join(root, "npm", "platform", "core-linux-x64", "package.json"), '{"version":"1.2.4"}\n');
  assert.throws(() => assertVersionsMatch(root), /version mismatch/);
});

test("npm agent assets stay synced with source skills", () => {
  const root = path.resolve(__dirname, "..", "..");
  assert.equal(assetMappings(root).length, 1);
  assert.doesNotThrow(() => assertAgentAssetsSynced(root));
});
