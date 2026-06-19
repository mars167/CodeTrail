function optionValue(args, name) {
  const index = args.indexOf(name);
  if (index >= 0 && args[index + 1]) return args[index + 1];
  const inline = args.find((arg) => arg.startsWith(`${name}=`));
  if (inline) return inline.slice(name.length + 1);
  return null;
}

function outputMode(args) {
  return optionValue(args, "--output") || "text";
}

function isJsonOutput(args) {
  return ["json", "jsonl", "compact-json"].includes(outputMode(args));
}

function commandName(args) {
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--output" || arg === "--path" || arg === "--include" || arg === "--exclude") {
      index += 1;
      continue;
    }
    if (!arg.startsWith("-")) {
      return arg;
    }
  }
  return "";
}

module.exports = {
  outputMode,
  isJsonOutput,
  commandName
};
