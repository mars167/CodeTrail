use clap::{ArgAction, Parser, Subcommand, ValueEnum};

use crate::{
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
    #[command(alias = "ls")]
    List {
        dir: Option<String>,
        #[arg(long)]
        recursive: bool,
    },
    Tree {
        dir: Option<String>,
        #[arg(long)]
        depth: Option<u8>,
    },
    Read {
        target: String,
    },
    Refs {
        identifier: String,
    },
    Symbols {
        query: String,
    },
    Defs {
        identifier: String,
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
    Hooks {
        #[command(subcommand)]
        command: HooksCommand,
    },
    Completions {
        #[arg(value_enum)]
        shell: CompletionShell,
    },
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
        /// Skip the best-effort LSP semantic / SCIP generation phase.
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
