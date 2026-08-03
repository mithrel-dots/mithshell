use std::{env, process::ExitCode};

use clap::Parser;
use mithshell::{app, cli::Cli};

fn main() -> ExitCode {
    select_default_renderer();

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

/// Defaults GSK to the cairo renderer unless the user asked for another one.
///
/// The island is a small, mostly static 2D surface, so GPU compositing buys
/// very little while pulling the whole driver stack into a process that runs
/// for the entire session. On this machine the GL renderer maps about 113MB of
/// NVIDIA libraries; cairo maps none, cutting RSS from 224MB to 85MB and
/// private dirty pages from 53MB to 25MB, at a cost of roughly 2ms of paint
/// time per frame and no measurable idle CPU.
///
/// Override by exporting GSK_RENDERER, for example `GSK_RENDERER=ngl`.
fn select_default_renderer() {
    if env::var_os("GSK_RENDERER").is_some() {
        return;
    }
    // SAFETY: called at the top of `main` before any thread is spawned and
    // before GTK reads the environment, so there is no concurrent access.
    unsafe {
        env::set_var("GSK_RENDERER", "cairo");
    }
}
