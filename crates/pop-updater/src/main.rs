//! `pop-updater [--config /etc/pop/updater.toml]`: one run of automatic
//! upgrades (#148), started by pop-updater.timer. docs/upgrades.md.

use std::{path::PathBuf, process::ExitCode};

use pop_updater::{Config, Outcome, SystemHost, run};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut path = PathBuf::from("/etc/pop/updater.toml");
    while let Some(a) = args.next() {
        match (a.as_str(), args.next()) {
            ("--config", Some(p)) => path = p.into(),
            ("--version", _) => {
                println!("pop-updater {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("usage: pop-updater [--config /etc/pop/updater.toml]");
                return ExitCode::from(2);
            }
        }
    }
    let cfg = match Config::load(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let host = SystemHost {
        service: cfg.service.clone(),
        user: cfg.user.clone(),
        pop_config: cfg.pop_config.clone(),
    };
    // A failure that needs a person shows in `systemctl status`; a refused or
    // rolled-back release was handled, and says so in the audit log.
    match run(&cfg, &host).await {
        Outcome::Failed => ExitCode::FAILURE,
        _ => ExitCode::SUCCESS,
    }
}
