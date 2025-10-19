use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tracing::info;

pub struct WindowsService;

impl Default for WindowsService {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsService {
    pub fn new() -> Self {
        Self
    }

    /// Install service in Windows SCM
    pub fn install(config_path: Option<PathBuf>) -> Result<()> {
        let exe_path = std::env::current_exe()
            .context("Failed to get executable path")?;

        // Determine config path (absolute)
        let config_full_path = if let Some(path) = config_path {
            if path.is_absolute() {
                path
            } else {
                std::env::current_dir()?.join(path)
            }
        } else {
            PathBuf::from(r"C:\ProgramData\KeepHive\keephive_config.json")
        };

        // Pass config path via binPath argument
        let bin_path = format!("\"{}\" --service \"{}\"", exe_path.display(), config_full_path.display());

        info!("Installing Windows Service: KeepHive");
        info!("Binary path: {}", bin_path);
        info!("Config path: {}", config_full_path.display());

        let output = Command::new("sc")
            .args(&[
                "create",
                "KeepHive",
                "binPath=",
                &bin_path,
                "DisplayName=",
                "KeepHive Backup Service",
                "start=",
                "auto",
            ])
            .output()
            .context("Failed to execute sc create")?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to create service: {}", error);
        }

        // Set description
        let _ = Command::new("sc")
            .args(&[
                "description",
                "KeepHive",
                "A Daemon service for KeepHive backup operations.",
            ])
            .output();

        info!("✓ Service installed successfully");
        info!("  Config: {}", config_full_path.display());
        info!("  Start:  sc start KeepHive");
        info!("  Stop:   sc stop KeepHive");
        info!("  Status: sc query KeepHive");
        Ok(())
    }

    /// Uninstall service from Windows SCM
    pub fn uninstall() -> Result<()> {
        info!("Uninstalling Windows Service: KeepHive");

        // Stop first
        let _ = Command::new("sc").args(&["stop", "KeepHive"]).output();
        std::thread::sleep(Duration::from_secs(2));

        // Delete
        let output = Command::new("sc")
            .args(&["delete", "KeepHive"])
            .output()
            .context("Failed to execute sc delete")?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to delete service: {}", error);
        }

        info!("✓ Service uninstalled successfully");
        Ok(())
    }

    /// Start the service
    pub fn start() -> Result<()> {
        info!("Starting KeepHive service...");
        let output = Command::new("sc")
            .args(&["start", "KeepHive"])
            .output()
            .context("Failed to start service")?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to start service: {}", error);
        }

        info!("✓ Service started");
        Ok(())
    }

    /// Stop the service
    pub fn stop() -> Result<()> {
        info!("Stopping KeepHive service...");
        let output = Command::new("sc")
            .args(&["stop", "KeepHive"])
            .output()
            .context("Failed to stop service")?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to stop service: {}", error);
        }

        info!("✓ Service stopped");
        Ok(())
    }
}