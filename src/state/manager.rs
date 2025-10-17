use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::models::BackupState;

pub struct StateManager {
    state: Arc<RwLock<BackupState>>,
    state_path: PathBuf,
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
        let state = self.state.read().await;
        self.save_state_atomic(&state).await
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
        let mut state = self.state.write().await;

        if let Some(job) = state.get_job_mut(job_id) {
            updater(job);
            state.last_updated = chrono::Utc::now();
            drop(state); // Release lock before save
            self.save().await?;
        } else {
            warn!("Job not found in state: {}", job_id);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}