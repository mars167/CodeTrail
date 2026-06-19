const test = require("node:test");
const assert = require("node:assert/strict");
const { EventEmitter } = require("node:events");
const os = require("node:os");
const path = require("node:path");
const {
  shouldSkipUpdateCheck,
  compareVersions,
  updateCachePath,
  buildInstallArgs,
  canUseGlobalNpm,
  defaultFetchJson,
  installVersion,
  packageTagForVersion
} = require("../lib/update");

test("skips update check for CI, non-tty, json output, or opt-out", () => {
  assert.equal(shouldSkipUpdateCheck({ ci: "true", stderrIsTTY: true, args: [] }), true);
  assert.equal(shouldSkipUpdateCheck({ ci: "false", stderrIsTTY: true, args: [] }), false);
  assert.equal(shouldSkipUpdateCheck({ ci: "", stderrIsTTY: false, args: [] }), true);
  assert.equal(shouldSkipUpdateCheck({ ci: "", stderrIsTTY: true, args: ["--output", "json"] }), true);
  assert.equal(shouldSkipUpdateCheck({ ci: "", stderrIsTTY: true, args: ["--output=jsonl"] }), true);
  assert.equal(shouldSkipUpdateCheck({ ci: "", stderrIsTTY: true, args: [], noUpdateCheck: "1" }), true);
  assert.equal(shouldSkipUpdateCheck({ ci: "", stderrIsTTY: true, args: ["status"] }), false);
});

test("compares semver-like versions without treating beta as newer than stable", () => {
  assert.equal(compareVersions("0.1.7", "0.1.8"), -1);
  assert.equal(compareVersions("0.1.8", "0.1.8"), 0);
  assert.equal(compareVersions("0.1.9", "0.1.8"), 1);
  assert.equal(compareVersions("0.1.8-beta.1", "0.1.8"), -1);
});

test("uses user home cache path", () => {
  const home = path.join(os.tmpdir(), "codetrail-home");
  assert.equal(updateCachePath(home), path.join(home, ".codetrail", "update-check.json"));
});

test("builds npm install command args", () => {
  assert.deepEqual(buildInstallArgs("0.2.0"), ["install", "-g", "codetrail@0.2.0"]);
});

test("uses next dist tag for prerelease and latest for stable", () => {
  assert.equal(packageTagForVersion("0.2.0"), "latest");
  assert.equal(packageTagForVersion("0.2.0-beta.1"), "next");
});

test("registry fetch has a timeout and aborts the request", async () => {
  let timeoutMs = 0;
  let destroyed = false;
  const request = new EventEmitter();
  request.destroy = (error) => {
    destroyed = true;
    request.emit("error", error);
  };
  const transport = {
    get(_url, _options, _callback) {
      return request;
    }
  };
  const originalSetTimeout = global.setTimeout;
  const originalClearTimeout = global.clearTimeout;
  global.setTimeout = (callback, ms) => {
    timeoutMs = ms;
    callback();
    return { unref() {} };
  };
  global.clearTimeout = () => {};
  try {
    await assert.rejects(
      defaultFetchJson("https://registry.example.invalid/latest", { transport, timeoutMs: 25 }),
      /timed out after 25ms/
    );
  } finally {
    global.setTimeout = originalSetTimeout;
    global.clearTimeout = originalClearTimeout;
  }
  assert.equal(timeoutMs, 25);
  assert.equal(destroyed, true);
});

test("checks npm global environment before installing", () => {
  const calls = [];
  const spawn = (cmd, args) => {
    calls.push([cmd, args]);
    if (args[0] === "--version") return { status: 0, stdout: "10.0.0\n" };
    if (args[0] === "prefix") return { status: 0, stdout: "/usr/local\n" };
    return { status: 0 };
  };
  assert.equal(canUseGlobalNpm(spawn), true);
  assert.equal(installVersion("0.2.0", spawn), 0);
  assert.deepEqual(calls.at(-1), ["npm", ["install", "-g", "codetrail@0.2.0"]]);
});

test("prints manual install command when npm global environment is not confirmed", () => {
  const calls = [];
  const errors = [];
  const spawn = (cmd, args) => {
    calls.push([cmd, args]);
    if (args[0] === "--version") return { status: 0, stdout: "10.0.0\n" };
    if (args[0] === "prefix") return { status: 1, stdout: "" };
    return { status: 0 };
  };
  const originalError = console.error;
  console.error = (message) => errors.push(message);
  try {
    assert.equal(installVersion("0.2.0", spawn), 1);
  } finally {
    console.error = originalError;
  }
  assert.deepEqual(calls, [
    ["npm", ["--version"]],
    ["npm", ["prefix", "-g"]]
  ]);
  assert.match(errors[0], /npm install -g codetrail@0\.2\.0/);
});
