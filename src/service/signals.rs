use tokio::signal;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Setup graceful shutdown handler
pub async fn setup_shutdown_handler(cancellation: CancellationToken) {
    tokio::spawn(async move {
        #[cfg(windows)]
        {
            match signal::ctrl_c().await {
                Ok(()) => {
                    info!("Received shutdown signal (Ctrl+C)");
                    cancellation.cancel();
                }
                Err(e) => {
                    eprintln!("Failed to listen for shutdown signal: {}", e);
                }
            }
        }

        #[cfg(unix)]
        {
            use signal::unix::{signal, SignalKind};

            let mut sigterm = signal(SignalKind::terminate()).unwrap();
            let mut sigint = signal(SignalKind::interrupt()).unwrap();

            tokio::select! {
                _ = sigterm.recv() => {
                    info!("Received SIGTERM");
                    cancellation.cancel();
                }
                _ = sigint.recv() => {
                    info!("Received SIGINT");
                    cancellation.cancel();
                }
            }
        }
    });
}