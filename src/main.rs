mod cli;
mod config;
mod hub;
mod macos_mem;
mod model;
mod power;
mod record;
mod report;
mod sampler;
mod storage;
mod theme;
mod tui;

use clap::Parser;
use cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        None => hub::run_hub(&cli::HubArgs::default()),
        Some(Command::Hub(a)) => hub::run_hub(&a),
        Some(Command::Record(a)) => tui::run_record(&a),
        Some(Command::Daemon(a)) => record::run_daemon(&a).map_err(std::io::Error::other),
        Some(Command::Inspect(a)) => tui::run_inspect(&a),
        Some(Command::Report(a)) => report::run_report(&a).map_err(std::io::Error::other),
        Some(Command::Events(a)) => report::run_events(&a).map_err(std::io::Error::other),
        Some(Command::Sessions(a)) => report::run_sessions(&a).map_err(std::io::Error::other),
    };
    if let Err(e) = result {
        eprintln!("错误: {e}");
        std::process::exit(1);
    }
}
