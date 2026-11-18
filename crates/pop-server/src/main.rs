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
    // Backed up first when this POP is newer than the database (#147).
    let (pool, _backed_up) = db::open_upgrading(data_dir).await.map_err(|e| {
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
    // What the updater did, into the audit log (#148).
    pop_server::updater::spawn(
        pool.clone(),
        data_dir.clone(),
        std::time::Duration::from_secs(30),
    );
    let mut state = AppState::new(pool, Arc::new(Embedded))
        .with_access(config.access.clone())
        .with_secrets(pop_server::secrets::Secrets::new(&config.secrets.key))
        .with_public_url(&config.server.public_url)
        .with_files_dir(config.storage.data_dir.join(pop_server::backup::FILES_DIR))
        .with_guides(config.guides.clone())
        .with_public(config.public.clone());
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
        // One store of pins, for the printers and their cameras alike.
        let pins: Arc<dyn pop_printers::bambu::PinStore> = Arc::new(pins);
        let (cameras_tx, wanted) = tokio::sync::watch::channel(pop_server::cameras::Wanted::new());
        let mut cameras = None;
        if !config.go2rtc.url.is_empty() {
            let g = &config.go2rtc;
            let auth = (!g.username.is_empty()).then(|| (g.username.clone(), g.password.clone()));
            if auth.is_none() {
                tracing::warn!(
                    "go2rtc: no user name and password; anything that reaches go2rtc's API can change its streams (docs/integrations/go2rtc.md)"
                );
            }
            let client = pop_integrations::go2rtc::Go2rtc::new(&g.url, auth)
                .map_err(|e| format!("go2rtc: {e}"))?;
            // A password go2rtc does not ask for protects nothing (#149).
            let probe = client.clone();
            tokio::spawn(async move {
                if probe.has_password()
                    && matches!(probe.answers_without_password().await, Ok(true))
                {
                    tracing::warn!(
                        "go2rtc answers requests with no password: turn on local_auth in its api section (docs/integrations/go2rtc.md)"
                    );
                }
            });
            // go2rtc serves the tower printers' cameras, kept as BAM! has
            // them (#66).
            let (kept, _keeper) = pop_server::cameras::spawn(
                client.clone(),
                wanted,
                pop_server::cameras::Options::default(),
                pins.clone(),
            );
            state = state.with_cameras(kept, client);
            cameras = Some(cameras_tx);
        }
        let (tower, _runner) = pop_server::run::spawn(pop_server::run::Runner {
            db: state.db.clone(),
            secrets: state.secrets.clone(),
            fields: config.bam.fields.clone(),
            view,
            events: state.events.clone(),
            decided: state.decided.clone(),
            pins,
            backoff: pop_printers::fleet::Backoff::default(),
            cameras,
        });
        state = state.with_tower(tower);
    }
    if config.bam.url.is_empty() && !config.go2rtc.url.is_empty() {
        tracing::warn!("go2rtc is set, but the cameras come from BAM!: set [bam] to show any");
    }
    // Feedback from any screen, filed as GitHub issues (#96).
    if !config.feedback.app_id.is_empty() {
        let f = &config.feedback;
        let key = std::fs::read_to_string(&f.private_key_file).map_err(|e| {
            format!(
                "feedback.private_key_file: cannot read {}: {e}",
                f.private_key_file
            )
        })?;
        let (owner, repo) = f.repo.split_once('/').unwrap_or_default();
        let github = pop_integrations::github::GitHub::new(pop_integrations::github::AppConfig {
            api: f.api.clone(),
            app_id: f.app_id.clone(),
            installation_id: f.installation_id.clone(),
            private_key: key,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
        })
        .map_err(|e| format!("feedback: {e}"))?;
        let feedback = Arc::new(pop_server::feedback::Feedback {
            config: f.clone(),
            github,
            wake: Arc::new(tokio::sync::Notify::new()),
        });
        let _sender = pop_server::feedback::spawn(
            state.db.clone(),
            feedback.clone(),
            pop_server::feedback::Options::default(),
        );
        state = state.with_feedback(feedback);
    }
    // Group notices (#68), to the Discord webhook as saved at each one.
    let (notifier, _delivery) = pop_server::notify::spawn(
        state.db.clone(),
        state.secrets.clone(),
        state.http.clone(),
        pop_server::notify::Options::default(),
    );
    // The group hears when a printer needs a hand, and when it is fixed (#69).
    // And its screen shows what the group was told (#55).
    let (told, _notices) =
        pop_server::notices::spawn(state.tower.clone(), notifier.clone(), state.events.clone());
    state = state.with_notifier(notifier).with_told(told);
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
