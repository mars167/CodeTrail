const test = require("node:test");
const assert = require("node:assert/strict");
const { augmentCompletionScript } = require("../lib/completions");

test("adds node wrapper commands to bash completions", () => {
  const script = augmentCompletionScript("bash", 'commands="find index completions"');
  assert.match(script, /commands="find index completions update skills agents"/);
  assert.match(script, /update_cmds="check install"/);
  assert.match(script, /skill_cmds="list add remove doctor"/);
  assert.match(script, /agent_cmds="list add remove doctor"/);
});

test("adds node wrapper commands to zsh completions", () => {
  const script = augmentCompletionScript("zsh", "commands=(find index completions)");
  assert.match(script, /commands=\(find index completions update skills agents\)/);
  assert.match(script, /update_cmds=\(check install\)/);
  assert.match(script, /skill_cmds=\(list add remove doctor\)/);
  assert.match(script, /agent_cmds=\(list add remove doctor\)/);
});

test("adds node wrapper commands to fish completions", () => {
  const script = augmentCompletionScript("fish", "complete -c codetrail -f -a find");
  assert.match(script, /complete -c codetrail -f -a update/);
  assert.match(script, /__fish_seen_subcommand_from skills/);
  assert.match(script, /__fish_seen_subcommand_from agents/);
});
