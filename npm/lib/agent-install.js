const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { TARGETS, targetById } = require("./agent-assets");

function listTargets() {
  return TARGETS.map(({ id, label }) => ({ id, label }));
}

function baseDir(options) {
  if (options.scope === "project") {
    return options.project || process.cwd();
  }
  return options.home || os.homedir();
}

function destinationFor(file, options) {
  const segments = options.scope === "project" ? file.project : file.user;
  if (!segments) {
    throw new Error(`target does not support ${options.scope} scope`);
  }
  return path.join(baseDir(options), ...segments);
}

function planTarget(id, options = {}) {
  const target = targetById(id);
  const scope = options.scope || "user";
  return target.files.map((file) => ({
    source: file.source,
    destination: destinationFor(file, { ...options, scope })
  }));
}

function installTarget(id, options = {}) {
  const planned = planTarget(id, options);
  if (options.dryRun) {
    return { changed: false, planned };
  }

  let changed = false;
  for (const item of planned) {
    if (fs.existsSync(item.destination) && !options.force) {
      continue;
    }
    fs.mkdirSync(path.dirname(item.destination), { recursive: true });
    fs.copyFileSync(item.source, item.destination);
    changed = true;
  }
  return { changed, planned };
}

function removeTarget(id, options = {}) {
  const planned = planTarget(id, options);
  let changed = false;
  for (const item of planned) {
    if (fs.existsSync(item.destination)) {
      fs.rmSync(item.destination);
      changed = true;
    }
  }
  return { changed, planned };
}

function doctorTarget(id, options = {}) {
  const planned = planTarget(id, options);
  const files = planned.map((item) => ({
    destination: item.destination,
    exists: fs.existsSync(item.destination)
  }));
  return { ok: files.every((file) => file.exists), files };
}

module.exports = {
  listTargets,
  installTarget,
  removeTarget,
  doctorTarget
};
