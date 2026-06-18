const { spawn } = require("node:child_process");

function runCore(binaryPath, args, options = {}) {
  const child = spawn(binaryPath, args, {
    stdio: "inherit",
    env: process.env,
    ...options
  });

  child.on("exit", (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code ?? 1);
  });

  child.on("error", (error) => {
    console.error(`codetrail: failed to start core binary: ${error.message}`);
    process.exit(1);
  });
}

module.exports = { runCore };
