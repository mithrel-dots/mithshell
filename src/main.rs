use std::process::ExitCode;

use clap::Parser;
use mithshell::{app, cli::Cli};

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("mithshell=info"))
        .format_timestamp_millis()
        .init();

    match app::run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mithshell: {error:#}");
            ExitCode::FAILURE
        }
    }
}
