const test = require("node:test");
const assert = require("node:assert/strict");
const {
  parseIndexProviderInstallArgs,
  shouldHandleInteractiveIndexProviderInstall,
  providerChoices,
  defaultSelectedProviders,
  filterChoices,
  installationPlanForSelection,
  buildInstallSummary
} = require("../lib/index-provider-install-ui");

test("parses index-provider install command", () => {
  const parsed = parseIndexProviderInstallArgs([
    "--path",
    "/repo",
    "index-provider",
    "install",
    "--force"
  ]);

  assert.equal(parsed.matches, true);
  assert.equal(parsed.path, "/repo");
  assert.deepEqual(parsed.languages, []);
  assert.equal(parsed.force, true);
});

test("parses explicit provider languages", () => {
  const parsed = parseIndexProviderInstallArgs([
    "index-provider",
    "install",
    "java",
    "typescript",
    "--dry-run"
  ]);

  assert.equal(parsed.matches, true);
  assert.deepEqual(parsed.languages, ["java", "typescript"]);
  assert.equal(parsed.dryRun, true);
});

test("handles interactive provider install only for text tty without language args", () => {
  const tty = { stdin: { isTTY: true }, stdout: { isTTY: true } };
  const pipe = { stdin: { isTTY: false }, stdout: { isTTY: true } };

  assert.equal(
    shouldHandleInteractiveIndexProviderInstall(["index-provider", "install"], tty),
    true
  );
  assert.equal(
    shouldHandleInteractiveIndexProviderInstall(["index-provider", "install", "java"], tty),
    false
  );
  assert.equal(
    shouldHandleInteractiveIndexProviderInstall([
      "--output",
      "json",
      "index-provider",
      "install"
    ], tty),
    false
  );
  assert.equal(
    shouldHandleInteractiveIndexProviderInstall(["index-provider", "install"], pipe),
    false
  );
});

test("builds provider choices from dry-run JSON results", () => {
  const choices = providerChoices([
    {
      language: "java",
      provider: "scip-java",
      command: "scip-java",
      args: ["index"],
      envKey: "CODETRAIL_SCIP_JAVA",
      installCommands: ["install java"],
      status: "planned",
      availableBefore: false
    },
    {
      language: "rust",
      provider: "rust-analyzer-scip",
      command: "rust-analyzer",
      args: ["scip", "."],
      envKey: "CODETRAIL_SCIP_RUST",
      installCommands: ["rustup component add rust-analyzer"],
      status: "skipped_available",
      availableBefore: true
    }
  ]);

  assert.deepEqual(defaultSelectedProviders(choices), ["java"]);
  assert.deepEqual(
    filterChoices(choices, "rust").map((choice) => choice.id),
    ["rust"]
  );
  assert.equal(choices[0].command, "scip-java index");
});

function sampleProviderChoices() {
  return providerChoices([
    {
      language: "java",
      provider: "scip-java",
      command: "scip-java",
      args: ["index"],
      envKey: "CODETRAIL_SCIP_JAVA",
      installCommands: ["install java"],
      status: "planned",
      availableBefore: false
    },
    {
      language: "rust",
      provider: "rust-analyzer-scip",
      command: "rust-analyzer",
      args: ["scip", "."],
      envKey: "CODETRAIL_SCIP_RUST",
      installCommands: ["rustup component add rust-analyzer"],
      status: "skipped_available",
      availableBefore: true
    }
  ]);
}

test("provider install plan skips available providers unless forced", () => {
  const choices = sampleProviderChoices();

  assert.deepEqual(installationPlanForSelection(["rust"], choices, false, false), {
    languages: [],
    force: false,
    needed: false,
    hasAvailable: true
  });
  assert.deepEqual(installationPlanForSelection(["java", "rust"], choices, false, false), {
    languages: ["java"],
    force: false,
    needed: true,
    hasAvailable: true
  });
  assert.deepEqual(installationPlanForSelection(["rust"], choices, false, true), {
    languages: ["rust"],
    force: true,
    needed: true,
    hasAvailable: true
  });
});

test("provider install summary includes commands and available status", () => {
  const choices = sampleProviderChoices();

  const summary = buildInstallSummary(["java", "rust"], choices, false);

  assert.match(summary, /Installation Summary/);
  assert.match(summary, /java: scip-java/);
  assert.match(summary, /install java/);
  assert.match(summary, /already available; no command will run unless --force is enabled/);
  assert.match(buildInstallSummary(["rust"], choices, false, true), /will reinstall/);
});
