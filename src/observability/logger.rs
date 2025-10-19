use std::path::Path;
use std::sync::{Mutex, OnceLock};
use tracing_subscriber::{
    layer::SubscriberExt,
    reload,
    util::SubscriberInitExt,
    EnvFilter,
};

/// Must be kept alive for the entire application lifetime
static LOG_GUARD: OnceLock<Mutex<Option<tracing_appender::non_blocking::WorkerGuard>>> = OnceLock::new();

/// Reload handle for dynamically changing the log filter at runtime
static RELOAD_HANDLE: OnceLock<Mutex<reload::Handle<EnvFilter, tracing_subscriber::Registry>>> = OnceLock::new();

/// Log rotation strategy
#[derive(Debug, Clone, Copy)]
pub enum Rotation {
    Daily,
    Hourly,
    Never,
}

pub fn init_logging(
    level: &str,
    log_dir: Option<&Path>,
    rotation: Rotation,
) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));

    // Create a reloadable filter layer
    let (filter_layer, reload_handle) = reload::Layer::new(filter);

    // Console layer - always enabled
    let console_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true);

    // Build subscriber with reloadable filter and console layer
    let subscriber = tracing_subscriber::registry()
        .with(filter_layer)
        .with(console_layer);

    // Add file layer if log directory is specified
    if let Some(dir) = log_dir {
        // Ensure log directory exists
        std::fs::create_dir_all(dir)?;

        let file_appender = match rotation {
            Rotation::Daily => {
                tracing_appender::rolling::daily(dir, "keephive.log")
            }
            Rotation::Hourly => {
                tracing_appender::rolling::hourly(dir, "keephive.log")
            }
            Rotation::Never => {
                tracing_appender::rolling::never(dir, "keephive.log")
            }
        };

        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(non_blocking)
            .with_target(true)
            .with_thread_ids(true)
            .with_file(true)
            .with_line_number(true)
            .with_ansi(false); // No ANSI colors in file

        subscriber.with(file_layer).init();

        LOG_GUARD.set(Mutex::new(Some(guard)))
            .map_err(|_| anyhow::anyhow!("Logger already initialized"))?;
    } else {
        subscriber.init();
    }

    // Store the reload handle for runtime reconfiguration
    RELOAD_HANDLE.set(Mutex::new(reload_handle))
        .map_err(|_| anyhow::anyhow!("Reload handle already initialized"))?;

    Ok(())
}

/// Reload logging configuration at runtime with hot reload support
pub fn reload_logging(
    level: &str,
    log_dir: Option<&Path>,
    rotation: Rotation,
) -> anyhow::Result<()> {
    // Try to reload the log level dynamically
    if let Some(handle_mutex) = RELOAD_HANDLE.get() {
        if let Ok(handle) = handle_mutex.lock() {
            let new_filter = EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(level));

            match handle.reload(new_filter) {
                Ok(_) => {
                    tracing::info!(
                        "Log level changed to '{}'",
                        level
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to hot reload log level: {}. Change will take effect on restart.",
                        e
                    );
                }
            }
        }
    }

    // Log directory and rotation changes still require restart
    // Replacing the file appender would require dropping the old WorkerGuard
    let has_dir_or_rotation_change = log_dir.is_some() || !matches!(rotation, Rotation::Daily);

    if has_dir_or_rotation_change {
        tracing::info!(
            "Logging configuration updated - directory: {:?}, rotation: {:?}",
            log_dir,
            rotation
        );
        tracing::warn!(
            "Log directory and rotation changes require a service restart to take effect"
        );
    }

    Ok(())
}

pub fn shutdown_logging() {
    // Take the guard out of the static and drop it explicitly
    if let Some(mutex) = LOG_GUARD.get() {
        if let Ok(mut guard_option) = mutex.lock() {
            if let Some(guard) = guard_option.take() {
                drop(guard);
            }
        }
    }
}