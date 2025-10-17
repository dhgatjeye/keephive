use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, warn};

use super::models::BackupState;

pub struct StateManager {
    state: Arc<RwLock<BackupState>>,
    state_path: PathBuf,
    save_mutex: Arc<Mutex<()>>,
}

impl StateManager {
    /// Create new state manager
    pub async fn new(state_path: PathBuf) -> Result<Self> {
        let state = if state_path.exists() {
            Self::load_state(&state_path).await?
        } else {
            debug!("No existing state found, creating new state");
            BackupState::new()
        };

        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            state_path,
            save_mutex: Arc::new(Mutex::new(())),
        })
    }

    /// Load state from disk
    async fn load_state(path: &Path) -> Result<BackupState> {
        debug!("Loading state from: {}", path.display());

        let content = tokio::fs::read_to_string(path).await
            .context("Failed to read state file")?;

        let state: BackupState = serde_json::from_str(&content)
            .context("Failed to parse state file")?;

        debug!("Loaded state with {} jobs", state.jobs.len());
        Ok(state)
    }

    /// Get read-only access to state
    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, BackupState> {
        self.state.read().await
    }

    /// Get mutable access to state (caller must call save() after modifications)
    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, BackupState> {
        self.state.write().await
    }

    /// Save state to disk with atomic write and fsync
    pub async fn save(&self) -> Result<()> {
        // Acquire save mutex to serialize save operations
        let _save_guard = self.save_mutex.lock().await;

        // Take a snapshot of current state
        let state_snapshot = {
            let state = self.state.read().await;
            state.clone()
        }; // Read lock released here

        // Perform holding the state lock
        self.save_state_atomic(&state_snapshot).await
    }

    /// Atomic state persistence with fsync
    async fn save_state_atomic(&self, state: &BackupState) -> Result<()> {
        let temp_path = self.state_path.with_extension("tmp");

        debug!("Saving state atomically to: {}", self.state_path.display());

        // 1. Write to temporary file
        let json = serde_json::to_string_pretty(state)
            .context("Failed to serialize state")?;

        tokio::fs::write(&temp_path, &json).await
            .context("Failed to write temporary state file")?;

        // 2. fsync temporary file
        let temp_file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&temp_path)
            .await?;

        temp_file.sync_all().await
            .context("Failed to sync temporary state file")?;

        drop(temp_file);

        // 3. Atomic rename
        tokio::fs::rename(&temp_path, &self.state_path).await
            .context("Failed to rename temporary state file")?;

        debug!("State saved successfully");
        Ok(())
    }

    /// Update job state and persist
    pub async fn update_job_state<F>(&self, job_id: &str, updater: F) -> Result<()>
    where
        F: FnOnce(&mut super::models::JobState),
    {
        // Acquire save mutex first to update+save
        let _save_guard = self.save_mutex.lock().await;

        // Now update state
        let state_snapshot = {
            let mut state = self.state.write().await;

            if let Some(job) = state.get_job_mut(job_id) {
                updater(job);
                state.last_updated = chrono::Utc::now();
                state.clone()
            } else {
                drop(state);
                warn!("Job not found in state: {}", job_id);
                return Ok(());
            }
        }; // Write lock released here

        // Save with snapshot while holding save_mutex
        self.save_state_atomic(&state_snapshot).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_state_persistence() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let state_path = dir.path().join("test_state.json");

        let manager = StateManager::new(state_path.clone()).await.unwrap();

        {
            let mut state = manager.write().await;
            state.jobs.push(super::super::models::JobState::new(
                "test_job".to_string(),
                PathBuf::from("C:\\source"),
                PathBuf::from("C:\\target"),
            ));
        }

        manager.save().await.unwrap();

        // Load again and verify
        let manager2 = StateManager::new(state_path).await.unwrap();
        let state = manager2.read().await;

        assert_eq!(state.jobs.len(), 1);
        assert_eq!(state.jobs[0].id, "test_job");
    }

    #[tokio::test]
    async fn test_concurrent_state_updates() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let state_path = dir.path().join("test_concurrent.json");

        let manager = Arc::new(StateManager::new(state_path.clone()).await.unwrap());

        // Initialize multiple jobs
        {
            let mut state = manager.write().await;
            for i in 0..10 {
                state.jobs.push(super::super::models::JobState::new(
                    format!("job_{}", i),
                    PathBuf::from(format!("C:\\source_{}", i)),
                    PathBuf::from(format!("C:\\target_{}", i)),
                ));
            }
        }
        manager.save().await.unwrap();

        // Spawn multiple concurrent update tasks
        let mut handles = vec![];
        for i in 0..10 {
            let manager_clone = Arc::clone(&manager);
            let job_id = format!("job_{}", i);

            let handle = tokio::spawn(async move {
                // Update job state multiple times
                for iteration in 0..5 {
                    manager_clone.update_job_state(&job_id, |js| {
                        js.status = super::super::models::JobStatus::Running {
                            started_at: chrono::Utc::now(),
                        };
                    }).await.unwrap();

                    // Small delay to increase contention
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

                    manager_clone.update_job_state(&job_id, |js| {
                        js.status = super::super::models::JobStatus::Idle;
                        js.last_run = Some(chrono::Utc::now());
                    }).await.unwrap();

                    if iteration % 2 == 0 {
                        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
                    }
                }
            });

            handles.push(handle);
        }

        // Wait for all updates to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify all jobs were updated correctly (no data loss)
        let state = manager.read().await;
        assert_eq!(state.jobs.len(), 10);

        for i in 0..10 {
            let job = state.get_job(&format!("job_{}", i));
            assert!(job.is_some(), "Job {} should exist", i);
            let job = job.unwrap();
            assert_eq!(job.status, super::super::models::JobStatus::Idle);
            assert!(job.last_run.is_some(), "Job {} should have last_run set", i);
        }

        // Reload from disk and verify persistence
        drop(state);
        let manager2 = StateManager::new(state_path).await.unwrap();
        let reloaded_state = manager2.read().await;

        assert_eq!(reloaded_state.jobs.len(), 10);
        for i in 0..10 {
            let job = reloaded_state.get_job(&format!("job_{}", i));
            assert!(job.is_some(), "Reloaded job {} should exist", i);
        }
    }

    #[tokio::test]
    async fn test_update_job_state_atomicity() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let state_path = dir.path().join("test_atomicity.json");

        let manager = StateManager::new(state_path.clone()).await.unwrap();

        // Initialize a job
        {
            let mut state = manager.write().await;
            state.jobs.push(super::super::models::JobState::new(
                "test_job".to_string(),
                PathBuf::from("C:\\source"),
                PathBuf::from("C:\\target"),
            ));
        }
        manager.save().await.unwrap();

        // Update job state
        manager.update_job_state("test_job", |js| {
            js.status = super::super::models::JobStatus::Running {
                started_at: chrono::Utc::now(),
            };
        }).await.unwrap();

        // Immediately reload from disk to verify atomicity
        let manager2 = StateManager::new(state_path).await.unwrap();
        let state = manager2.read().await;

        let job = state.get_job("test_job").unwrap();
        assert!(matches!(job.status, super::super::models::JobStatus::Running { .. }));
    }

    #[tokio::test]
    async fn test_update_nonexistent_job() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let state_path = dir.path().join("test_nonexistent.json");

        let manager = StateManager::new(state_path).await.unwrap();

        // Try to update a job that doesn't exist
        let result = manager.update_job_state("nonexistent", |js| {
            js.status = super::super::models::JobStatus::Idle;
        }).await;

        // Should succeed but do nothing
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_save_mutex_serialization() {
        use tempfile::tempdir;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = tempdir().unwrap();
        let state_path = dir.path().join("test_serialization.json");

        let manager = Arc::new(StateManager::new(state_path.clone()).await.unwrap());

        // Initialize a job
        {
            let mut state = manager.write().await;
            state.jobs.push(super::super::models::JobState::new(
                "test_job".to_string(),
                PathBuf::from("C:\\source"),
                PathBuf::from("C:\\target"),
            ));
        }

        // Track concurrent access - use a counter instead of boolean
        let concurrent_count = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];
        for _ in 0..20 {
            let manager_clone = Arc::clone(&manager);
            let count = Arc::clone(&concurrent_count);
            let max = Arc::clone(&max_concurrent);

            let handle = tokio::spawn(async move {
                // Acquire the save mutex directly to simulate what happens in save()
                let _guard = manager_clone.save_mutex.lock().await;

                let current = count.fetch_add(1, Ordering::SeqCst) + 1;

                max.fetch_max(current, Ordering::SeqCst);

                tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

                count.fetch_sub(1, Ordering::SeqCst);

                // Drop guard to release mutex
                drop(_guard);
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // With proper serialization, max_concurrent should never exceed 1
        let max_seen = max_concurrent.load(Ordering::SeqCst);
        assert_eq!(max_seen, 1,
                   "Detected {} concurrent operations", max_seen);

        // Verify final state
        let state = manager.read().await;
        assert_eq!(state.jobs.len(), 1);
    }

    #[tokio::test]
    async fn test_concurrent_reads_with_updates() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let state_path = dir.path().join("test_concurrent_reads.json");

        let manager = Arc::new(StateManager::new(state_path.clone()).await.unwrap());

        // Initialize jobs
        {
            let mut state = manager.write().await;
            for i in 0..5 {
                state.jobs.push(super::super::models::JobState::new(
                    format!("job_{}", i),
                    PathBuf::from(format!("C:\\source_{}", i)),
                    PathBuf::from(format!("C:\\target_{}", i)),
                ));
            }
        }
        manager.save().await.unwrap();

        let mut handles = vec![];

        // Spawn readers
        for _ in 0..10 {
            let manager_clone = Arc::clone(&manager);
            let handle = tokio::spawn(async move {
                for _ in 0..20 {
                    let state = manager_clone.read().await;
                    assert_eq!(state.jobs.len(), 5);
                    tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
                }
            });
            handles.push(handle);
        }

        // Spawn writers
        for i in 0..5 {
            let manager_clone = Arc::clone(&manager);
            let job_id = format!("job_{}", i);
            let handle = tokio::spawn(async move {
                for _ in 0..10 {
                    manager_clone.update_job_state(&job_id, |js| {
                        js.last_run = Some(chrono::Utc::now());
                    }).await.unwrap();
                    tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;
                }
            });
            handles.push(handle);
        }

        // Wait for all
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify final state
        let state = manager.read().await;
        assert_eq!(state.jobs.len(), 5);
        for i in 0..5 {
            let job = state.get_job(&format!("job_{}", i)).unwrap();
            assert!(job.last_run.is_some());
        }
    }
}