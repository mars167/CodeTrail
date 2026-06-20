use clap::{ArgAction, Parser, Subcommand, ValueEnum};

use crate::{
    code_context::MAX_CODE_MAX_LINES,
    query_input::InputMode,
    search_pattern::{ContentPatternMode, SearchPatternMode},
};

#[derive(Debug, Parser)]
#[command(name = "codetrail")]
#[command(version)]
#[command(about = "CodeTrail: deterministic-first code search with reliability-labeled evidence")]
pub struct Cli {
    #[arg(short, long, default_value = ".")]
    pub path: String,

    #[arg(short = 'v', long, global = true, action = ArgAction::Count)]
    pub verbose: u8,

    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub output: OutputFormat,

    #[arg(long, global = true)]
    pub include: Vec<String>,

    #[arg(long, global = true)]
    pub exclude: Vec<String>,

    #[arg(long, global = true)]
    pub hidden: bool,

    #[arg(long, global = true)]
    pub no_ignore: bool,

    #[arg(long, global = true)]
    pub lang: Vec<String>,

    #[arg(
        long,
        global = true,
        help = "Workspace-relative directory scope; repeat for OR"
    )]
    pub dir: Vec<String>,

    #[arg(
        long,
        global = true,
        help = "File extension scope, with or without leading dot; repeat for OR"
    )]
    pub ext: Vec<String>,

    #[arg(
        long,
        global = true,
        help = "Path pattern scope applied before content/symbol search; repeat for OR"
    )]
    pub file_pattern: Vec<String>,

    #[arg(long, global = true, value_enum, default_value_t = SearchPatternMode::Wildcard, help = "Pattern mode for --file-pattern")]
    pub file_mode: SearchPatternMode,

    #[arg(
        long,
        global = true,
        conflicts_with = "ignore_case",
        help = "Match text, paths, and compatible symbol input with exact case"
    )]
    pub case_sensitive: bool,

    #[arg(
        long,
        global = true,
        help = "Match text, paths, and compatible symbol input ignoring case (default)"
    )]
    pub ignore_case: bool,

    #[arg(long, global = true, value_enum, default_value_t = InputMode::Compatible, help = "Symbol input handling for defs/refs/symbols/calls/callers")]
    pub input_mode: InputMode,

    #[arg(long, global = true)]
    pub changed: bool,

    #[arg(long, global = true)]
    pub cursor: Option<String>,

    #[arg(long, global = true)]
    pub allow_broad: bool,

    #[arg(long, global = true, default_value_t = 100)]
    pub limit: usize,

    #[arg(long, global = true, default_value_t = 0)]
    pub context: u16,

    #[arg(long, global = true)]
    pub save_query: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Json,
    CompactJson,
    Jsonl,
    Text,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Find {
        text: String,
        #[arg(long, value_enum, default_value_t = ContentPatternMode::Literal, help = "Content match mode")]
        mode: ContentPatternMode,
    },
    Grep {
        pattern: String,
        #[arg(long, value_enum, default_value_t = ContentPatternMode::Regex, help = "Content match mode")]
        mode: ContentPatternMode,
        #[arg(long)]
        context: Option<u16>,
    },
    Files {
        pattern: String,
        #[arg(long, value_enum, default_value_t = SearchPatternMode::Literal, help = "Path match mode")]
        mode: SearchPatternMode,
    },
    #[command(alias = "findpath", alias = "path")]
    FindPath {
        pattern: String,
        #[arg(long, value_enum, default_value_t = SearchPatternMode::Literal, help = "Path match mode")]
        mode: SearchPatternMode,
    },
    Glob {
        pattern: String,
        #[arg(long, value_enum, default_value_t = SearchPatternMode::Glob, help = "Path match mode")]
        mode: SearchPatternMode,
    },
    Refs {
        identifier: String,
    },
    Symbols {
        query: String,
        #[arg(long)]
        include_code: bool,
        #[arg(long, requires = "include_code", value_parser = clap::value_parser!(u16), help = "Lines around a symbol occurrence when body range is unavailable")]
        code_context: Option<u16>,
        #[arg(long, requires = "include_code", value_parser = parse_code_max_lines, help = "Maximum source lines returned per result")]
        code_max_lines: Option<usize>,
    },
    Defs {
        identifier: String,
        #[arg(long)]
        include_code: bool,
        #[arg(long, requires = "include_code", value_parser = clap::value_parser!(u16), help = "Lines around a symbol occurrence when body range is unavailable")]
        code_context: Option<u16>,
        #[arg(long, requires = "include_code", value_parser = parse_code_max_lines, help = "Maximum source lines returned per result")]
        code_max_lines: Option<usize>,
    },
    Routes {
        pattern: Option<String>,
        #[arg(long)]
        framework: Vec<String>,
        #[arg(long)]
        method: Vec<String>,
    },
    Calls {
        identifier: String,
    },
    Callers {
        identifier: String,
    },
    Changed,
    Status,
    Mcp,
    Watch {
        #[arg(long)]
        once: bool,
        #[arg(long)]
        status: bool,
    },
    Serve {
        #[arg(long)]
        no_watch: bool,
    },
    Query {
        #[command(subcommand)]
        command: QueryCommand,
    },
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    IndexProvider {
        #[command(subcommand)]
        command: IndexProviderCommand,
    },
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    Hooks {
        #[command(subcommand)]
        command: HooksCommand,
    },
    Completions {
        #[arg(value_enum)]
        shell: CompletionShell,
    },
}

fn parse_code_max_lines(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| "must be an integer".to_string())?;
    if !(1..=MAX_CODE_MAX_LINES).contains(&parsed) {
        return Err(format!("must be between 1 and {MAX_CODE_MAX_LINES}"));
    }
    Ok(parsed)
}

#[derive(Debug, Subcommand)]
pub enum QueryCommand {
    Replay {
        name: String,
        #[arg(long, value_enum, default_value_t = ReplaySnapshot::Current)]
        snapshot: ReplaySnapshot,
    },
    Show {
        name: String,
    },
    List,
    Delete {
        name: String,
    },
}

#[derive(Clone, Debug, ValueEnum)]
pub enum ReplaySnapshot {
    Current,
    Saved,
}

#[derive(Debug, Subcommand)]
pub enum IndexCommand {
    Build {
        #[arg(long)]
        staged: bool,
        #[arg(long)]
        changed: bool,
        #[arg(long)]
        force: bool,
        /// Skip the best-effort semantic provider / SCIP generation phase.
        #[arg(long)]
        no_semantic: bool,
    },
    Update,
    Status,
    Skipped {
        #[arg(long)]
        staged: bool,
    },
    Verify,
    Clean,
    Pack {
        #[arg(long, default_value = "output.tar.gz")]
        output: String,
    },
    Unpack {
        path: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum IndexProviderCommand {
    Install {
        #[arg(value_name = "LANGUAGE")]
        languages: Vec<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum SkillCommand {
    Install {
        #[arg(value_name = "TARGET")]
        target: Option<String>,
        #[arg(long, value_enum, default_value_t = SkillScope::User)]
        scope: SkillScope,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum SkillScope {
    User,
    Project,
}

#[derive(Debug, Subcommand)]
pub enum HooksCommand {
    Install,
    Uninstall,
    Status,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}
