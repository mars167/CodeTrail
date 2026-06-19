const { spawnSync } = require("node:child_process");

const WRAPPER_COMMANDS = ["update", "skills", "agents"];
const UPDATE_COMMANDS = ["check", "install"];
const INSTALL_COMMANDS = ["list", "add", "remove", "doctor"];

function appendMissingWords(value, words) {
  const existing = new Set(value.trim().split(/\s+/).filter(Boolean));
  return [...value.trim().split(/\s+/).filter(Boolean), ...words.filter((word) => !existing.has(word))].join(" ");
}

function augmentBashCompletion(coreScript) {
  let script = `${coreScript}`.replace(/commands="([^"]*)"/, (_, words) => {
    return `commands="${appendMissingWords(words, WRAPPER_COMMANDS)}"`;
  });
  script = script.replace(
    "local cur prev commands query_cmds index_cmds hooks_cmds shells",
    "local cur prev commands query_cmds index_cmds hooks_cmds shells update_cmds skill_cmds agent_cmds"
  );
  script = script.replace(
    'hooks_cmds="install uninstall status"',
    'hooks_cmds="install uninstall status"\n  update_cmds="check install"\n  skill_cmds="list add remove doctor"\n  agent_cmds="list add remove doctor"'
  );
  script = script.replace(
    "completions)\n      COMPREPLY",
    "update)\n      COMPREPLY=( $(compgen -W \"$update_cmds\" -- \"$cur\") )\n      return 0\n      ;;\n    skills)\n      COMPREPLY=( $(compgen -W \"$skill_cmds\" -- \"$cur\") )\n      return 0\n      ;;\n    agents)\n      COMPREPLY=( $(compgen -W \"$agent_cmds\" -- \"$cur\") )\n      return 0\n      ;;\n    completions)\n      COMPREPLY"
  );
  if (!script.includes('update_cmds="check install"')) {
    script = `${script.trimEnd()}\nupdate_cmds="check install"\nskill_cmds="list add remove doctor"\nagent_cmds="list add remove doctor"\n`;
  }
  return script;
}

function augmentZshCompletion(coreScript) {
  let script = `${coreScript}`.replace(/commands=\(([^)]*)\)/, (_, words) => {
    return `commands=(${appendMissingWords(words, WRAPPER_COMMANDS)})`;
  });
  script = script.replace(
    "local -a commands query_cmds index_cmds hooks_cmds shells global_opts",
    "local -a commands query_cmds index_cmds hooks_cmds shells global_opts update_cmds skill_cmds agent_cmds"
  );
  script = script.replace(
    "hooks_cmds=(install uninstall status)",
    "hooks_cmds=(install uninstall status)\n  update_cmds=(check install)\n  skill_cmds=(list add remove doctor)\n  agent_cmds=(list add remove doctor)"
  );
  script = script.replace(
    "completions)\n      _describe 'shell' shells",
    "update)\n      _describe 'update command' update_cmds\n      ;;\n    skills)\n      _describe 'skills command' skill_cmds\n      ;;\n    agents)\n      _describe 'agents command' agent_cmds\n      ;;\n    completions)\n      _describe 'shell' shells"
  );
  if (!script.includes("update_cmds=(check install)")) {
    script = `${script.trimEnd()}\nupdate_cmds=(check install)\nskill_cmds=(list add remove doctor)\nagent_cmds=(list add remove doctor)\n`;
  }
  return script;
}

function augmentFishCompletion(coreScript) {
  return `${coreScript.trimEnd()}
complete -c codetrail -f -a update
complete -c codetrail -f -a skills
complete -c codetrail -f -a agents
complete -c codetrail -n '__fish_seen_subcommand_from update' -a '${UPDATE_COMMANDS.join(" ")}'
complete -c codetrail -n '__fish_seen_subcommand_from skills' -a '${INSTALL_COMMANDS.join(" ")}'
complete -c codetrail -n '__fish_seen_subcommand_from agents' -a '${INSTALL_COMMANDS.join(" ")}'
`;
}

function augmentCompletionScript(shell, coreScript) {
  if (shell === "bash") return augmentBashCompletion(coreScript);
  if (shell === "zsh") return augmentZshCompletion(coreScript);
  if (shell === "fish") return augmentFishCompletion(coreScript);
  return coreScript;
}

function renderNodeCompletions(binaryPath, shell) {
  const result = spawnSync(binaryPath, ["completions", shell], {
    encoding: "utf8",
    env: process.env
  });
  if (result.stderr) process.stderr.write(result.stderr);
  if (result.status !== 0) process.exit(result.status ?? 1);
  return augmentCompletionScript(shell, result.stdout);
}

module.exports = {
  augmentCompletionScript,
  renderNodeCompletions
};
