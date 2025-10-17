use anyhow::{Context, Result};
use keephive::{
    config::ServiceConfig,
    observability::{init_logging, Rotation},
    service::ServiceDaemon,
};
use std::path::PathBuf;
use tracing::info;

#[cfg(windows)]
use keephive::platform::windows::service::WindowsService;

fn main() -> Result<()> {
    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();

    // Check for service-related commands
    if args.len() > 1 {
        match args[1].as_str() {
            "--install" => {
                let config_path = if args.len() > 2 {
                    Some(PathBuf::from(&args[2]))
                } else {
                    None
                };
                return WindowsService::install(config_path);
            }
            "--uninstall" => {
                return WindowsService::uninstall();
            }
            "--start" => {
                return WindowsService::start();
            }
            "--stop" => {
                return WindowsService::stop();
            }
            #[cfg(windows)]
            "--service" => unsafe {
                if args.len() < 3 {
                    eprintln!("Error: --service requires config path argument");
                    eprintln!("This should be set automatically during installation");
                    std::process::exit(1);
                }

                let config_path = PathBuf::from(&args[2]);

                // Set config path in environment for service_impl to read
                std::env::set_var("KEEPHIVE_CONFIG", config_path.to_str().unwrap());

                use keephive::platform::windows::service_impl;
                return service_impl::get_service_dispatcher_entry();
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            _ => {
                // Assume it's a config file path
            }
        }
    }

    // Run in console mode
    run_console_mode()?;
    Ok(())
}

#[tokio::main]
async fn run_console_mode() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let config_path = if args.len() > 1 && !args[1].starts_with("--") {
        PathBuf::from(&args[1])
    } else {
        PathBuf::from("keephive_config.json")
    };

    // Load configuration
    let config = load_config(&config_path).await
        .context("Failed to load configuration")?;

    // Initialize logging with console + optional file output
    let rotation = match config.log_rotation {
        keephive::config::LogRotation::Daily => Rotation::Daily,
        keephive::config::LogRotation::Hourly => Rotation::Hourly,
        keephive::config::LogRotation::Never => Rotation::Never,
    };

    init_logging(
        &config.log_level,
        config.log_directory.as_deref(),
        rotation,
    )?;

    info!("KeepHive v{} - Console Mode", env!("CARGO_PKG_VERSION"));
    info!("Configuration loaded from: {}", config_path.display());

    if let Some(log_dir) = &config.log_directory {
        info!("File logging enabled: {}", log_dir.display());
    } else {
        info!("Console logging only (no log file configured)");
    }

    info!("Press Ctrl+C to stop");

    // Create and run service daemon
    let daemon = ServiceDaemon::new(config).await?;
    daemon.run(config_path).await?;

    Ok(())
}

async fn load_config(path: &PathBuf) -> Result<ServiceConfig> {
    if !path.exists() {
        anyhow::bail!(
            "Configuration file not found: {}\n\nCreate a config file first. Example:\n{}",
            path.display(),
            get_example_config()
        );
    }

    let content = tokio::fs::read_to_string(path).await
        .context("Failed to read config file")?;

    let config: ServiceConfig = serde_json::from_str(&content)
        .context("Failed to parse config file")?;

    Ok(config)
}

fn print_help() {
    println!("KeepHive v{} - Enterprise Backup Daemon", env!("CARGO_PKG_VERSION"));
    println!();
    println!("USAGE:");
    println!("  keephive.exe [CONFIG_FILE]              Run in console mode");
    println!("  keephive.exe --install [CONFIG_FILE]    Install as Windows Service");
    println!("  keephive.exe --uninstall                Uninstall Windows Service");
    println!("  keephive.exe --start                    Start Windows Service");
    println!("  keephive.exe --stop                     Stop Windows Service");
    println!("  keephive.exe --help                     Show this help");
    println!();
    println!("EXAMPLES:");
    println!("  # Run in console mode (interactive)");
    println!("  keephive.exe config.json");
    println!();
    println!("  # Install and run as Windows Service");
    println!("  keephive.exe --install config.json");
    println!("  sc start KeepHive");
    println!();
    println!("  # Uninstall service");
    println!("  sc stop KeepHive");
    println!("  keephive.exe --uninstall");
}

fn get_example_config() -> &'static str {
    r#"{
  "jobs": [
    {
      "id": "my_backup",
      "source": "C:\\Users\\User\\Documents",
      "target": "D:\\Backups",
      "schedule": {
        "type": "daily",
        "hour": 2,
        "minute": 0
      },
      "description": "Daily backup of Documents"
    }
  ],
  "retention_count": 10,
  "log_level": "info",
  "state_path": ".keephive_state.json",
  "log_directory": "./logs",
  "log_rotation": {
    "type": "daily"
  }
}"#
}