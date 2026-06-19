const { spawnSync } = require("node:child_process");
const readline = require("node:readline");
const { resolveCoreBinary } = require("./core-binary");

const VALUE_FLAGS = new Set([
  "--context",
  "--cursor",
  "--dir",
  "--exclude",
  "--ext",
  "--file-mode",
  "--file-pattern",
  "--include",
  "--input-mode",
  "--lang",
  "--limit",
  "--output",
  "--path",
  "--save-query"
]);

function stripInlineValue(arg) {
  const index = arg.indexOf("=");
  return index >= 0 ? arg.slice(0, index) : arg;
}

function optionValue(args, name, fallback = null) {
  const inline = args.find((arg) => arg.startsWith(`${name}=`));
  if (inline) return inline.slice(name.length + 1);
  const index = args.indexOf(name);
  return index >= 0 && args[index + 1] ? args[index + 1] : fallback;
}

function skipValueFlag(args, index) {
  const flag = stripInlineValue(args[index]);
  if (!VALUE_FLAGS.has(flag)) return index;
  if (args[index].includes("=")) return index;
  return index + 1;
}

function findCommandIndex(args) {
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg.startsWith("-")) {
      index = skipValueFlag(args, index);
      continue;
    }
    return index;
  }
  return -1;
}

function positionals(args) {
  const values = [];
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg.startsWith("-")) {
      index = skipValueFlag(args, index);
      continue;
    }
    values.push(arg);
  }
  return values;
}

function parseIndexProviderInstallArgs(args) {
  const commandIndex = findCommandIndex(args);
  if (
    commandIndex < 0 ||
    args[commandIndex] !== "index-provider" ||
    args[commandIndex + 1] !== "install"
  ) {
    return { matches: false };
  }

  const localArgs = args.slice(commandIndex + 2);
  const languages = positionals(localArgs);
  return {
    matches: true,
    help: localArgs.includes("--help") || localArgs.includes("-h"),
    languages,
    path: optionValue(args, "--path", process.cwd()),
    output: optionValue(args, "--output", "text"),
    dryRun: localArgs.includes("--dry-run"),
    force: localArgs.includes("--force"),
    yes: localArgs.includes("--yes") || localArgs.includes("-y")
  };
}

function shouldHandleInteractiveIndexProviderInstall(args, io = process) {
  const parsed = parseIndexProviderInstallArgs(args);
  return Boolean(
    parsed.matches &&
      !parsed.help &&
      parsed.languages.length === 0 &&
      parsed.output === "text" &&
      io.stdin &&
      io.stdin.isTTY &&
      io.stdout &&
      io.stdout.isTTY
  );
}

function corePlanArgs(parsed) {
  const args = [
    "--path",
    parsed.path,
    "--output",
    "json",
    "index-provider",
    "install",
    ...parsed.languages,
    "--dry-run"
  ];
  if (parsed.force) args.push("--force");
  return args;
}

function coreInstallArgs(parsed, languages, force = parsed.force) {
  const args = ["--path", parsed.path, "index-provider", "install", ...languages];
  if (force) args.push("--force");
  return args;
}

function runCoreJson(args) {
  const binaryPath = resolveCoreBinary();
  const result = spawnSync(binaryPath, args, {
    env: process.env,
    encoding: "utf8"
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const message = result.stderr.trim() || result.stdout.trim() || "index provider planning failed";
    throw new Error(message);
  }
  return JSON.parse(result.stdout);
}

function providerChoices(results) {
  return results.map((item) => {
    const command = item.args && item.args.length > 0
      ? `${item.command} ${item.args.join(" ")}`
      : item.command;
    return {
      id: item.language,
      label: `${item.language}: ${item.provider}`,
      status: item.status,
      command,
      envKey: item.envKey,
      installCommands: item.installCommands || [],
      available: item.availableBefore === true
    };
  });
}

function defaultSelectedProviders(choices) {
  const missing = choices
    .filter((choice) => !choice.available || choice.status !== "skipped_available")
    .map((choice) => choice.id);
  return missing.length > 0 ? missing : choices.map((choice) => choice.id);
}

function filterChoices(choices, query) {
  const normalized = query.trim().toLowerCase();
  if (!normalized) return choices;
  return choices.filter((choice) => {
    return (
      choice.id.toLowerCase().includes(normalized) ||
      choice.label.toLowerCase().includes(normalized) ||
      choice.command.toLowerCase().includes(normalized) ||
      choice.envKey.toLowerCase().includes(normalized)
    );
  });
}

function selectedSummary(choices, selected) {
  const labels = choices
    .filter((choice) => selected.has(choice.id))
    .map((choice) => choice.label);
  if (labels.length === 0) return "(none)";
  if (labels.length <= 3) return labels.join(", ");
  return `${labels.slice(0, 3).join(", ")} +${labels.length - 3} more`;
}

function clearLines(output, count) {
  if (count <= 0) return;
  output.write(`\x1b[${count}A`);
  for (let index = 0; index < count; index += 1) {
    output.write("\x1b[2K\x1b[1B");
  }
  output.write(`\x1b[${count}A`);
}

function selectProvidersInteractive(choices, initialSelected, io = process) {
  const input = io.stdin;
  const output = io.stdout;
  return new Promise((resolve) => {
    const selected = new Set(initialSelected);
    let query = "";
    let cursor = 0;
    let renderedLines = 0;
    let validationMessage = "";
    const wasRaw = input.isRaw;

    readline.emitKeypressEvents(input);
    if (input.setRawMode) input.setRawMode(true);
    if (input.resume) input.resume();

    const render = (state = "active") => {
      clearLines(output, renderedLines);
      const filtered = filterChoices(choices, query);
      if (cursor >= filtered.length) cursor = Math.max(0, filtered.length - 1);

      const lines = [];
      if (state === "cancel") {
        lines.push("x Select semantic index providers to install");
        lines.push("| Cancelled");
      } else if (state === "submit") {
        lines.push("v Select semantic index providers to install");
        lines.push(`| Selected: ${selectedSummary(choices, selected)}`);
      } else {
        lines.push("> Select semantic index providers to install");
        lines.push(`| Search: ${query}_`);
        lines.push("| Up/down move, space toggles, enter confirms, esc cancels");
        lines.push("|");
        if (filtered.length === 0) {
          lines.push("| No matches");
        } else {
          const visibleStart = Math.max(0, Math.min(cursor - 3, filtered.length - 8));
          const visibleItems = filtered.slice(visibleStart, visibleStart + 8);
          for (let index = 0; index < visibleItems.length; index += 1) {
            const choice = visibleItems[index];
            const actualIndex = visibleStart + index;
            const pointer = actualIndex === cursor ? ">" : " ";
            const mark = selected.has(choice.id) ? "[x]" : "[ ]";
            const status = choice.available ? "available" : "missing";
            lines.push(`| ${pointer} ${mark} ${choice.label} - ${status}, ${choice.command}`);
          }
        }
        lines.push("|");
        lines.push(`| Selected: ${selectedSummary(choices, selected)}`);
        if (validationMessage) lines.push(`| ${validationMessage}`);
      }

      output.write(`${lines.join("\n")}\n`);
      renderedLines = lines.length;
    };

    const cleanup = () => {
      input.removeListener("keypress", onKeypress);
      if (input.setRawMode) input.setRawMode(Boolean(wasRaw));
      if (input.pause) input.pause();
    };

    const submit = () => {
      if (selected.size === 0) {
        validationMessage = "Select at least one provider.";
        render();
        return;
      }
      render("submit");
      cleanup();
      resolve([...selected]);
    };

    const cancel = () => {
      render("cancel");
      cleanup();
      resolve(null);
    };

    const onKeypress = (_chunk, key) => {
      validationMessage = "";
      const filtered = filterChoices(choices, query);
      if (!key) return;
      if (key.name === "return") {
        submit();
        return;
      }
      if (key.name === "escape" || (key.ctrl && key.name === "c")) {
        cancel();
        return;
      }
      if (key.name === "up") {
        cursor = Math.max(0, cursor - 1);
        render();
        return;
      }
      if (key.name === "down") {
        cursor = Math.min(Math.max(0, filtered.length - 1), cursor + 1);
        render();
        return;
      }
      if (key.name === "space") {
        const choice = filtered[cursor];
        if (choice) {
          if (selected.has(choice.id)) selected.delete(choice.id);
          else selected.add(choice.id);
        }
        render();
        return;
      }
      if (key.name === "backspace") {
        query = query.slice(0, -1);
        cursor = 0;
        render();
        return;
      }
      if (key.sequence && !key.ctrl && !key.meta && key.sequence.length === 1) {
        query += key.sequence;
        cursor = 0;
        render();
      }
    };

    input.on("keypress", onKeypress);
    render();
  });
}

function installationPlanForSelection(selectedIds, choices, force, reinstallAvailable = false) {
  const selectedChoices = choices.filter((choice) => selectedIds.includes(choice.id));
  if (force || reinstallAvailable) {
    return {
      languages: selectedChoices.map((choice) => choice.id),
      force: true,
      needed: selectedChoices.length > 0,
      hasAvailable: selectedChoices.some((choice) => choice.available)
    };
  }
  const missing = selectedChoices.filter((choice) => !choice.available);
  return {
    languages: missing.map((choice) => choice.id),
    force: false,
    needed: missing.length > 0,
    hasAvailable: selectedChoices.some((choice) => choice.available)
  };
}

function buildInstallSummary(selectedIds, choices, dryRun, force = false) {
  const selected = choices.filter((choice) => selectedIds.includes(choice.id));
  const lines = [
    "Installation Summary",
    `  mode: ${dryRun ? "dry-run" : "install"}`,
    `  providers: ${selected.map((choice) => choice.label).join(", ")}`,
    "  commands:"
  ];
  for (const choice of selected) {
    lines.push(`    ${choice.label}:`);
    if (choice.available) {
      lines.push(
        force
          ? "      will reinstall because --force is enabled"
          : "      already available; no command will run unless --force is enabled"
      );
    }
    for (const command of choice.installCommands) {
      lines.push(`      ${command}`);
    }
  }
  return lines.join("\n");
}

function confirmInteractive(message, io = process, defaultValue = true) {
  return new Promise((resolve) => {
    const rl = readline.createInterface({
      input: io.stdin,
      output: io.stdout
    });
    rl.question(`${message} ${defaultValue ? "[Y/n]" : "[y/N]"} `, (answer) => {
      rl.close();
      const normalized = answer.trim().toLowerCase();
      resolve(normalized === "" ? defaultValue : normalized === "y" || normalized === "yes");
    });
  });
}

async function maybeRunIndexProviderInstallInteractive(args, io = process) {
  if (!shouldHandleInteractiveIndexProviderInstall(args, io)) return false;
  const parsed = parseIndexProviderInstallArgs(args);
  const plan = runCoreJson(corePlanArgs(parsed));
  const choices = providerChoices(plan.results || []);
  if (choices.length === 0) {
    io.stdout.write("No semantic index providers were detected for this workspace.\n");
    return true;
  }

  const selected = await selectProvidersInteractive(
    choices,
    defaultSelectedProviders(choices),
    io
  );
  if (!selected) return true;

  io.stdout.write(`\n${buildInstallSummary(selected, choices, parsed.dryRun, parsed.force)}\n\n`);

  let installPlan = installationPlanForSelection(selected, choices, parsed.force, false);
  if (!parsed.dryRun && !parsed.force && installPlan.hasAvailable && !parsed.yes) {
    const reinstall = await confirmInteractive(
      installPlan.needed
        ? "Reinstall already available providers with --force?"
        : "Selected providers are already available. Reinstall with --force?",
      io,
      false
    );
    installPlan = installationPlanForSelection(selected, choices, parsed.force, reinstall);
  }

  if (!parsed.dryRun && !installPlan.needed) {
    io.stdout.write("No provider installation needed. Use --force to reinstall.\n");
    return true;
  }

  if (!parsed.dryRun && !parsed.yes) {
    const confirmed = await confirmInteractive("Proceed with provider installation?", io);
    if (!confirmed) {
      io.stdout.write("Installation cancelled\n");
      return true;
    }
  }

  if (parsed.dryRun) return true;

  const binaryPath = resolveCoreBinary();
  const result = spawnSync(binaryPath, coreInstallArgs(parsed, installPlan.languages, installPlan.force), {
    env: process.env,
    stdio: "inherit"
  });
  if (result.error) throw result.error;
  process.exit(result.status ?? 1);
}

module.exports = {
  parseIndexProviderInstallArgs,
  shouldHandleInteractiveIndexProviderInstall,
  providerChoices,
  defaultSelectedProviders,
  filterChoices,
  installationPlanForSelection,
  buildInstallSummary,
  maybeRunIndexProviderInstallInteractive
};
