use anyhow::Result;
use std::path::Path;
use tracing::{info, warn};

use crate::core::BackupOrchestrator;

pub struct RecoveryManager;

impl RecoveryManager {
    pub fn new(_state_manager: std::sync::Arc<crate::state::StateManager>) -> Self {
        Self
    }

    /// Detect and log partial backups on startup
    pub async fn recover_partial_backups(&self, target_dirs: Vec<&Path>) -> Result<()> {
        info!("Checking for partial backups...");

        for target in target_dirs {
            let partials = BackupOrchestrator::detect_partial_backups(target).await?;

            for partial_path in partials {
                warn!("Found partial backup: {}", partial_path.display());
                warn!("Manual action required: Review and delete partial backup if needed");
            }
        }

        Ok(())
    }
}