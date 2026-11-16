use std::{path::PathBuf, process::ExitCode, sync::Arc};

use clap::{Parser, Subcommand};
use pop_server::{
    AppState,
    assets::Embedded,
    config::{Config, LogFormat},
    db, router,
};
use tracing_subscriber::EnvFilter;

/// POP, the Print Observation Platform.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The configuration file. Defaults to pop.toml, if it exists.
    #[arg(long, global = true, env = "POP_CONFIG")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (what `pop` alone does).
    Serve,
    /// Work with the configuration.
    #[command(subcommand)]
    Config(ConfigCommand),
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Check the configuration, with the environment's overrides, without
    /// starting the server.
    Check,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = match Config::load(cli.config.as_deref(), |k| std::env::var(k).ok()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pop: configuration: {e}");
            return ExitCode::from(2);
        }
    };
    match cli.command.unwrap_or(Command::Serve) {
        Command::Config(ConfigCommand::Check) => {
            println!("The configuration is valid.");
            ExitCode::SUCCESS
        }
        Command::Serve => {
            init_logs(&config);
            match serve(config).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    tracing::error!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn init_logs(config: &Config) {
    let filter = EnvFilter::new(config.log.level.as_str());
    let logs = tracing_subscriber::fmt().with_env_filter(filter);
    match config.log.format {
        LogFormat::Text => logs.init(),
        LogFormat::Json => logs.json().init(),
    }
}

async fn serve(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = &config.storage.data_dir;
    tokio::fs::create_dir_all(data_dir).await.map_err(|e| {
        format!(
            "cannot create the data directory {}: {e}",
            data_dir.display()
        )
    })?;
    let pool = db::open(&config.database_path()).await.map_err(|e| {
        format!(
            "cannot open the database {}: {e}",
            config.database_path().display()
        )
    })?;
    if !Embedded::present() {
        tracing::warn!(
            "built without the web app: run tools/web/build.sh, then build the server again"
        );
    }
    let state = AppState::new(pool, Arc::new(Embedded));
    let events = state.events.clone();
    let app = router(state);
    let listen = config.server.listen;
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| format!("cannot listen on {listen}: {e}"))?;
    tracing::info!(
        "POP {} listening on http://{listen}",
        env!("CARGO_PKG_VERSION")
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown().await;
            // Live streams never end on their own; end them, or the
            // graceful shutdown waits on them forever.
            events.close();
        })
        .await?;
    Ok(())
}

/// Ctrl-C, or SIGTERM from a supervisor.
async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = term => {},
    }
    tracing::info!("shutting down");
}
