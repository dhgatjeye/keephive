use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

const SERVICE_NAME: &str = "KeepHive";

define_windows_service!(ffi_service_main, service_entry_point);

/// FFI entry point called by Windows SCM
fn service_entry_point(arguments: Vec<OsString>) {
    let config_path = if arguments.len() > 1 {
        PathBuf::from(&arguments[1])
    } else {
        // Fallback
        eprintln!("WARNING: No config path in service arguments!");
        eprintln!("Using default: C:\\ProgramData\\KeepHive\\keephive_config.json");
        PathBuf::from(r"C:\ProgramData\KeepHive\keephive_config.json")
    };

    if let Err(e) = run_service(arguments, config_path) {
        error!("Service error: {:?}", e);
    }
}

fn run_service(
    _arguments: Vec<OsString>,
    config_path: PathBuf,
) -> windows_service::Result<()> {
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    info!("Windows Service starting...");

    let cancellation = Arc::new(CancellationToken::new());
    let cancellation_clone = cancellation.clone();

    let shutdown_requested = Arc::new(Mutex::new(false));
    let shutdown_clone = shutdown_requested.clone();

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                info!("Service stop requested");
                *shutdown_clone.lock().unwrap() = true;
                cancellation_clone.cancel();
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::StartPending,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::from_secs(5),
        process_id: None,
    })?;

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| windows_service::Error::Winapi(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

    let service_result = runtime.block_on(async move {
        run_async(config_path, cancellation, status_handle).await
    });

    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    });

    service_result.map_err(|e| {
        error!("Service error: {}", e);
        windows_service::Error::Winapi(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    })
}

async fn run_async(
    config_path: PathBuf,
    cancellation: Arc<tokio_util::sync::CancellationToken>,
    status_handle: service_control_handler::ServiceStatusHandle,
) -> Result<()> {
    use crate::service::ServiceDaemon;

    let config = load_config(&config_path).await?;

    init_logging_from_config(&config)?;

    info!("Windows Service starting...");

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    info!("Service running");

    let daemon = ServiceDaemon::new_for_service_impl(config, (*cancellation).clone()).await?;
    let config_path_clone = config_path.clone();
    let daemon_task = tokio::spawn(async move { daemon.run(config_path_clone).await });

    // Wait for daemon to complete (it will handle cancellation internally now)
    let result = daemon_task.await;

    match result {
        Ok(Ok(())) => {
            info!("Daemon completed successfully");
            Ok(())
        }
        Ok(Err(e)) => {
            error!("Daemon error: {}", e);
            Err(e)
        }
        Err(e) => {
            error!("Daemon task panicked: {}", e);
            anyhow::bail!("Daemon task failed: {}", e)
        }
    }
}

/// Load config and normalize paths for service mode
async fn load_config(path: &PathBuf) -> Result<crate::config::ServiceConfig> {
    if !path.exists() {
        anyhow::bail!("Config not found: {}", path.display());
    }

    let content = tokio::fs::read_to_string(path).await?;
    let mut config: crate::config::ServiceConfig = serde_json::from_str(&content)
        .context("Parse error")?;

    // Normalize relative paths to be relative to config file location
    if let Some(config_dir) = path.parent() {
        // Normalize state_path
        if !config.state_path.is_absolute() {
            let state_filename = config.state_path.file_name()
                .unwrap_or_else(|| "keephive_state.json".as_ref());
            config.state_path = config_dir.join(state_filename);
            info!("Service mode: state_path normalized to {}", config.state_path.display());
        }

        // Normalize log_directory
        if let Some(log_dir) = &config.log_directory {
            if !log_dir.is_absolute() {
                config.log_directory = Some(config_dir.join(log_dir));
                info!("Service mode: log_directory normalized to {}",
                    config.log_directory.as_ref().unwrap().display());
            }
        }
    }

    Ok(config)
}

fn init_logging_from_config(config: &crate::config::ServiceConfig) -> Result<()> {
    use crate::observability::{init_logging, Rotation};

    let rotation = match config.log_rotation {
        crate::config::LogRotation::Daily => Rotation::Daily,
        crate::config::LogRotation::Hourly => Rotation::Hourly,
        crate::config::LogRotation::Never => Rotation::Never,
    };

    init_logging(&config.log_level, config.log_directory.as_deref(), rotation)
}

pub fn get_service_dispatcher_entry() -> Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("Failed to start service dispatcher")
}