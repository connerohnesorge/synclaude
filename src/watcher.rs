use anyhow::Result;
use notify::{Event, RecursiveMode, Watcher};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info};

use crate::config::Config;
use crate::sync;

/// Start watching the configured sync directories for changes.
/// When changes are detected (debounced), stage + commit + push.
pub fn watch_and_sync(config: &Config) -> Result<()> {
    let (tx, rx) = mpsc::channel();

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        match res {
            Ok(event) => {
                debug!("File event: {:?}", event);
                let _ = tx.send(());
            }
            Err(e) => error!("Watch error: {}", e),
        }
    })?;

    let paths = config.sync_source_paths()?;
    for path in &paths {
        if path.exists() {
            info!("Watching {}", path.display());
            watcher.watch(path, RecursiveMode::Recursive)?;
        } else {
            info!(
                "Sync dir {} does not exist yet, will watch when created",
                path.display()
            );
        }
    }

    info!("File watcher started. Waiting for changes...");

    // Debounce: wait for events, then batch process after a quiet period
    let debounce_duration = Duration::from_secs(5);

    loop {
        // Block until we get at least one event
        rx.recv().map_err(|e| anyhow::anyhow!("Watch channel closed: {}", e))?;

        // Drain any additional events within the debounce window
        while rx.recv_timeout(debounce_duration).is_ok() {}

        info!("Changes detected, syncing...");
        if let Err(e) = do_push_sync(config) {
            error!("Sync failed: {}", e);
        }
    }
}

fn do_push_sync(config: &Config) -> Result<()> {
    sync::stage_changes(config)?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let message = format!(
        "synclaude: auto-sync from {} at {}",
        config.machine_id, timestamp
    );
    sync::commit_and_push(config, &message)?;

    Ok(())
}
