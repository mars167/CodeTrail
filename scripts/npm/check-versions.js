#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");

function cargoVersionFromToml(content) {
  const match = content.match(/^version\s*=\s*"([^"]+)"/m);
  if (!match) {
    throw new Error("Cargo.toml package version not found");
  }
  return match[1];
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function platformPackageFiles(root) {
  const platformRoot = path.join(root, "npm", "platform");
  return fs.readdirSync(platformRoot)
    .map((name) => path.join(platformRoot, name, "package.json"))
    .filter((file) => fs.existsSync(file));
}

function assertVersionsMatch(root = path.resolve(__dirname, "..", "..")) {
  const cargoVersion = cargoVersionFromToml(fs.readFileSync(path.join(root, "Cargo.toml"), "utf8"));
  const mainPackageFile = path.join(root, "npm", "package.json");
  const packageFiles = [mainPackageFile, ...platformPackageFiles(root)];

  for (const file of packageFiles) {
    const version = readJson(file).version;
    if (version !== cargoVersion) {
      throw new Error(`version mismatch: ${file} has ${version}, Cargo.toml has ${cargoVersion}`);
    }
  }

  const mainPackage = readJson(mainPackageFile);
  for (const [name, version] of Object.entries(mainPackage.optionalDependencies || {})) {
    if (version !== cargoVersion) {
      throw new Error(`version mismatch: optional dependency ${name} has ${version}, Cargo.toml has ${cargoVersion}`);
    }
  }

  return cargoVersion;
}

if (require.main === module) {
  const version = assertVersionsMatch();
  console.log(`codetrail package versions match: ${version}`);
}

module.exports = {
  cargoVersionFromToml,
  assertVersionsMatch
};
