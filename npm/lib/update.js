const fs = require("node:fs");
const https = require("node:https");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const { isJsonOutput } = require("./args");

const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;
const REGISTRY_URL = "https://registry.npmjs.org/codetrail/latest";

function isTruthyEnv(value) {
  return /^(1|true|yes)$/i.test(String(value || ""));
}

function shouldSkipUpdateCheck({ ci, stderrIsTTY, args, noUpdateCheck }) {
  return isTruthyEnv(ci) || !stderrIsTTY || isJsonOutput(args) || noUpdateCheck === "1";
}

function updateCachePath(home = os.homedir()) {
  return path.join(home, ".codetrail", "update-check.json");
}

function readCache(cachePath = updateCachePath()) {
  try {
    return JSON.parse(fs.readFileSync(cachePath, "utf8"));
  } catch {
    return null;
  }
}

function writeCache(value, cachePath = updateCachePath()) {
  fs.mkdirSync(path.dirname(cachePath), { recursive: true });
  fs.writeFileSync(cachePath, `${JSON.stringify(value, null, 2)}\n`);
}

function parseVersion(version) {
  const [core, prerelease] = version.split("-", 2);
  const parts = core.split(".").map((part) => Number.parseInt(part, 10));
  return {
    major: parts[0] || 0,
    minor: parts[1] || 0,
    patch: parts[2] || 0,
    prerelease: prerelease || ""
  };
}

function compareVersions(left, right) {
  const a = parseVersion(left);
  const b = parseVersion(right);
  for (const key of ["major", "minor", "patch"]) {
    if (a[key] < b[key]) return -1;
    if (a[key] > b[key]) return 1;
  }
  if (a.prerelease === b.prerelease) return 0;
  if (a.prerelease && !b.prerelease) return -1;
  if (!a.prerelease && b.prerelease) return 1;
  return a.prerelease < b.prerelease ? -1 : 1;
}

function defaultFetchJson(url) {
  return new Promise((resolve, reject) => {
    https
      .get(url, { headers: { accept: "application/json", "user-agent": "codetrail-update-check" } }, (response) => {
        let body = "";
        response.setEncoding("utf8");
        response.on("data", (chunk) => {
          body += chunk;
        });
        response.on("end", () => {
          if (response.statusCode < 200 || response.statusCode >= 300) {
            reject(new Error(`npm registry returned ${response.statusCode}`));
            return;
          }
          resolve(JSON.parse(body));
        });
      })
      .on("error", reject);
  });
}

async function fetchLatestVersion(fetchJson = defaultFetchJson) {
  const json = await fetchJson(REGISTRY_URL);
  return json.version;
}

async function maybePrintUpdateNotice(currentVersion, args, env = process.env, now = Date.now()) {
  if (shouldSkipUpdateCheck({
    ci: env.CI,
    stderrIsTTY: process.stderr.isTTY,
    args,
    noUpdateCheck: env.CODETRAIL_NO_UPDATE_CHECK
  })) {
    return;
  }

  const cachePath = updateCachePath();
  const cached = readCache(cachePath);
  if (cached && now - cached.checkedAtEpochMs < CHECK_INTERVAL_MS) {
    if (cached.latest && compareVersions(currentVersion, cached.latest) < 0) {
      console.error(`codetrail: update available ${currentVersion} -> ${cached.latest}. Run: codetrail update install`);
    }
    return;
  }

  try {
    const latest = await fetchLatestVersion();
    try {
      writeCache({ checkedAtEpochMs: now, current: currentVersion, latest }, cachePath);
    } catch {
      return;
    }
    if (compareVersions(currentVersion, latest) < 0) {
      console.error(`codetrail: update available ${currentVersion} -> ${latest}. Run: codetrail update install`);
    }
  } catch (error) {
    try {
      writeCache({ checkedAtEpochMs: now, current: currentVersion, error: error.message }, cachePath);
    } catch {
      return;
    }
  }
}

function buildInstallArgs(version) {
  return ["install", "-g", `codetrail@${version}`];
}

function packageTagForVersion(version) {
  return version.includes("-") ? "next" : "latest";
}

function manualInstallCommand(args) {
  return `npm ${args.join(" ")}`;
}

function canUseGlobalNpm(spawn = spawnSync) {
  const version = spawn("npm", ["--version"], { encoding: "utf8", stdio: "pipe" });
  if (version.error || version.status !== 0) {
    return false;
  }
  const prefix = spawn("npm", ["prefix", "-g"], { encoding: "utf8", stdio: "pipe" });
  return !prefix.error && prefix.status === 0 && Boolean(String(prefix.stdout || "").trim());
}

function installVersion(version, spawn = spawnSync) {
  const args = buildInstallArgs(version);
  if (!canUseGlobalNpm(spawn)) {
    console.error(`codetrail: unable to confirm npm global install environment. Run manually: ${manualInstallCommand(args)}`);
    return 1;
  }
  const result = spawn("npm", args, { stdio: "inherit" });
  if (result.error) {
    console.error(`codetrail: npm is unavailable. Run manually: ${manualInstallCommand(args)}`);
    return 1;
  }
  return result.status ?? 1;
}

module.exports = {
  shouldSkipUpdateCheck,
  updateCachePath,
  readCache,
  writeCache,
  compareVersions,
  fetchLatestVersion,
  maybePrintUpdateNotice,
  buildInstallArgs,
  canUseGlobalNpm,
  packageTagForVersion,
  installVersion
};
