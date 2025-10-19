use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::ServiceConfig;
use crate::observability::{reload_logging, shutdown_logging, Rotation};
use crate::scheduler::{JobExecutor, Scheduler};
use crate::service::{setup_shutdown_handler, RecoveryManager};
use crate::state::{ConfigWatcher, StateManager};

/// Service daemon orchestrating all operations
pub struct ServiceDaemon {
    config: ServiceConfig,
    state_manager: Arc<StateManager>,
    scheduler: Scheduler,
    executor: JobExecutor,
    recovery: RecoveryManager,
    cancellation: CancellationToken,
}

impl ServiceDaemon {
    pub async fn new(config: ServiceConfig) -> Result<Self> {
        let state_manager = Arc::new(
            StateManager::new(config.state_path.clone()).await
                .context("Failed to initialize state manager")?
        );

        let scheduler = Scheduler::new(state_manager.clone());
        let executor = JobExecutor::with_retention_count(
            state_manager.clone(),
            config.retention_count,
        );
        let recovery = RecoveryManager::new(state_manager.clone());
        let cancellation = CancellationToken::new();

        Ok(Self {
            config,
            state_manager,
            scheduler,
            executor,
            recovery,
            cancellation,
        })
    }

    /// Create daemon with external cancellation token (for service mode)
    pub async fn new_for_service_impl(config: ServiceConfig, cancellation: CancellationToken) -> Result<Self> {
        let state_manager = Arc::new(
            StateManager::new(config.state_path.clone()).await
                .context("Failed to initialize state manager")?
        );

        let scheduler = Scheduler::new(state_manager.clone());
        let executor = JobExecutor::with_retention_count(
            state_manager.clone(),
            config.retention_count,
        );
        let recovery = RecoveryManager::new(state_manager.clone());

        Ok(Self {
            config,
            state_manager,
            scheduler,
            executor,
            recovery,
            cancellation,
        })
    }

    /// Run the service daemon
    pub async fn run(mut self, config_path: std::path::PathBuf) -> Result<()> {
        info!("KeepHive service starting...");

        // Setup shutdown handler
        setup_shutdown_handler(self.cancellation.clone()).await;

        // Initialize job states before recovery
        self.scheduler.initialize_jobs(&self.config.jobs).await?;

        // Reset failed jobs to Idle on startup
        self.reset_failed_jobs().await?;

        // Recover from partial backups
        let target_dirs: Vec<_> = self.config.jobs.iter()
            .map(|j| j.target.as_path())
            .collect();
        self.recovery.recover_partial_backups(target_dirs).await?;

        // Calculate initial next runs
        self.scheduler.calculate_next_runs(&self.config.jobs).await?;

        // Setup config watcher with cancellation support
        let (watcher, mut config_rx) = ConfigWatcher::new(config_path, self.cancellation.clone())?;
        tokio::spawn(async move {
            if let Err(e) = watcher.watch().await {
                error!("Config watcher error: {}", e);
            }
        });

        // Main service loop - track both handles and cancellation tokens
        let mut running_jobs: std::collections::HashMap<
            String,
            (tokio::task::JoinHandle<Result<()>>, CancellationToken)
        > = std::collections::HashMap::new();

        loop {
            tokio::select! {
                // Check for shutdown
                _ = self.cancellation.cancelled() => {
                    info!("Shutdown signal received, waiting for jobs to complete...");
                    self.shutdown_gracefully(&mut running_jobs).await?;
                    break;
                }

                // Config changes
                Some(config_change) = config_rx.recv() => {
                    info!("Configuration changed, processing updates...");
                    self.handle_config_change(config_change.config, &mut running_jobs).await?;
                }

                // Periodic job check
                _ = sleep(Duration::from_secs(5)) => {
                    self.process_jobs(&mut running_jobs).await?;
                }
            }
        }

        info!("KeepHive service stopped");
        Ok(())
    }

    /// Reset failed jobs to Idle on startup
    async fn reset_failed_jobs(&self) -> Result<()> {
        let state = self.state_manager.read().await;
        let failed_jobs: Vec<String> = state.jobs.iter()
            .filter(|j| matches!(j.status, crate::state::JobStatus::Failed { .. }))
            .map(|j| j.id.clone())
            .collect();
        drop(state);

        for job_id in failed_jobs {
            info!("Resetting failed job to Idle: {}", job_id);
            self.state_manager.update_job_state(&job_id, |js| {
                js.status = crate::state::JobStatus::Idle;
            }).await?;
        }

        Ok(())
    }

    async fn process_jobs(
        &mut self,
        running_jobs: &mut std::collections::HashMap<String, (tokio::task::JoinHandle<Result<()>>, CancellationToken)>,
    ) -> Result<()> {
        // Track which jobs completed
        let mut completed_jobs = Vec::new();

        // Remove completed jobs
        running_jobs.retain(|id, (handle, _token)| {
            if handle.is_finished() {
                debug!("Job completed: {}", id);
                completed_jobs.push(id.clone());
                false
            } else {
                true
            }
        });

        // Recalculate next runs only for completed jobs
        if !completed_jobs.is_empty() {
            // Filter config to only include completed jobs
            let completed_job_configs: Vec<_> = self.config.jobs.iter()
                .filter(|j| completed_jobs.contains(&j.id))
                .cloned()
                .collect();

            for job_config in completed_job_configs {
                self.scheduler.calculate_next_runs(&[job_config]).await?;
            }
        }

        // Get ready jobs
        let ready_jobs = self.scheduler.get_ready_jobs(&self.config.jobs).await?;

        for job in ready_jobs {
            if !running_jobs.contains_key(&job.id) {
                info!("Starting job: {}", job.id);

                let executor = self.executor.clone();
                let job_clone = job.clone();
                let job_cancellation = self.cancellation.child_token();
                let job_cancellation_clone = job_cancellation.clone();

                let handle = tokio::spawn(async move {
                    executor.execute_job(&job_clone, job_cancellation_clone).await
                });

                running_jobs.insert(job.id.clone(), (handle, job_cancellation));
            }
        }

        Ok(())
    }

    async fn handle_config_change(
        &mut self,
        new_config: ServiceConfig,
        running_jobs: &mut std::collections::HashMap<String, (tokio::task::JoinHandle<Result<()>>, CancellationToken)>,
    ) -> Result<()> {
        // Detect changes in global configuration parameters
        let retention_changed = self.config.retention_count != new_config.retention_count;
        let log_level_changed = self.config.log_level != new_config.log_level;
        let log_directory_changed = self.config.log_directory != new_config.log_directory;
        let log_rotation_changed = !matches!(
            (&self.config.log_rotation, &new_config.log_rotation),
            (crate::config::LogRotation::Daily, crate::config::LogRotation::Daily) |
            (crate::config::LogRotation::Hourly, crate::config::LogRotation::Hourly) |
            (crate::config::LogRotation::Never, crate::config::LogRotation::Never)
        );
        let state_path_changed = self.config.state_path != new_config.state_path;

        // Log detected configuration changes
        if retention_changed {
            info!(
                "Retention count changed: {} -> {}",
                self.config.retention_count,
                new_config.retention_count
            );
        }

        if log_level_changed {
            info!(
                "Log level changed: {} -> {}",
                self.config.log_level,
                new_config.log_level
            );
        }

        if log_directory_changed {
            info!(
                "Log directory changed: {:?} -> {:?}",
                self.config.log_directory,
                new_config.log_directory
            );
        }

        if log_rotation_changed {
            info!(
                "Log rotation changed: {:?} -> {:?}",
                self.config.log_rotation,
                new_config.log_rotation
            );
        }

        if state_path_changed {
            warn!(
                "State path changed: {:?} -> {:?}. This requires a service restart to take effect.",
                self.config.state_path,
                new_config.state_path
            );
        }

        // Apply logging configuration changes
        if log_level_changed || log_directory_changed || log_rotation_changed {
            let rotation = match new_config.log_rotation {
                crate::config::LogRotation::Daily => Rotation::Daily,
                crate::config::LogRotation::Hourly => Rotation::Hourly,
                crate::config::LogRotation::Never => Rotation::Never,
            };

            if let Err(e) = reload_logging(
                &new_config.log_level,
                new_config.log_directory.as_deref(),
                rotation,
            ) {
                warn!("Failed to reload logging configuration: {}", e);
            }
        }

        // Apply retention count changes
        if retention_changed {
            self.executor.set_retention_count(new_config.retention_count);
            info!("Retention count updated successfully");
        }

        // Detect job configuration changes
        let changes = self.scheduler.detect_config_changes(
            &self.config.jobs,
            &new_config.jobs,
        ).await?;

        // Handle removed jobs - cancel with token before aborting
        for removed_id in &changes.removed {
            if let Some((handle, token)) = running_jobs.remove(removed_id) {
                warn!("Job {} removed from config, cancelling running backup", removed_id);

                // Cancel the token first - this signals execute_backup to stop
                token.cancel();

                // Then abort the task as fallback
                handle.abort();
            }
            info!("Job removed: {}", removed_id);
        }

        // Handle modified jobs (handling based on change type)
        for modified in &changes.modified {
            let job_id = &modified.job.id;
            let is_running = running_jobs.contains_key(job_id);

            match &modified.change_type {
                crate::scheduler::engine::ConfigChangeType::ScheduleOnly => {
                    if is_running {
                        info!(
                            "Job {} schedule changed (but currently running), will apply new schedule after completion",
                            job_id
                        );
                    } else {
                        info!("Job {} schedule changed, recalculating next run", job_id);
                    }
                    // No action needed for running job, it will finish with old schedule
                    // New schedule will be applied when next_run is recalculated
                }

                crate::scheduler::engine::ConfigChangeType::PathChanged => {
                    if is_running {
                        warn!(
                            "Job {} source/target changed, cancelling running backup for safety",
                            job_id
                        );
                        if let Some((handle, token)) = running_jobs.remove(job_id) {
                            token.cancel();
                            handle.abort();
                        }

                        // Mark as failed and update paths in state
                        self.state_manager.update_job_state(job_id, |js| {
                            js.status = crate::state::JobStatus::Failed {
                                error: "Backup cancelled due to source/target path change".to_string(),
                                timestamp: chrono::Utc::now(),
                            };
                            js.source = modified.job.source.clone();
                            js.target = modified.job.target.clone();
                        }).await?;
                    } else {
                        info!("Job {} source/target changed, updating state", job_id);
                        // Update paths in state
                        self.state_manager.update_job_state(job_id, |js| {
                            js.source = modified.job.source.clone();
                            js.target = modified.job.target.clone();
                        }).await?;
                    }
                }

                crate::scheduler::engine::ConfigChangeType::PathAndSchedule => {
                    if is_running {
                        warn!(
                            "Job {} path and schedule changed, cancelling running backup",
                            job_id
                        );
                        if let Some((handle, token)) = running_jobs.remove(job_id) {
                            token.cancel();
                            handle.abort();
                        }

                        // Mark as failed and update both paths and schedule
                        self.state_manager.update_job_state(job_id, |js| {
                            js.status = crate::state::JobStatus::Failed {
                                error: "Backup cancelled due to configuration change".to_string(),
                                timestamp: chrono::Utc::now(),
                            };
                            js.source = modified.job.source.clone();
                            js.target = modified.job.target.clone();
                        }).await?;
                    } else {
                        info!("Job {} path and schedule changed, updating state", job_id);
                        // Update paths in state
                        self.state_manager.update_job_state(job_id, |js| {
                            js.source = modified.job.source.clone();
                            js.target = modified.job.target.clone();
                        }).await?;
                    }
                }
            }
        }

        // Update config
        self.config = new_config;

        // Initialize new jobs
        self.scheduler.initialize_jobs(&self.config.jobs).await?;

        // Recalculate next runs for all jobs (including modified ones)
        self.scheduler.calculate_next_runs(&self.config.jobs).await?;

        info!("Configuration reloaded: {} jobs ({} added, {} removed, {} modified)",
            self.config.jobs.len(),
            changes.added.len(),
            changes.removed.len(),
            changes.modified.len()
        );

        Ok(())
    }

    /// Shutdown - wait for running jobs
    async fn shutdown_gracefully(
        &self,
        running_jobs: &mut std::collections::HashMap<String, (tokio::task::JoinHandle<Result<()>>, CancellationToken)>,
    ) -> Result<()> {
        info!("Waiting for {} running jobs to complete...", running_jobs.len());

        // Wait for all jobs with timeout
        let timeout = Duration::from_secs(300); // 5 minutes
        let start = std::time::Instant::now();

        while !running_jobs.is_empty() && start.elapsed() < timeout {
            running_jobs.retain(|id, (handle, _token)| {
                if handle.is_finished() {
                    info!("Job finished during shutdown: {}", id);
                    false
                } else {
                    true
                }
            });

            if !running_jobs.is_empty() {
                sleep(Duration::from_secs(1)).await;
            }
        }

        // Force cancel remaining jobs
        if !running_jobs.is_empty() {
            warn!("Force cancelling {} remaining jobs", running_jobs.len());
            for (id, (handle, token)) in running_jobs.drain() {
                warn!("Cancelling job: {}", id);

                token.cancel();
                handle.abort();
            }
        }

        // Final state save
        self.state_manager.save().await?;

        // Flush logging before shutdown
        info!("Flushing logs before shutdown...");
        shutdown_logging();

        Ok(())
    }
}