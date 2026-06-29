const test = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { installTarget, doctorTarget, listTargets } = require("../lib/agent-install");
const { assertAgentAssetsSynced } = require("../../scripts/npm/check-agent-assets");

test("lists common code agent targets", () => {
  const targets = listTargets().map((target) => target.id);
  assert.deepEqual(targets, ["codex", "claude", "cursor", "continue", "cline", "roo"]);
});

test("dry-run returns a plan without writing files", () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "codetrail-home-"));
  const result = installTarget("codex", { home, scope: "user", dryRun: true });
  assert.equal(result.changed, false);
  assert.equal(result.planned.some((item) => item.destination.endsWith(".codex/skills/codetrail/SKILL.md")), true);
  assert.equal(fs.existsSync(path.join(home, ".codex", "skills", "codetrail", "SKILL.md")), false);
});

test("project scope writes into the project directory", () => {
  const project = fs.mkdtempSync(path.join(os.tmpdir(), "codetrail-project-"));
  const result = installTarget("cursor", { project, scope: "project", force: true });
  const rulePath = path.join(project, ".cursor", "rules", "codetrail.mdc");
  assert.equal(result.changed, true);
  assert.equal(fs.existsSync(rulePath), true);
});

test("legacy agent wrapper uses project-root for project scope", () => {
  const project = fs.mkdtempSync(path.join(os.tmpdir(), "codetrail-project-"));
  const bin = path.resolve(__dirname, "..", "bin", "codetrail.js");
  const ok = spawnSync(
    process.execPath,
    [bin, "skills", "add", "cursor", "--scope", "project", "--project-root", project, "--dry-run"],
    { encoding: "utf8" }
  );
  assert.equal(ok.status, 0, ok.stderr);
  const value = JSON.parse(ok.stdout);
  assert.equal(value.planned[0].destination.startsWith(project), true);

  const oldPath = spawnSync(
    process.execPath,
    [bin, "skills", "add", "cursor", "--scope", "project", "--path", project, "--dry-run"],
    { encoding: "utf8" }
  );
  assert.notEqual(oldPath.status, 0);
  assert.match(oldPath.stderr, /--project-root/);
});

test("doctor reports missing and installed states", () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "codetrail-home-"));
  const before = doctorTarget("codex", { home, scope: "user" });
  assert.equal(before.ok, false);
  installTarget("codex", { home, scope: "user", force: true });
  const after = doctorTarget("codex", { home, scope: "user" });
  assert.equal(after.ok, true);
});

test("codetrail skill stays compact and routes agents to semantic-index commands", () => {
  assertAgentAssetsSynced(path.resolve(__dirname, "..", ".."));
  const skill = fs.readFileSync(
    path.resolve(__dirname, "..", "assets", "codetrail", "SKILL.md"),
    "utf8"
  );
  assert.equal(skill.includes("index doctor"), true);
  assert.equal(skill.includes("explore flow"), false);
  assert.equal(skill.includes("explore node"), false);
  assert.equal(skill.includes("--compact"), false);
  assert.equal(skill.includes("precise_fact"), true);
  assert.equal(skill.includes("parser_fact"), true);
  assert.equal(skill.includes("inferred_candidate"), true);
  assert.equal(skill.includes("source_fact"), false);
  assert.equal(skill.includes("| Language | Provider command |"), false);
  assert.equal(skill.includes("\"evidence\""), false);
  assert.equal(skill.includes("RuoYi"), false);
  assert.equal(skill.includes("codetrail read"), false);
  assert.equal(skill.includes("rg"), true);
  assert.equal(skill.split(/\r?\n/).length <= 90, true);
});

test("codetrail skill no longer ships subagent templates", () => {
  const agentsDir = path.resolve(__dirname, "..", "assets", "codetrail", "agents");
  assert.equal(fs.existsSync(agentsDir), false);
  const skill = fs.readFileSync(
    path.resolve(__dirname, "..", "assets", "codetrail", "SKILL.md"),
    "utf8"
  );
  assert.equal(skill.toLowerCase().includes("subagent"), false);
});
