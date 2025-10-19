use anyhow::Result;
use chrono::Utc;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::BackupJob;
use crate::core::BackupOrchestrator;
use crate::state::{JobStatus, StateManager};

pub struct JobExecutor {
    pub(crate) orchestrator: BackupOrchestrator,
    pub(crate) state_manager: Arc<StateManager>,
    pub(crate) retention_count: usize,
}

// Make executor cloneable for spawning
impl Clone for JobExecutor {
    fn clone(&self) -> Self {
        Self {
            orchestrator: BackupOrchestrator::new(),
            state_manager: self.state_manager.clone(),
            retention_count: self.retention_count,
        }
    }
}

impl JobExecutor {
    pub fn new(state_manager: Arc<StateManager>) -> Self {
        Self {
            orchestrator: BackupOrchestrator::new(),
            state_manager,
            retention_count: 10, // Default, should be updated via set_retention_count
        }
    }

    /// Create executor with specific retention count from config
    pub fn with_retention_count(state_manager: Arc<StateManager>, retention_count: usize) -> Self {
        Self {
            orchestrator: BackupOrchestrator::new(),
            state_manager,
            retention_count,
        }
    }

    /// Update retention count (called when config changes)
    pub fn set_retention_count(&mut self, retention_count: usize) {
        self.retention_count = retention_count;
    }

    pub async fn execute_job(
        &self,
        job: &BackupJob,
        cancellation: CancellationToken,
    ) -> Result<()> {
        info!("Executing job: {}", job.id);

        // Update state to Running
        self.state_manager.update_job_state(&job.id, |js| {
            js.status = JobStatus::Running {
                started_at: Utc::now(),
            };
            js.source = job.source.clone();
            js.target = job.target.clone();
        }).await?;

        // Execute backup
        let result = self.orchestrator.execute_backup(
            &job.id,
            &job.source,
            &job.target,
            cancellation,
        ).await;

        match result {
            Ok(metadata) => {
                // Update state to Idle with successful backup
                self.state_manager.update_job_state(&job.id, |js| {
                    js.status = JobStatus::Idle;
                    js.last_run = Some(Utc::now());
                    js.last_backup = Some(metadata.clone());
                    js.active_backup = None;
                }).await?;

                // Cleanup old backups using actual retention count from config
                info!(
                    "Cleaning up old backups for job {} (retention: {} backups)",
                    job.id, self.retention_count
                );

                if let Err(e) = BackupOrchestrator::cleanup_old_backups(
                    &job.target,
                    self.retention_count,
                ).await {
                    warn!("Failed to cleanup old backups for job {}: {}", job.id, e);
                }

                info!("Job completed successfully: {}", job.id);
                Ok(())
            }
            Err(e) => {
                error!("Job failed: {}: {}", job.id, e);

                // Update state to Failed
                self.state_manager.update_job_state(&job.id, |js| {
                    js.status = JobStatus::Failed {
                        error: e.to_string(),
                        timestamp: Utc::now(),
                    };
                    js.active_backup = None;
                }).await?;

                Err(e)
            }
        }
    }
}