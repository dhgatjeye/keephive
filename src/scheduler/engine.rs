use anyhow::Result;
use chrono::Utc;
use std::collections::HashMap;
use tracing::{debug, info};

pub use super::changes::{ConfigChangeType, ConfigChanges, ModifiedJob};
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
        Self::validate_no_duplicate_job_ids(jobs)?;

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

    /// Validate that there are no duplicate job IDs
    fn validate_no_duplicate_job_ids(jobs: &[BackupJob]) -> Result<()> {
        let mut seen_ids = HashMap::new();
        let mut duplicates = Vec::new();

        for (index, job) in jobs.iter().enumerate() {
            if let Some(&first_index) = seen_ids.get(&job.id) {
                duplicates.push((job.id.clone(), first_index, index));
            } else {
                seen_ids.insert(job.id.clone(), index);
            }
        }

        if !duplicates.is_empty() {
            let mut error_msg = String::from("Duplicate job IDs detected in configuration:\n");
            for (id, first_idx, dup_idx) in duplicates {
                error_msg.push_str(&format!(
                    "  - Job ID '{}' appears at positions {} and {}\n",
                    id, first_idx, dup_idx
                ));
            }
            error_msg.push_str("\nEach job must have a unique ID. Please fix the configuration.");

            anyhow::bail!(error_msg);
        }

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


#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Schedule;
    use std::path::PathBuf;
    use tempfile::TempDir;

    async fn create_test_scheduler() -> (Scheduler, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("test_state.json");
        let state_manager = std::sync::Arc::new(
            StateManager::new(state_path).await.unwrap()
        );
        let scheduler = Scheduler::new(state_manager);
        (scheduler, temp_dir)
    }

    fn create_test_job(id: &str) -> BackupJob {
        BackupJob {
            id: id.to_string(),
            source: PathBuf::from(format!("C:\\source_{}", id)),
            target: PathBuf::from(format!("C:\\target_{}", id)),
            schedule: Schedule::Interval { seconds: 3600 },
            description: String::new(),
        }
    }

    #[tokio::test]
    async fn test_no_duplicate_jobs_succeeds() {
        let (scheduler, _temp_dir) = create_test_scheduler().await;

        let jobs = vec![
            create_test_job("job1"),
            create_test_job("job2"),
            create_test_job("job3"),
        ];

        let result = scheduler.initialize_jobs(&jobs).await;
        assert!(result.is_ok(), "Valid jobs should initialize successfully");
    }

    #[tokio::test]
    async fn test_duplicate_job_ids_rejected() {
        let (scheduler, _temp_dir) = create_test_scheduler().await;

        let jobs = vec![
            create_test_job("job1"),
            create_test_job("job2"),
            create_test_job("job1"), // duplicate
        ];

        let result = scheduler.initialize_jobs(&jobs).await;
        assert!(result.is_err(), "Duplicate job IDs should be rejected");

        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("Duplicate job IDs"),
                "Error should mention duplicates: {}", error_msg);
        assert!(error_msg.contains("job1"),
                "Error should mention the duplicate ID: {}", error_msg);
        assert!(error_msg.contains("positions"),
                "Error should show positions: {}", error_msg);
    }

    #[tokio::test]
    async fn test_multiple_duplicates_all_reported() {
        let (scheduler, _temp_dir) = create_test_scheduler().await;

        let jobs = vec![
            create_test_job("job1"),
            create_test_job("job2"),
            create_test_job("job1"), // duplicate of job1
            create_test_job("job3"),
            create_test_job("job2"), // duplicate of job2
        ];

        let result = scheduler.initialize_jobs(&jobs).await;
        assert!(result.is_err(), "Multiple duplicates should be rejected");

        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("job1"), "Should report job1 duplicate");
        assert!(error_msg.contains("job2"), "Should report job2 duplicate");
    }

    #[tokio::test]
    async fn test_triple_duplicate_reported() {
        let (scheduler, _temp_dir) = create_test_scheduler().await;

        let jobs = vec![
            create_test_job("duplicate"),
            create_test_job("duplicate"), // second occurrence
            create_test_job("duplicate"), // third occurrence
        ];

        let result = scheduler.initialize_jobs(&jobs).await;
        assert!(result.is_err(), "Triple duplicate should be rejected");

        let error_msg = result.unwrap_err().to_string();

        // Should report at least two duplicate instances
        let duplicate_count = error_msg.matches("duplicate").count();
        assert!(duplicate_count >= 2, "Should report multiple occurrences");
    }

    #[tokio::test]
    async fn test_empty_job_list_succeeds() {
        let (scheduler, _temp_dir) = create_test_scheduler().await;

        let jobs: Vec<BackupJob> = vec![];
        let result = scheduler.initialize_jobs(&jobs).await;
        assert!(result.is_ok(), "Empty job list should be valid");
    }

    #[tokio::test]
    async fn test_single_job_succeeds() {
        let (scheduler, _temp_dir) = create_test_scheduler().await;

        let jobs = vec![create_test_job("only_job")];
        let result = scheduler.initialize_jobs(&jobs).await;
        assert!(result.is_ok(), "Single job should initialize successfully");
    }

    #[tokio::test]
    async fn test_validate_no_duplicate_job_ids_directly() {
        // Test the validation function directly
        let jobs = vec![
            create_test_job("job1"),
            create_test_job("job2"),
        ];

        let result = Scheduler::validate_no_duplicate_job_ids(&jobs);
        assert!(result.is_ok(), "No duplicates should pass validation");

        let jobs_with_dup = vec![
            create_test_job("job1"),
            create_test_job("job1"),
        ];

        let result = Scheduler::validate_no_duplicate_job_ids(&jobs_with_dup);
        assert!(result.is_err(), "Duplicates should fail validation");
    }

    #[tokio::test]
    async fn test_case_sensitive_job_ids() {
        let (scheduler, _temp_dir) = create_test_scheduler().await;

        // Job IDs should be case-sensitive
        let mut job1 = create_test_job("JobOne");
        let mut job2 = create_test_job("jobone");

        job1.id = "JobOne".to_string();
        job2.id = "jobone".to_string();

        let jobs = vec![job1, job2];
        let result = scheduler.initialize_jobs(&jobs).await;
        assert!(result.is_ok(),
                "Job IDs with different cases should be treated as different");
    }

    #[tokio::test]
    async fn test_whitespace_in_job_ids_matters() {
        let (scheduler, _temp_dir) = create_test_scheduler().await;

        let mut job1 = create_test_job("job1");
        let mut job2 = create_test_job("job1 ");

        job1.id = "job1".to_string();
        job2.id = "job1 ".to_string(); // trailing space

        let jobs = vec![job1, job2];
        let result = scheduler.initialize_jobs(&jobs).await;
        assert!(result.is_ok(),
                "Job IDs with different whitespace should be treated as different");
    }

    #[tokio::test]
    async fn test_duplicate_detection_error_message_format() {
        let (scheduler, _temp_dir) = create_test_scheduler().await;

        let jobs = vec![
            create_test_job("my_backup_job"),
            create_test_job("another_job"),
            create_test_job("my_backup_job"), // duplicate at position 2
        ];

        let result = scheduler.initialize_jobs(&jobs).await;
        assert!(result.is_err());

        let error_msg = result.unwrap_err().to_string();

        // Verify error message contains all required information
        assert!(error_msg.contains("Duplicate"), "Should mention 'Duplicate'");
        assert!(error_msg.contains("my_backup_job"), "Should mention the job ID");
        assert!(error_msg.contains("0"), "Should show first position");
        assert!(error_msg.contains("2"), "Should show duplicate position");
        assert!(error_msg.contains("unique ID"),
                "Should suggest using unique IDs");
    }

    #[tokio::test]
    async fn test_scheduler_initialization_with_valid_jobs() {
        let (scheduler, _temp_dir) = create_test_scheduler().await;

        let jobs = vec![
            create_test_job("daily_backup"),
            create_test_job("weekly_backup"),
            create_test_job("monthly_backup"),
        ];

        // Initialize jobs
        scheduler.initialize_jobs(&jobs).await.unwrap();

        // Verify all jobs were initialized
        let state = scheduler.state_manager.read().await;
        assert_eq!(state.jobs.len(), 3, "Should have 3 jobs in state");

        assert!(state.get_job("daily_backup").is_some());
        assert!(state.get_job("weekly_backup").is_some());
        assert!(state.get_job("monthly_backup").is_some());
    }

    #[tokio::test]
    async fn test_duplicate_prevents_any_initialization() {
        let (scheduler, _temp_dir) = create_test_scheduler().await;

        let jobs = vec![
            create_test_job("job1"),
            create_test_job("job2"),
            create_test_job("job1"), // duplicate
            create_test_job("job3"),
        ];

        // Should fail due to duplicate
        let result = scheduler.initialize_jobs(&jobs).await;
        assert!(result.is_err());

        // Verify no jobs were initialized
        let state = scheduler.state_manager.read().await;
        assert_eq!(state.jobs.len(), 0,
                   "No jobs should be initialized when duplicates are detected");
    }
}