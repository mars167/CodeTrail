const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const {
  parseSkillInstallArgs,
  shouldHandleInteractiveSkillInstall,
  targetChoices,
  defaultSelectedTargets,
  filterChoices,
  buildInstallSummary,
  formatInstallResults,
  maybeRunSkillInstallInteractive
} = require("../lib/skill-install-ui");

test("parses skill install with global path and project-root options", () => {
  const parsed = parseSkillInstallArgs([
    "--path",
    "/repo",
    "skill",
    "install",
    "--scope",
    "project",
    "--project-root",
    "/override",
    "--dry-run"
  ]);

  assert.equal(parsed.matches, true);
  assert.equal(parsed.target, null);
  assert.equal(parsed.scope, "project");
  assert.equal(parsed.projectRoot, "/override");
  assert.equal(parsed.project, "/override");
  assert.equal(parsed.dryRun, true);
});

test("parses explicit target without enabling interactive install", () => {
  const parsed = parseSkillInstallArgs(["skill", "install", "codex", "--force"]);

  assert.equal(parsed.matches, true);
  assert.equal(parsed.target, "codex");
  assert.equal(parsed.force, true);
  assert.equal(
    shouldHandleInteractiveSkillInstall(["skill", "install", "codex"], {
      stdin: { isTTY: true },
      stdout: { isTTY: true }
    }),
    false
  );
});

test("handles interactive install only for text tty with no target", () => {
  const tty = { stdin: { isTTY: true }, stdout: { isTTY: true } };
  const pipe = { stdin: { isTTY: false }, stdout: { isTTY: true } };

  assert.equal(shouldHandleInteractiveSkillInstall(["skill", "install"], tty), true);
  assert.equal(
    shouldHandleInteractiveSkillInstall(["--output", "json", "skill", "install"], tty),
    false
  );
  assert.equal(shouldHandleInteractiveSkillInstall(["skill", "install"], pipe), false);
  assert.equal(shouldHandleInteractiveSkillInstall(["skill", "install", "--help"], tty), false);
});

test("project-root requires project scope for interactive skill install", async () => {
  const tty = { stdin: { isTTY: true }, stdout: { isTTY: true } };
  await assert.rejects(
    () =>
      maybeRunSkillInstallInteractive(["skill", "install", "--project-root", "/repo"], tty),
    /--project-root can only be used with --scope project/
  );
});

test("builds choices with destination hints and searchable labels", () => {
  const project = fs.mkdtempSync(path.join(os.tmpdir(), "codetrail-ui-project-"));
  const choices = targetChoices({ scope: "project", project });

  assert.equal(choices.some((choice) => choice.id === "codex"), true);
  assert.equal(choices.find((choice) => choice.id === "codex").hint.includes(".codex"), true);
  assert.deepEqual(
    filterChoices(choices, "cursor").map((choice) => choice.id),
    ["cursor"]
  );
});

test("defaults to codex when no installed target files exist", () => {
  const project = fs.mkdtempSync(path.join(os.tmpdir(), "codetrail-ui-project-"));
  const choices = targetChoices({ scope: "project", project });

  assert.deepEqual(defaultSelectedTargets(choices), ["codex"]);
});

test("summaries and result output include all selected targets", () => {
  const project = fs.mkdtempSync(path.join(os.tmpdir(), "codetrail-ui-project-"));
  const options = { scope: "project", project, dryRun: true, force: false };
  const summary = buildInstallSummary(["codex", "cursor"], options);
  const results = new Map([
    ["codex", { changed: false }],
    ["cursor", { changed: false }]
  ]);
  const output = formatInstallResults(["codex", "cursor"], options, results);

  assert.match(summary, /Installation Summary/);
  assert.match(summary, /Codex/);
  assert.match(summary, /Cursor/);
  assert.match(output, /Planned CodeTrail skill install/);
  assert.match(output, /\.cursor/);
});
