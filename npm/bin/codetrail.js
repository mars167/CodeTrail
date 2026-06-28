#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const { resolveCoreBinary } = require("../lib/core-binary");
const { renderNodeCompletions } = require("../lib/completions");
const { runCore } = require("../lib/run-core");
const { listTargets, installTarget, removeTarget, doctorTarget } = require("../lib/agent-install");
const { maybeRunIndexProviderInstallInteractive } = require("../lib/index-provider-install-ui");
const { maybeRunSkillInstallInteractive } = require("../lib/skill-install-ui");
const { compareVersions, fetchLatestVersion, installVersion, maybePrintUpdateNotice } = require("../lib/update");

function currentVersion() {
  const pkg = JSON.parse(fs.readFileSync(path.join(__dirname, "..", "package.json"), "utf8"));
  return pkg.version;
}

function optionValue(args, name, fallback) {
  const inline = args.find((arg) => arg.startsWith(`${name}=`));
  if (inline) return inline.slice(name.length + 1);
  const index = args.indexOf(name);
  return index >= 0 && args[index + 1] ? args[index + 1] : fallback;
}

function hasOption(args, name) {
  return args.includes(name) || args.some((arg) => arg.startsWith(`${name}=`));
}

function installOptions(args) {
  const scope = optionValue(args, "--scope", "user");
  if (!["user", "project"].includes(scope)) {
    throw new Error("scope must be user or project");
  }
  if (hasOption(args, "--path")) {
    throw new Error("--path is reserved for workspace roots; use --project-root for project-scope installs");
  }
  const projectRoot = optionValue(args, "--project-root", null);
  if (projectRoot && scope !== "project") {
    throw new Error("--project-root can only be used with --scope project");
  }
  return {
    scope,
    project: projectRoot || process.cwd(),
    dryRun: args.includes("--dry-run"),
    force: args.includes("--force")
  };
}

function handleInstallCommand(kind, args) {
  const action = args[1] || "list";
  if (action === "list") {
    process.stdout.write(`${JSON.stringify({ kind, targets: listTargets() }, null, 2)}\n`);
    return true;
  }

  const target = args[2];
  const fn = action === "add"
    ? installTarget
    : action === "remove"
      ? removeTarget
      : action === "doctor"
        ? doctorTarget
        : null;

  if (!fn || !target) {
    throw new Error(`usage: codetrail ${kind} list|add|remove|doctor <target> [--scope user|project] [--project-root <path>] [--dry-run] [--force]`);
  }

  const value = fn(target, installOptions(args));
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
  return true;
}

function normalizedArgs() {
  const args = process.argv.slice(2);
  if (path.basename(process.argv[1] || "") === "codetrail-skills" && args[0] !== "skills" && args[0] !== "agents") {
    return ["skills", ...args];
  }
  return args;
}

async function main() {
  const args = normalizedArgs();

  if (args[0] === "update" && args[1] === "check") {
    const latest = await fetchLatestVersion();
    const current = currentVersion();
    const updateAvailable = compareVersions(current, latest) < 0;
    process.stdout.write(`${JSON.stringify({ current, latest, updateAvailable }, null, 2)}\n`);
    return;
  }

  if (args[0] === "update" && args[1] === "install") {
    const version = optionValue(args, "--version", null) || await fetchLatestVersion();
    process.exit(installVersion(version));
  }

  if (args[0] === "update") {
    throw new Error("usage: codetrail update check|install [--version <version>]");
  }

  if ((args[0] === "skills" || args[0] === "agents") && handleInstallCommand(args[0], args)) {
    return;
  }

  if (await maybeRunIndexProviderInstallInteractive(args)) {
    return;
  }

  if (await maybeRunSkillInstallInteractive(args)) {
    return;
  }

  if (args[0] === "completions" && args[1]) {
    const binaryPath = resolveCoreBinary();
    process.stdout.write(renderNodeCompletions(binaryPath, args[1]));
    return;
  }

  await maybePrintUpdateNotice(currentVersion(), args);
  const binaryPath = resolveCoreBinary();
  runCore(binaryPath, args);
}

main().catch((error) => {
  console.error(`codetrail: ${error.message}`);
  process.exit(1);
});
