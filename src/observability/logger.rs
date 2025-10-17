use std::path::Path;
use std::sync::{Mutex, OnceLock};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

/// Must be kept alive for the entire application lifetime
static LOG_GUARD: OnceLock<Mutex<Option<tracing_appender::non_blocking::WorkerGuard>>> = OnceLock::new();

/// Log rotation strategy
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

    // Console layer - always enabled
    let console_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .with_filter(filter.clone());

    // Build subscriber with console layer
    let subscriber = tracing_subscriber::registry().with(console_layer);

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
            .with_ansi(false) // No ANSI colors in file
            .with_filter(filter);

        subscriber.with(file_layer).init();

        LOG_GUARD.set(Mutex::new(Some(guard)))
            .map_err(|_| anyhow::anyhow!("Logger already initialized"))?;
    } else {
        subscriber.init();
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