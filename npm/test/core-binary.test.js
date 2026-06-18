const test = require("node:test");
const assert = require("node:assert/strict");
const { packageNameForPlatform, binaryNameForPlatform } = require("../lib/core-binary");

test("maps supported platforms to optional platform packages", () => {
  assert.equal(packageNameForPlatform("darwin", "arm64"), "@codetrail/core-darwin-arm64");
  assert.equal(packageNameForPlatform("darwin", "x64"), "@codetrail/core-darwin-x64");
  assert.equal(packageNameForPlatform("linux", "arm64"), "@codetrail/core-linux-arm64");
  assert.equal(packageNameForPlatform("linux", "x64"), "@codetrail/core-linux-x64");
  assert.equal(packageNameForPlatform("win32", "arm64"), "@codetrail/core-win32-arm64");
  assert.equal(packageNameForPlatform("win32", "x64"), "@codetrail/core-win32-x64");
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
