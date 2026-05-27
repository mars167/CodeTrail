use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "code-search")]
#[command(version)]
#[command(about = "Deterministic-first code search with reliability-labeled evidence")]
struct Cli {
    #[arg(short, long, default_value = ".")]
    path: String,

    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,

    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Debug, ValueEnum)]
enum OutputFormat {
    Json,
    Text,
}

#[derive(Debug, Subcommand)]
enum Command {
    Find {
        text: String,
        #[arg(long, default_value = "literal")]
        mode: String,
    },
    Grep {
        pattern: String,
        #[arg(long, default_value = "regex")]
        mode: String,
        #[arg(long, default_value_t = 0)]
        context: u16,
    },
    Files {
        pattern: String,
    },
    #[command(alias = "findpath", alias = "path")]
    FindPath {
        pattern: String,
    },
    Glob {
        pattern: String,
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
    Calls {
        identifier: String,
    },
    Callers {
        identifier: String,
    },
    Changed,
    Status,
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
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    Hooks {
        #[command(subcommand)]
        command: HooksCommand,
    },
}

#[derive(Debug, Subcommand)]
enum IndexCommand {
    Build {
        #[arg(long)]
        staged: bool,
        #[arg(long)]
        changed: bool,
        #[arg(long)]
        force: bool,
    },
    Update,
    Status,
    Verify,
    Clean,
}

#[derive(Debug, Subcommand)]
enum HooksCommand {
    Install,
    Uninstall,
    Status,
}

fn main() {
    let cli = Cli::parse();

    eprintln!(
        "design scaffold only: command={:?}, path={}, output={:?}",
        cli.command, cli.path, cli.output
    );
}
