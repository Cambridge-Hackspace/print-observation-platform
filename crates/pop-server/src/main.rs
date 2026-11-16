use std::{path::PathBuf, process::ExitCode, sync::Arc};

use clap::{Parser, Subcommand};
use pop_server::{
    AppState,
    api::Role,
    assets::Embedded,
    backup,
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
    /// Write the database and the stored print files to one archive. Safe
    /// while the server is running.
    Backup {
        /// The archive to write. It must not exist yet.
        file: PathBuf,
    },
    /// Restore a backup into the configured data directory, which must not
    /// already hold a database.
    Restore {
        /// The archive `pop backup` wrote.
        file: PathBuf,
    },
    /// Work with accounts.
    #[command(subcommand)]
    User(UserCommand),
}

#[derive(Subcommand)]
enum UserCommand {
    /// Create an admin: the first one, or another when no admin can sign in.
    /// Asks for the password, or reads it from standard input in a script.
    CreateAdmin {
        username: String,
        /// The name shown for them. Defaults to the username.
        #[arg(long)]
        name: Option<String>,
    },
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
        Command::Backup { file } => match backup::backup(&config.storage.data_dir, &file).await {
            Ok(m) => {
                println!(
                    "Backed up {} file(s), schema {}, to {}.",
                    m.files.len(),
                    m.schema_version,
                    file.display()
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("pop: backup: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Restore { file } => match backup::restore(&file, &config.storage.data_dir).await {
            Ok(m) => {
                println!(
                    "Restored {} file(s), schema {}, made by POP {}, into {}.",
                    m.files.len(),
                    m.schema_version,
                    m.pop_version,
                    config.storage.data_dir.display()
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("pop: restore: {e}");
                ExitCode::FAILURE
            }
        },
        Command::User(UserCommand::CreateAdmin { username, name }) => {
            match create_admin(&config, &username, name.as_deref()).await {
                Ok(()) => {
                    println!("Created {username}, an admin. They can sign in now.");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("pop: user create-admin: {e}");
                    ExitCode::FAILURE
                }
            }
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

/// The password from a person at a terminal, asked twice and not echoed, or
/// one line from standard input in a script.
fn read_password() -> Result<String, String> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        let a = rpassword::prompt_password("Password: ").map_err(|e| e.to_string())?;
        let b = rpassword::prompt_password("Again: ").map_err(|e| e.to_string())?;
        if a != b {
            return Err("the two passwords differ".into());
        }
        Ok(a)
    } else {
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }
}

async fn create_admin(config: &Config, username: &str, name: Option<&str>) -> Result<(), String> {
    let password = read_password()?;
    tokio::fs::create_dir_all(&config.storage.data_dir)
        .await
        .map_err(|e| e.to_string())?;
    let db = db::open(&config.database_path())
        .await
        .map_err(|e| format!("cannot open the database: {e}"))?;
    let account = pop_server::users::NewAccount {
        username,
        name: name.unwrap_or(username),
        role: Role::Admin,
        password: &password,
    };
    pop_server::users::create(&db, account, &pop_server::audit::Actor::command_line())
        .await
        .map(|_| ())
        .map_err(|e| match e {
            pop_server::users::UserError::Invalid { field, reason } => format!("{field}: {reason}"),
            pop_server::users::UserError::UsernameTaken => {
                format!("{username} already exists; an admin can reset their password")
            }
            other => format!("{other:?}"),
        })
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
    let mut state = AppState::new(pool, Arc::new(Embedded))
        .with_access(config.access.clone())
        .with_secrets(pop_server::secrets::Secrets::new(&config.secrets.key))
        .with_public_url(&config.server.public_url);
    if !config.bam.url.is_empty() {
        let client = pop_integrations::bam::Bam::new(&config.bam.url, &config.bam.token)
            .map_err(|e| format!("BAM!: {e}"))?;
        // Kept in sync with BAM!'s live connection, re-read on every
        // reconnect and every five minutes (#22).
        let (view, _sync) = pop_integrations::sync::spawn(
            client.clone(),
            config.bam.category.clone(),
            pop_integrations::sync::Options::default(),
        );
        state = state
            .with_bam(pop_server::printers::BamSource {
                client,
                category: config.bam.category.clone(),
                fields: config.bam.fields.clone(),
            })
            .with_bam_sync(view.clone());
        // Printers archived in BAM! leave the tower as they go (#23).
        let _retirer = pop_server::tower::spawn_retirer(state.db.clone(), view.clone());
        // And the tower's printers run, and every screen sees them (#49).
        let pins = pop_server::pins::DbPins::load(state.db.clone())
            .await
            .map_err(|e| format!("cannot read the Bambu certificate pins: {e}"))?;
        let (tower, _runner) = pop_server::run::spawn(pop_server::run::Runner {
            db: state.db.clone(),
            secrets: state.secrets.clone(),
            fields: config.bam.fields.clone(),
            view,
            events: state.events.clone(),
            decided: state.decided.clone(),
            pins: Arc::new(pins),
            backoff: pop_printers::fleet::Backoff::default(),
        });
        state = state.with_tower(tower);
    }
    // Group notices (#68), to the Discord webhook as saved at each one.
    let (notifier, _notices) = pop_server::notify::spawn(
        state.db.clone(),
        state.secrets.clone(),
        state.http.clone(),
        pop_server::notify::Options::default(),
    );
    state = state.with_notifier(notifier);
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
    // With each connection's peer address, which decides what is local (#7).
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
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
