use anyhow::Result;
use chrono::Utc;
use std::collections::HashMap;
use tracing::{debug, info};

use crate::config::BackupJob;
use crate::state::{JobState, JobStatus, StateManager};

pub struct Scheduler {
    state_manager: std::sync::Arc<StateManager>,
}

impl Scheduler {
    pub fn new(state_manager: std::sync::Arc<StateManager>) -> Self {
        Self { state_manager }
    }

    /// Calculate next run time for all jobs
    pub async fn calculate_next_runs(&self, jobs: &[BackupJob]) -> Result<()> {
        for job in jobs {
            let state = self.state_manager.read().await;
            let job_state = state.get_job(&job.id);
            let last_run = job_state.and_then(|js| js.last_run);
            let current_status = job_state.map(|js| js.status.clone());
            drop(state);

            // Skip calculation for running jobs
            if let Some(JobStatus::Running { .. }) = current_status {
                debug!("Skipping next_run calculation for running job: {}", job.id);
                continue;
            }

            let next_duration = job.schedule.next_run_duration(last_run);
            let next_run = Utc::now() + next_duration;

            self.state_manager.update_job_state(&job.id, |js| {
                js.next_run = Some(next_run);
                debug!("Job {} scheduled for {}", job.id, next_run);
            }).await?;
        }

        Ok(())
    }

    /// Get jobs that are ready to run
    pub async fn get_ready_jobs(&self, jobs: &[BackupJob]) -> Result<Vec<BackupJob>> {
        let mut ready_jobs = Vec::new();
        let now = Utc::now();

        let state = self.state_manager.read().await;

        for job in jobs {
            if let Some(job_state) = state.get_job(&job.id) {
                // Only run if idle and next_run has passed
                if matches!(job_state.status, JobStatus::Idle) {
                    if let Some(next_run) = job_state.next_run {
                        if next_run <= now {
                            ready_jobs.push(job.clone());
                        }
                    } else {
                        // No next_run set, run immediately
                        ready_jobs.push(job.clone());
                    }
                }
            } else {
                // New job, run immediately
                ready_jobs.push(job.clone());
            }
        }

        Ok(ready_jobs)
    }

    /// Initialize job states for new jobs
    pub async fn initialize_jobs(&self, jobs: &[BackupJob]) -> Result<()> {
        let mut state = self.state_manager.write().await;

        for job in jobs {
            if state.get_job(&job.id).is_none() {
                info!("Initializing new job: {}", job.id);
                let job_state = JobState::new(
                    job.id.clone(),
                    job.source.clone(),
                    job.target.clone(),
                );
                state.upsert_job(job_state);
            }
        }

        drop(state);
        self.state_manager.save().await?;

        Ok(())
    }

    /// Detect configuration changes for running jobs
    pub async fn detect_config_changes(
        &self,
        old_jobs: &[BackupJob],
        new_jobs: &[BackupJob],
    ) -> Result<ConfigChanges> {
        let mut changes = ConfigChanges {
            added: Vec::new(),
            removed: Vec::new(),
            modified: Vec::new(),
        };

        let old_map: HashMap<_, _> = old_jobs.iter()
            .map(|j| (j.id.clone(), j))
            .collect();

        let new_map: HashMap<_, _> = new_jobs.iter()
            .map(|j| (j.id.clone(), j))
            .collect();

        // Find added jobs
        for job in new_jobs {
            if !old_map.contains_key(&job.id) {
                changes.added.push(job.clone());
            }
        }

        // Find removed jobs
        for job in old_jobs {
            if !new_map.contains_key(&job.id) {
                changes.removed.push(job.id.clone());
            }
        }

        // Find modified jobs with detailed change type
        for job in new_jobs {
            if let Some(old_job) = old_map.get(&job.id) {
                let schedule_changed = job.schedule != old_job.schedule;
                let path_changed = job.source != old_job.source || job.target != old_job.target;

                if schedule_changed || path_changed {
                    let change_type = match (schedule_changed, path_changed) {
                        (true, true) => ConfigChangeType::PathAndSchedule,
                        (false, true) => ConfigChangeType::PathChanged,
                        (true, false) => ConfigChangeType::ScheduleOnly,
                        (false, false) => unreachable!(),
                    };

                    changes.modified.push(ModifiedJob {
                        job: job.clone(),
                        change_type,
                    });
                }
            }
        }

        Ok(changes)
    }
}

/// Configuration change detection result
#[derive(Debug)]
pub struct ConfigChanges {
    pub added: Vec<BackupJob>,
    pub removed: Vec<String>,
    pub modified: Vec<ModifiedJob>,
}

/// Details about a modified job
#[derive(Debug, Clone)]
pub struct ModifiedJob {
    pub job: BackupJob,
    pub change_type: ConfigChangeType,
}

/// Type of configuration change
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigChangeType {
    /// Only schedule changed (safe to continue running job)
    ScheduleOnly,
    /// Source or target changed (requires job restart)
    PathChanged,
    /// Both changed
    PathAndSchedule,
}