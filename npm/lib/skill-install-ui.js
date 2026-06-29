const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const readline = require("node:readline");
const { listTargets, planTarget, installTarget } = require("./agent-install");

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
  "--project-root",
  "--save-query",
  "--scope"
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

function firstPositional(args) {
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg.startsWith("-")) {
      index = skipValueFlag(args, index);
      continue;
    }
    return arg;
  }
  return null;
}

function parseSkillInstallArgs(args) {
  const commandIndex = findCommandIndex(args);
  if (commandIndex < 0 || args[commandIndex] !== "skill" || args[commandIndex + 1] !== "install") {
    return { matches: false };
  }

  const localArgs = args.slice(commandIndex + 2);
  const scope = optionValue(localArgs, "--scope", "user");
  const projectRoot = optionValue(localArgs, "--project-root", null);
  const globalPath = optionValue(args, "--path", null);
  const output = optionValue(args, "--output", "text");
  return {
    matches: true,
    help: localArgs.includes("--help") || localArgs.includes("-h"),
    target: firstPositional(localArgs),
    scope,
    projectRoot,
    project: projectRoot || globalPath || process.cwd(),
    dryRun: localArgs.includes("--dry-run"),
    force: localArgs.includes("--force"),
    yes: localArgs.includes("--yes") || localArgs.includes("-y"),
    output
  };
}

function shouldHandleInteractiveSkillInstall(args, io = process) {
  const parsed = parseSkillInstallArgs(args);
  return Boolean(
    parsed.matches &&
      !parsed.help &&
      !parsed.target &&
      parsed.output === "text" &&
      io.stdin &&
      io.stdin.isTTY &&
      io.stdout &&
      io.stdout.isTTY
  );
}

function homeRelative(value) {
  const home = os.homedir();
  if (value === home) return "~";
  if (value.startsWith(`${home}${path.sep}`)) return `~${value.slice(home.length)}`;
  return value;
}

function projectRelative(value, project) {
  const resolvedProject = path.resolve(project || process.cwd());
  const resolvedValue = path.resolve(value);
  if (resolvedValue === resolvedProject) return ".";
  if (resolvedValue.startsWith(`${resolvedProject}${path.sep}`)) {
    return `.${path.sep}${path.relative(resolvedProject, resolvedValue)}`;
  }
  return homeRelative(resolvedValue);
}

function shortenPath(value, options) {
  return projectRelative(value, options.project);
}

function targetChoices(options) {
  return listTargets().map((target) => {
    const planned = planTarget(target.id, options);
    const first = planned[0] ? shortenPath(planned[0].destination, options) : "";
    const suffix = planned.length > 1 ? ` +${planned.length - 1} file` : "";
    return {
      id: target.id,
      label: target.label,
      hint: `${first}${suffix}`,
      planned
    };
  });
}

function defaultSelectedTargets(choices) {
  const installed = choices
    .filter((choice) => choice.planned.some((item) => fs.existsSync(item.destination)))
    .map((choice) => choice.id);
  if (installed.length > 0) return installed;
  return choices.some((choice) => choice.id === "codex") ? ["codex"] : [];
}

function filterChoices(choices, query) {
  const normalized = query.trim().toLowerCase();
  if (!normalized) return choices;
  return choices.filter((choice) => {
    return (
      choice.id.toLowerCase().includes(normalized) ||
      choice.label.toLowerCase().includes(normalized) ||
      choice.hint.toLowerCase().includes(normalized)
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

function selectTargetsInteractive(choices, initialSelected, io = process) {
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
        lines.push("x Select target agents for CodeTrail skill install");
        lines.push("| Cancelled");
      } else if (state === "submit") {
        lines.push("v Select target agents for CodeTrail skill install");
        lines.push(`| Selected: ${selectedSummary(choices, selected)}`);
      } else {
        lines.push("> Select target agents for CodeTrail skill install");
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
            lines.push(`| ${pointer} ${mark} ${choice.label} (${choice.id}) - ${choice.hint}`);
          }
          const hiddenAfter = filtered.length - visibleStart - visibleItems.length;
          if (visibleStart > 0 || hiddenAfter > 0) {
            const hiddenBeforeText = visibleStart > 0 ? `${visibleStart} above` : "";
            const separator = visibleStart > 0 && hiddenAfter > 0 ? ", " : "";
            const hiddenAfterText = hiddenAfter > 0 ? `${hiddenAfter} below` : "";
            lines.push(`| ${hiddenBeforeText}${separator}${hiddenAfterText}`);
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
        validationMessage = "Select at least one target.";
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

function buildInstallSummary(targetIds, options) {
  const choices = targetChoices(options).filter((choice) => targetIds.includes(choice.id));
  const lines = [
    "Installation Summary",
    `  scope: ${options.scope}`,
    `  mode: ${options.dryRun ? "dry-run" : "copy"}`,
    `  targets: ${choices.map((choice) => choice.label).join(", ")}`,
    "  files:"
  ];
  for (const choice of choices) {
    for (const item of choice.planned) {
      lines.push(`    ${choice.label}: ${shortenPath(item.destination, options)}`);
    }
  }
  return lines.join("\n");
}

function confirmInteractive(message, io = process) {
  return new Promise((resolve) => {
    const rl = readline.createInterface({
      input: io.stdin,
      output: io.stdout
    });
    rl.question(`${message} [Y/n] `, (answer) => {
      rl.close();
      const normalized = answer.trim().toLowerCase();
      resolve(normalized === "" || normalized === "y" || normalized === "yes");
    });
  });
}

function formatInstallResults(targetIds, options, results) {
  const choices = targetChoices(options).filter((choice) => targetIds.includes(choice.id));
  const title = options.dryRun
    ? "Planned CodeTrail skill install:"
    : "Installed CodeTrail skill assets:";
  const lines = [title];
  for (const choice of choices) {
    const result = results.get(choice.id);
    const status = result && result.changed ? "changed" : "unchanged";
    lines.push(`  ${choice.label} (${choice.id}): ${status}`);
    for (const item of choice.planned) {
      lines.push(`    ${shortenPath(item.destination, options)}`);
    }
  }
  return lines.join("\n");
}

async function maybeRunSkillInstallInteractive(args, io = process) {
  if (!shouldHandleInteractiveSkillInstall(args, io)) return false;
  const parsed = parseSkillInstallArgs(args);
  if (!["user", "project"].includes(parsed.scope)) {
    throw new Error("scope must be user or project");
  }
  if (parsed.projectRoot && parsed.scope !== "project") {
    throw new Error("--project-root can only be used with --scope project");
  }
  const options = {
    scope: parsed.scope,
    project: parsed.project,
    dryRun: parsed.dryRun,
    force: parsed.force
  };
  const choices = targetChoices(options);
  const selected = await selectTargetsInteractive(choices, defaultSelectedTargets(choices), io);
  if (!selected) return true;

  io.stdout.write(`\n${buildInstallSummary(selected, options)}\n\n`);
  if (!parsed.dryRun && !parsed.yes) {
    const confirmed = await confirmInteractive("Proceed with installation?", io);
    if (!confirmed) {
      io.stdout.write("Installation cancelled\n");
      return true;
    }
  }

  const results = new Map();
  for (const target of selected) {
    results.set(target, installTarget(target, options));
  }
  io.stdout.write(`${formatInstallResults(selected, options, results)}\n`);
  return true;
}

module.exports = {
  parseSkillInstallArgs,
  shouldHandleInteractiveSkillInstall,
  targetChoices,
  defaultSelectedTargets,
  filterChoices,
  buildInstallSummary,
  formatInstallResults,
  maybeRunSkillInstallInteractive
};
