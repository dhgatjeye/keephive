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
}

impl JobExecutor {
    pub fn new(state_manager: Arc<StateManager>) -> Self {
        Self {
            orchestrator: BackupOrchestrator::new(),
            state_manager,
        }
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

                // Cleanup old backups
                let retention_count = 10; // From config
                if let Err(e) = BackupOrchestrator::cleanup_old_backups(&job.target, retention_count).await {
                    warn!("Failed to cleanup old backups: {}", e);
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