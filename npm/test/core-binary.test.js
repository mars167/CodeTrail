const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const {
  packageNameForPlatform,
  binaryNameForPlatform,
  resolveCoreBinary
} = require("../lib/core-binary");

test("maps supported platforms to optional platform packages", () => {
  assert.equal(packageNameForPlatform("darwin", "arm64"), "@mars167/core-darwin-arm64");
  assert.equal(packageNameForPlatform("darwin", "x64"), "@mars167/core-darwin-x64");
  assert.equal(packageNameForPlatform("linux", "arm64"), "@mars167/core-linux-arm64");
  assert.equal(packageNameForPlatform("linux", "x64"), "@mars167/core-linux-x64");
  assert.equal(packageNameForPlatform("win32", "arm64"), "@mars167/core-win32-arm64");
  assert.equal(packageNameForPlatform("win32", "x64"), "@mars167/core-win32-x64");
});

test("uses executable extension only on Windows", () => {
  assert.equal(binaryNameForPlatform("linux"), "codetrail");
  assert.equal(binaryNameForPlatform("darwin"), "codetrail");
  assert.equal(binaryNameForPlatform("win32"), "codetrail.exe");
});

test("rejects unsupported platform and architecture", () => {
  assert.throws(() => packageNameForPlatform("freebsd", "x64"), /Unsupported platform/);
  assert.throws(() => packageNameForPlatform("linux", "ppc64"), /Unsupported platform/);
});

test("uses CODETRAIL_CORE_BINARY override before optional platform packages", () => {
  const previous = process.env.CODETRAIL_CORE_BINARY;
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "codetrail-core-"));
  const binary = path.join(dir, "codetrail");
  fs.writeFileSync(binary, "");
  process.env.CODETRAIL_CORE_BINARY = binary;
  try {
    assert.equal(resolveCoreBinary("darwin", "arm64"), binary);
  } finally {
    if (previous === undefined) delete process.env.CODETRAIL_CORE_BINARY;
    else process.env.CODETRAIL_CORE_BINARY = previous;
  }
});
