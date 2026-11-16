use std::{process::ExitCode, sync::Arc};

use pop_server::{AppState, assets::Embedded, config::Config, db, router};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    tokio::fs::create_dir_all(&config.data_dir)
        .await
        .map_err(|e| {
            format!(
                "cannot create the data directory {}: {e}",
                config.data_dir.display()
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
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|e| format!("cannot listen on {}: {e}", config.listen))?;
    tracing::info!(
        "POP {} listening on http://{}",
        env!("CARGO_PKG_VERSION"),
        config.listen
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
