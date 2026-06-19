const path = require("node:path");

const NPM_ROOT = path.resolve(__dirname, "..");
const ASSET_ROOT = path.join(NPM_ROOT, "assets", "codetrail");
const SKILL = path.join(ASSET_ROOT, "SKILL.md");
const CODEX_AGENT = path.join(ASSET_ROOT, "agents", "codex", "codetrail-evidence.toml");
const OPENCODE_AGENT = path.join(ASSET_ROOT, "agents", "opencode", "codetrail-evidence.md");
const OPENAI_AGENT = path.join(ASSET_ROOT, "agents", "openai.yaml");

const TARGETS = [
  {
    id: "codex",
    label: "Codex",
    files: [
      { source: SKILL, user: [".codex", "skills", "codetrail", "SKILL.md"], project: [".codex", "skills", "codetrail", "SKILL.md"] },
      { source: CODEX_AGENT, user: [".codex", "agents", "codetrail-evidence.toml"], project: [".codex", "agents", "codetrail-evidence.toml"] }
    ]
  },
  {
    id: "opencode",
    label: "OpenCode",
    files: [
      { source: OPENCODE_AGENT, user: [".config", "opencode", "agents", "codetrail-evidence.md"], project: [".opencode", "agents", "codetrail-evidence.md"] }
    ]
  },
  {
    id: "claude",
    label: "Claude Code",
    files: [
      { source: OPENCODE_AGENT, user: [".claude", "agents", "codetrail-evidence.md"], project: [".claude", "agents", "codetrail-evidence.md"] }
    ]
  },
  {
    id: "cursor",
    label: "Cursor",
    files: [
      { source: SKILL, user: [".cursor", "rules", "codetrail.mdc"], project: [".cursor", "rules", "codetrail.mdc"] }
    ]
  },
  {
    id: "continue",
    label: "Continue",
    files: [
      { source: SKILL, user: [".continue", "rules", "codetrail.md"], project: [".continue", "rules", "codetrail.md"] }
    ]
  },
  {
    id: "cline",
    label: "Cline",
    files: [
      { source: SKILL, user: [".cline", "rules", "codetrail.md"], project: [".clinerules", "codetrail.md"] }
    ]
  },
  {
    id: "roo",
    label: "Roo",
    files: [
      { source: SKILL, user: [".roo", "rules", "codetrail.md"], project: [".roo", "rules", "codetrail.md"] }
    ]
  },
  {
    id: "openai",
    label: "OpenAI Agents",
    files: [
      { source: OPENAI_AGENT, user: [".openai", "agents", "codetrail.yaml"], project: [".openai", "agents", "codetrail.yaml"] }
    ]
  }
];

function targetById(id) {
  const target = TARGETS.find((candidate) => candidate.id === id);
  if (!target) {
    throw new Error(`unknown agent target: ${id}`);
  }
  return target;
}

module.exports = {
  TARGETS,
  targetById
};
