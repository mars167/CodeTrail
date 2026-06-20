const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { installTarget, doctorTarget, listTargets } = require("../lib/agent-install");
const { assertAgentAssetsSynced } = require("../../scripts/npm/check-agent-assets");

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

test("codetrail skill stays compact and routes agents to summary flow", () => {
  assertAgentAssetsSynced(path.resolve(__dirname, "..", ".."));
  const skill = fs.readFileSync(
    path.resolve(__dirname, "..", "assets", "codetrail", "SKILL.md"),
    "utf8"
  );
  assert.equal(skill.includes("index status --summary"), true);
  assert.equal(skill.includes("explore flow"), true);
  assert.equal(skill.includes("explore node"), true);
  assert.equal(skill.includes("--compact"), true);
  assert.equal(skill.includes("precise_fact"), true);
  assert.equal(skill.includes("parser_fact"), true);
  assert.equal(skill.includes("inferred_candidate"), true);
  assert.equal(skill.includes("source_fact"), true);
  assert.equal(skill.includes("| Language | Provider command |"), false);
  assert.equal(skill.includes("\"evidence\""), false);
  assert.equal(skill.includes("RuoYi"), false);
  assert.equal(skill.includes("codetrail read"), true);
  assert.equal(skill.split(/\r?\n/).length <= 170, true);
});

test("codetrail subagent templates avoid duplicate skill loading", () => {
  const codex = fs.readFileSync(
    path.resolve(
      __dirname,
      "..",
      "assets",
      "codetrail",
      "agents",
      "codex",
      "codetrail-evidence.toml"
    ),
    "utf8"
  );
  const opencode = fs.readFileSync(
    path.resolve(
      __dirname,
      "..",
      "assets",
      "codetrail",
      "agents",
      "opencode",
      "codetrail-evidence.md"
    ),
    "utf8"
  );
  for (const template of [codex, opencode]) {
    assert.equal(template.includes("skill: deny"), true);
    assert.equal(template.includes("Use `$codetrail`"), false);
    assert.equal(template.includes("index status --summary"), true);
    assert.equal(template.includes("explore flow"), true);
    assert.equal(template.includes("explore node <query> --compact"), true);
    assert.equal(template.includes("evidence` <= 6"), true);
    assert.equal(template.includes("relationships` <= 8"), true);
    assert.equal(template.includes("queries` <= 10"), true);
    assert.equal(template.includes("prefer <= 6 CodeTrail commands total"), true);
  }
});
