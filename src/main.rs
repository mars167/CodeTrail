mod cli;
mod commands;
mod completions;
mod index;
mod output;
mod scip_index;
mod search;
mod syntax;
mod workspace;

use anyhow::Result;
use clap::Parser;
use cli::Cli;

fn main() {
    let cli = Cli::parse();
    let output = cli.output.clone();

    let exit_code = match commands::run(cli) {
        Ok(code) => code,
        Err(error) => {
            let value = output::error_response(error);
            if output::emit(&output, &value).is_err() {
                eprintln!("failed to render error response");
            }
            1
        }
    };

    std::process::exit(exit_code);
}

pub(crate) type AppResult<T> = Result<T>;
