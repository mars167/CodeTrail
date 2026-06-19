const fs = require("node:fs");
const path = require("node:path");

const PACKAGE_BY_PLATFORM = new Map([
  ["darwin:arm64", "@mars167/core-darwin-arm64"],
  ["darwin:x64", "@mars167/core-darwin-x64"],
  ["linux:arm64", "@mars167/core-linux-arm64"],
  ["linux:x64", "@mars167/core-linux-x64"],
  ["win32:arm64", "@mars167/core-win32-arm64"],
  ["win32:x64", "@mars167/core-win32-x64"]
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

function existingBinary(binaryPath) {
  return fs.existsSync(binaryPath) ? binaryPath : null;
}

function envCoreBinary() {
  const binaryPath = process.env.CODETRAIL_CORE_BINARY;
  if (!binaryPath) return null;
  if (!fs.existsSync(binaryPath)) {
    throw new Error(`Codetrail core binary missing: ${binaryPath}`);
  }
  return binaryPath;
}

function localDevCoreBinary(platform = process.platform) {
  return existingBinary(
    path.resolve(__dirname, "..", "..", "target", "release", binaryNameForPlatform(platform))
  );
}

function resolveCoreBinary(platform = process.platform, arch = process.arch) {
  const envBinary = envCoreBinary();
  if (envBinary) return envBinary;

  const packageName = packageNameForPlatform(platform, arch);
  try {
    const binaryPath = path.join(packageRoot(packageName), "bin", binaryNameForPlatform(platform));
    const packageBinary = existingBinary(binaryPath);
    if (packageBinary) return packageBinary;
    const localBinary = localDevCoreBinary(platform);
    if (localBinary) return localBinary;
    throw new Error(`Codetrail core binary missing: ${binaryPath}`);
  } catch (error) {
    if (!String(error.message || "").includes("Codetrail core package is missing")) {
      throw error;
    }
    const localBinary = localDevCoreBinary(platform);
    if (localBinary) return localBinary;
    throw error;
  }
}

module.exports = {
  packageNameForPlatform,
  binaryNameForPlatform,
  localDevCoreBinary,
  resolveCoreBinary
};
