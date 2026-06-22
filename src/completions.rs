use crate::cli::CompletionShell;

const COMMANDS: &[&str] = &[
    "refs",
    "symbols",
    "defs",
    "calls",
    "callers",
    "index",
    "completions",
];

const INDEX_COMMANDS: &[&str] = &["build", "status", "doctor"];

pub fn script(shell: &CompletionShell) -> String {
    match shell {
        CompletionShell::Bash => bash(),
        CompletionShell::Zsh => zsh(),
        CompletionShell::Fish => fish(),
    }
}

fn bash() -> String {
    let commands = COMMANDS.join(" ");
    let index_commands = INDEX_COMMANDS.join(" ");
    format!(
        r#"_codetrail()
{{
  local cur prev commands index_cmds shells
  COMPREPLY=()
  cur="${{COMP_WORDS[COMP_CWORD]}}"
  prev="${{COMP_WORDS[COMP_CWORD-1]}}"
  commands="{commands}"
  index_cmds="{index_commands}"
  shells="bash zsh fish"

  case "$prev" in
    index)
      COMPREPLY=( $(compgen -W "$index_cmds" -- "$cur") )
      return 0
      ;;
    completions)
      COMPREPLY=( $(compgen -W "$shells" -- "$cur") )
      return 0
      ;;
  esac

  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "--path --output --include --exclude --hidden --no-ignore --lang --dir --ext --file-pattern --file-mode --case-sensitive --ignore-case --input-mode --changed --cursor --allow-broad --limit --context --include-code --code-context --code-max-lines --save-query --help --version" -- "$cur") )
  else
    COMPREPLY=( $(compgen -W "$commands" -- "$cur") )
  fi
}}
complete -F _codetrail codetrail
"#
    )
}

fn zsh() -> String {
    let commands = COMMANDS.join(" ");
    let index_commands = INDEX_COMMANDS.join(" ");
    format!(
        r#"#compdef codetrail

_codetrail() {{
  local -a commands index_cmds shells global_opts
  commands=({commands})
  index_cmds=({index_commands})
  shells=(bash zsh fish)
  global_opts=(--path --output --include --exclude --hidden --no-ignore --lang --dir --ext --file-pattern --file-mode --case-sensitive --ignore-case --input-mode --changed --cursor --allow-broad --limit --context --include-code --code-context --code-max-lines --save-query --help --version)

  if [[ "$words[CURRENT]" == -* ]]; then
    _describe 'option' global_opts
    return
  fi

  if (( CURRENT == 2 )); then
    _describe 'command' commands
    return
  fi

  case $words[2] in
    index)
      _describe 'index command' index_cmds
      ;;
    completions)
      _describe 'shell' shells
      ;;
    *)
      _files
      ;;
  esac
}}

_codetrail "$@"
"#
    )
}

fn fish() -> String {
    let index_commands = INDEX_COMMANDS.join(" ");
    let mut lines = vec![
        "complete -c codetrail -f".to_string(),
        "complete -c codetrail -l path -r".to_string(),
        "complete -c codetrail -l output -xa 'json compact-json jsonl text'".to_string(),
        "complete -c codetrail -l include -r".to_string(),
        "complete -c codetrail -l exclude -r".to_string(),
        "complete -c codetrail -l hidden".to_string(),
        "complete -c codetrail -l no-ignore".to_string(),
        "complete -c codetrail -l lang -r".to_string(),
        "complete -c codetrail -l changed".to_string(),
        "complete -c codetrail -l cursor -r".to_string(),
        "complete -c codetrail -l allow-broad".to_string(),
        "complete -c codetrail -l limit -r".to_string(),
        "complete -c codetrail -l context -r".to_string(),
        "complete -c codetrail -l include-code".to_string(),
        "complete -c codetrail -l code-context -r".to_string(),
        "complete -c codetrail -l code-max-lines -r".to_string(),
        "complete -c codetrail -l save-query -r".to_string(),
    ];
    for command in COMMANDS {
        lines.push(format!("complete -c codetrail -f -a {command}"));
    }
    lines.push(format!(
        "complete -c codetrail -n '__fish_seen_subcommand_from index' -a '{index_commands}'"
    ));
    lines.push(
        "complete -c codetrail -n '__fish_seen_subcommand_from completions' -a 'bash zsh fish'"
            .to_string(),
    );
    lines.push(String::new());
    lines.join("\n")
}
