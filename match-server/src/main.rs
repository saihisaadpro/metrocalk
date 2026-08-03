//! Implement this feature in accordance with the Engine UI/UX Architecture Constitution.
//!
//! Thin candidate CLI. It exposes immutable build metadata and one max-speed deterministic smoke command;
//! there is intentionally no listener or daemon mode in MOB-2.

use std::env;
use std::process::ExitCode;

use metrocalk_match_server::{HostConfig, MatchHost, VERSION_JSON};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let command = args.next();
    if args.next().is_some() {
        return usage_error();
    }

    match command.as_deref() {
        Some("--version-json") => {
            println!("{VERSION_JSON}");
            ExitCode::SUCCESS
        }
        Some("--smoke-json") => run_smoke(),
        Some("--help" | "-h") => {
            println!("metrocalk-match-server --version-json | --smoke-json");
            ExitCode::SUCCESS
        }
        _ => usage_error(),
    }
}

fn run_smoke() -> ExitCode {
    match MatchHost::new(HostConfig::default()).and_then(|mut host| host.run_reference_smoke()) {
        Ok(report) => {
            println!("{}", report.to_json());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("reference smoke failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn usage_error() -> ExitCode {
    eprintln!("usage: metrocalk-match-server --version-json | --smoke-json");
    ExitCode::from(2)
}
