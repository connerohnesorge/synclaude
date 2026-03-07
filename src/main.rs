mod config;
mod sync;
mod watcher;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;

#[derive(Parser)]
#[command(name = "synclaude", about = "Synchronize ~/.claude/ across NixOS machines via git")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize synclaude with a remote git repo
    Init {
        /// Git remote URL (SSH or HTTPS)
        repo_url: String,

        /// Optional machine name override (defaults to /etc/machine-id)
        #[arg(long)]
        machine_name: Option<String>,
    },

    /// Push local changes to the remote
    Push,

    /// Pull and merge remote changes
    Pull,

    /// Run the background daemon (file watcher + periodic pull)
    Daemon,

    /// Show current configuration and status
    Status,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Init {
            repo_url,
            machine_name,
        } => cmd_init(repo_url, machine_name),
        Commands::Push => cmd_push(),
        Commands::Pull => cmd_pull(),
        Commands::Daemon => cmd_daemon(),
        Commands::Status => cmd_status(),
    }
}

fn cmd_init(repo_url: String, machine_name: Option<String>) -> Result<()> {
    let mut cfg = config::Config::default();
    cfg.remote_url = repo_url;

    if let Some(name) = machine_name {
        cfg.machine_id = name;
    }

    // Save config
    cfg.save()?;
    info!("Config saved to {}", config::Config::config_path()?.display());

    // Clone/init the repo
    sync::init_repo(&cfg)?;

    info!(
        "Initialized synclaude. Machine branch: {}",
        cfg.branch_name()
    );
    info!("Syncing directories: {:?}", cfg.sync_dirs);

    Ok(())
}

fn cmd_push() -> Result<()> {
    let config = config::Config::load()?;

    sync::stage_changes(&config)?;

    let message = format!("synclaude: manual push from {}", config.machine_id);
    sync::commit_and_push(&config, &message)?;

    info!("Push complete");
    Ok(())
}

fn cmd_pull() -> Result<()> {
    let config = config::Config::load()?;

    sync::pull_and_merge(&config)?;
    sync::apply_pulled_changes(&config)?;

    info!("Pull complete");
    Ok(())
}

fn cmd_daemon() -> Result<()> {
    let config = config::Config::load()?;

    info!(
        "Starting synclaude daemon for machine {}",
        config.machine_id
    );

    // Spawn periodic pull in background
    let pull_config = config.clone();
    let pull_interval = std::time::Duration::from_secs(config.pull_interval_secs);

    std::thread::spawn(move || loop {
        std::thread::sleep(pull_interval);
        info!("Periodic pull...");
        if let Err(e) = sync::pull_and_merge(&pull_config) {
            tracing::error!("Periodic pull failed: {}", e);
        }
        if let Err(e) = sync::apply_pulled_changes(&pull_config) {
            tracing::error!("Applying pulled changes failed: {}", e);
        }
    });

    // Run file watcher on main thread (blocking)
    watcher::watch_and_sync(&config)?;

    Ok(())
}

fn cmd_status() -> Result<()> {
    let config = config::Config::load()?;

    println!("synclaude status");
    println!("  Machine ID:     {}", config.machine_id);
    println!("  Branch:         {}", config.branch_name());
    println!("  Remote:         {}", config.remote_url);
    println!("  Local repo:     {}", config.local_repo_path.display());
    println!("  Sync dirs:      {:?}", config.sync_dirs);
    println!("  Pull interval:  {}s", config.pull_interval_secs);

    let claude_dir = config::Config::claude_dir()?;
    println!("  Claude dir:     {}", claude_dir.display());

    for dir in &config.sync_dirs {
        let path = claude_dir.join(dir);
        let exists = path.exists();
        println!(
            "    {}: {}",
            dir,
            if exists { "exists" } else { "missing" }
        );
    }

    Ok(())
}
