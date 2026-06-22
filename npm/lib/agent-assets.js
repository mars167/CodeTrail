const path = require("node:path");

const NPM_ROOT = path.resolve(__dirname, "..");
const ASSET_ROOT = path.join(NPM_ROOT, "assets", "codetrail");
const SKILL = path.join(ASSET_ROOT, "SKILL.md");

const TARGETS = [
  {
    id: "codex",
    label: "Codex",
    files: [
      { source: SKILL, user: [".codex", "skills", "codetrail", "SKILL.md"], project: [".codex", "skills", "codetrail", "SKILL.md"] }
    ]
  },
  {
    id: "claude",
    label: "Claude Code",
    files: [
      { source: SKILL, user: [".claude", "skills", "codetrail", "SKILL.md"], project: [".claude", "skills", "codetrail", "SKILL.md"] }
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
