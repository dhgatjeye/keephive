use anyhow::{Context, Result};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::ServiceConfig;

// Channel capacity constants for bounded channels
const CONFIG_CHANGE_CHANNEL_CAPACITY: usize = 10;
const FS_EVENT_CHANNEL_CAPACITY: usize = 1000;

#[derive(Debug, Clone)]
pub struct ConfigChangeEvent {
    pub config: ServiceConfig,
}

pub struct ConfigWatcher {
    config_path: PathBuf,
    tx: mpsc::Sender<ConfigChangeEvent>,
    cancellation: CancellationToken,
}

impl ConfigWatcher {
    /// Create new config watcher
    pub fn new(
        config_path: PathBuf,
        cancellation: CancellationToken,
    ) -> Result<(Self, mpsc::Receiver<ConfigChangeEvent>)> {
        // Use bounded channel to prevent unbounded memory growth
        let (tx, rx) = mpsc::channel(CONFIG_CHANGE_CHANNEL_CAPACITY);

        Ok((
            Self {
                config_path,
                tx,
                cancellation,
            },
            rx,
        ))
    }

    /// Start watching config file
    pub async fn watch(self) -> Result<()> {
        info!("Starting config file watcher for: {}", self.config_path.display());

        let (notify_tx, mut notify_rx) = mpsc::channel(FS_EVENT_CHANNEL_CAPACITY);

        let config_path = self.config_path.clone();
        let config_path_for_watcher = self.config_path.clone();
        let tx = self.tx.clone();
        let cancellation = self.cancellation.clone();  // Clone for select

        // Create the Watcher (sync, but with closure)
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            match res {
                Ok(event) => {
                    debug!("File system event: {:?}", event);
                    let _ = notify_tx.try_send(event);  // Sync try_send
                }
                Err(e) => error!("Watch error: {:?}", e),
            }
        })?;

        let watch_path = if let Some(parent) = config_path_for_watcher.parent() { parent } else { Path::new(".") };
        info!("Watching directory: {}", watch_path.display());
        watcher.watch(watch_path, RecursiveMode::NonRecursive).context("Failed to start watching")?;

        // Make the event loop asynchronous, integrate cancellation with select
        // Keep Watcher here (keep it alive until it drops)
        loop {
            tokio::select! {
                // Event receive
                Some(event) = notify_rx.recv() => {
                    if Self::is_config_modified(&event, &config_path) {
                        info!("Config file change detected, reloading...");

                        match Self::load_config(&config_path).await {
                            Ok(config) => {
                                info!("Config loaded successfully, notifying daemon");
                                if tx.try_send(ConfigChangeEvent { config }).is_err() {
                                    warn!("Config change channel full or receiver dropped, skipping update");
                                }
                            }
                            Err(e) => warn!("Failed to reload config: {}", e),
                        }
                    }
                }

                // Wait for cancellation
                _ = cancellation.cancelled() => {
                    info!("Config watcher shutdown complete");
                    break;  // Exit the loop, watcher drop occurs
                }
            }
        }

        debug!("Config watcher event loop terminated");
        Ok(())
    }

    fn is_config_modified(event: &Event, config_path: &Path) -> bool {
        // Check if this event is related to our config file
        let is_our_file = event.paths.iter().any(|p| {
            p.file_name() == config_path.file_name()
        });

        if !is_our_file {
            return false;
        }

        // Check if it's a modification event
        matches!(
            event.kind,
            EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
        )
    }

    async fn load_config(path: &Path) -> Result<ServiceConfig> {
        let content = tokio::fs::read_to_string(path).await
            .context("Failed to read config file")?;

        let config: ServiceConfig = serde_json::from_str(&content)
            .context("Failed to parse config file")?;

        Ok(config)
    }
}