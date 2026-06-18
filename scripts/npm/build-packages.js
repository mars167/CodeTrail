#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const ROOT = path.resolve(__dirname, "..", "..");
const DIST = process.argv[2] ? path.resolve(process.argv[2]) : path.join(ROOT, "dist");
const MAP = [
  ["codetrail-darwin-arm64.tar.gz", "core-darwin-arm64", "codetrail"],
  ["codetrail-darwin-amd64.tar.gz", "core-darwin-x64", "codetrail"],
  ["codetrail-linux-arm64.tar.gz", "core-linux-arm64", "codetrail"],
  ["codetrail-linux-amd64.tar.gz", "core-linux-x64", "codetrail"],
  ["codetrail-windows-arm64.exe.zip", "core-win32-arm64", "codetrail.exe"],
  ["codetrail-windows-amd64.exe.zip", "core-win32-x64", "codetrail.exe"]
];

function ensureCleanBin(packageDir) {
  const bin = path.join(packageDir, "bin");
  fs.rmSync(bin, { recursive: true, force: true });
  fs.mkdirSync(bin, { recursive: true });
  return bin;
}

function findUnpackedBinary(dir, binaryName) {
  const entries = fs.readdirSync(dir, { withFileTypes: true });
  for (const entry of entries) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      const nested = findUnpackedBinary(fullPath, binaryName);
      if (nested) return nested;
    } else if (entry.name === binaryName) {
      return fullPath;
    }
  }
  return null;
}

function unpack(asset, packageName, binaryName) {
  const assetPath = path.join(DIST, asset);
  if (!fs.existsSync(assetPath)) {
    throw new Error(`missing release asset: ${assetPath}`);
  }
  const packageDir = path.join(ROOT, "npm", "platform", packageName);
  const bin = ensureCleanBin(packageDir);
  const temp = fs.mkdtempSync(path.join(DIST, `${packageName}-`));
  try {
    if (asset.endsWith(".zip")) {
      execFileSync("unzip", ["-q", assetPath, "-d", temp], { stdio: "inherit" });
    } else {
      execFileSync("tar", ["-xzf", assetPath, "-C", temp], { stdio: "inherit" });
    }
    const source = findUnpackedBinary(temp, binaryName);
    if (!source) {
      throw new Error(`binary ${binaryName} not found in ${asset}`);
    }
    const destination = path.join(bin, binaryName);
    fs.copyFileSync(source, destination);
    fs.chmodSync(destination, 0o755);
  } finally {
    fs.rmSync(temp, { recursive: true, force: true });
  }
}

function main() {
  for (const row of MAP) {
    unpack(...row);
  }
  console.log("prepared npm platform packages");
}

if (require.main === module) {
  main();
}
