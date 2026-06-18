const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { installTarget, doctorTarget, listTargets } = require("../lib/agent-install");

test("lists common code agent targets", () => {
  const targets = listTargets().map((target) => target.id);
  assert.deepEqual(targets, ["codex", "opencode", "claude", "cursor", "continue", "cline", "roo", "openai"]);
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
  const result = installTarget("opencode", { project, scope: "project", force: true });
  const agentPath = path.join(project, ".opencode", "agents", "codetrail-evidence.md");
  assert.equal(result.changed, true);
  assert.equal(fs.existsSync(agentPath), true);
});

test("doctor reports missing and installed states", () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "codetrail-home-"));
  const before = doctorTarget("codex", { home, scope: "user" });
  assert.equal(before.ok, false);
  installTarget("codex", { home, scope: "user", force: true });
  const after = doctorTarget("codex", { home, scope: "user" });
  assert.equal(after.ok, true);
});
