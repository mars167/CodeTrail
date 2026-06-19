const fs = require("node:fs");
const path = require("node:path");

const PACKAGE_BY_PLATFORM = new Map([
  ["darwin:arm64", "@codetrail/core-darwin-arm64"],
  ["darwin:x64", "@codetrail/core-darwin-x64"],
  ["linux:arm64", "@codetrail/core-linux-arm64"],
  ["linux:x64", "@codetrail/core-linux-x64"],
  ["win32:arm64", "@codetrail/core-win32-arm64"],
  ["win32:x64", "@codetrail/core-win32-x64"]
]);

function packageNameForPlatform(platform = process.platform, arch = process.arch) {
  const key = `${platform}:${arch}`;
  const packageName = PACKAGE_BY_PLATFORM.get(key);
  if (!packageName) {
    throw new Error(`Unsupported platform for codetrail npm package: ${key}`);
  }
  return packageName;
}

function binaryNameForPlatform(platform = process.platform) {
  return platform === "win32" ? "codetrail.exe" : "codetrail";
}

function packageRoot(packageName) {
  try {
    return path.dirname(require.resolve(`${packageName}/package.json`));
  } catch (error) {
    if (error && error.code === "MODULE_NOT_FOUND") {
      throw new Error(`Codetrail core package is missing: ${packageName}`);
    }
    throw error;
  }
}

function resolveCoreBinary(platform = process.platform, arch = process.arch) {
  const packageName = packageNameForPlatform(platform, arch);
  const binaryPath = path.join(packageRoot(packageName), "bin", binaryNameForPlatform(platform));
  if (!fs.existsSync(binaryPath)) {
    throw new Error(`Codetrail core binary missing: ${binaryPath}`);
  }
  return binaryPath;
}

module.exports = {
  packageNameForPlatform,
  binaryNameForPlatform,
  resolveCoreBinary
};
