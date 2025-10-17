use anyhow::{bail, Context, Result};
use chrono::Utc;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::core::{validate_backup_job, CopyEngine};
use crate::state::BackupMetadata;

pub struct BackupOrchestrator {
    copy_engine: CopyEngine,
}

impl BackupOrchestrator {
    pub fn new() -> Self {
        Self {
            copy_engine: CopyEngine::new(),
        }
    }

    /// Execute backup with crash recovery support
    pub async fn execute_backup(
        &self,
        job_id: &str,
        source: &Path,
        target: &Path,
        cancellation: CancellationToken,
    ) -> Result<BackupMetadata> {
        info!("Starting backup: {} ({} -> {})", job_id, source.display(), target.display());

        // Prerequisites validation
        let validation = validate_backup_job(source, target).await?;

        if !validation.is_valid {
            bail!("Backup validation failed");
        }

        for warning in &validation.warnings {
            warn!("Validation warning: {}", warning);
        }

        // Create backup directory with timestamp
        let backup_name = Self::generate_backup_name(source);
        let backup_path = target.join(&backup_name);

        // Check for existing backup (crash recovery scenario)
        if backup_path.exists() {
            warn!("Backup directory already exists, removing: {}", backup_path.display());
            tokio::fs::remove_dir_all(&backup_path).await?;
        }

        tokio::fs::create_dir_all(&backup_path).await
            .context("Failed to create backup directory")?;

        let mut metadata = BackupMetadata::new(backup_name.clone(), backup_path.clone());

        // Execute copy with cancellation support
        let copy_result = tokio::select! {
            result = self.copy_with_progress(source, &backup_path, &mut metadata) => result,
            _ = cancellation.cancelled() => {
                warn!("Backup cancelled for job: {}", job_id);
                self.mark_partial(&backup_path).await?;
                bail!("Backup cancelled");
            }
        };

        match copy_result {
            Ok(_) => {
                metadata.mark_complete();
                info!("Backup completed: {} ({} files, {} bytes)",
                    job_id, metadata.files_copied, metadata.bytes_copied);
            }
            Err(e) => {
                error!("Backup failed: {}", e);
                self.mark_partial(&backup_path).await?;
                return Err(e);
            }
        }

        Ok(metadata)
    }

    /// Copy with progress tracking
    async fn copy_with_progress(
        &self,
        source: &Path,
        backup_path: &Path,
        metadata: &mut BackupMetadata,
    ) -> Result<()> {
        let progress = self.copy_engine.copy_directory(
            source,
            backup_path,
            |p| {
                metadata.bytes_copied = p.bytes_copied;
                metadata.files_copied = p.files_copied;
                metadata.files_skipped = p.files_skipped;
            },
        ).await?;

        metadata.bytes_copied = progress.bytes_copied;
        metadata.files_copied = progress.files_copied;
        metadata.files_skipped = progress.files_skipped;

        Ok(())
    }

    /// Mark backup as partial by renaming directory
    async fn mark_partial(&self, backup_path: &Path) -> Result<()> {
        let partial_name = format!("{}_PARTIAL", backup_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("backup"));

        let partial_path = backup_path.with_file_name(partial_name);

        tokio::fs::rename(backup_path, &partial_path).await
            .context("Failed to mark backup as partial")?;

        warn!("Marked backup as PARTIAL: {}", partial_path.display());
        Ok(())
    }

    /// Generate backup directory name with sortable timestamp
    fn generate_backup_name(source: &Path) -> String {
        let source_name = source.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("backup");

        let timestamp = Utc::now().format("%Y-%m-%d_%H%M%S");
        format!("{}_{}", source_name, timestamp)
    }

    /// Detect and handle partial backups on startup
    pub async fn detect_partial_backups(target: &Path) -> Result<Vec<PathBuf>> {
        let mut partial_backups = Vec::new();

        if !target.exists() {
            return Ok(partial_backups);
        }

        let mut entries = tokio::fs::read_dir(target).await?;

        while let Some(entry) = entries.next_entry().await? {
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with("_PARTIAL") {
                    partial_backups.push(entry.path());
                }
            }
        }

        if !partial_backups.is_empty() {
            warn!("Found {} partial backups", partial_backups.len());
        }

        Ok(partial_backups)
    }

    /// Clean old backups keeping only the specified retention count
    pub async fn cleanup_old_backups(target: &Path, retention_count: usize) -> Result<()> {
        let mut backups = Vec::new();

        let mut entries = tokio::fs::read_dir(target).await?;

        while let Some(entry) = entries.next_entry().await? {
            if let Some(name) = entry.file_name().to_str() {
                // Skip partial backups and state files
                if name.ends_with("_PARTIAL") || name.starts_with(".keephive") {
                    continue;
                }

                if let Ok(metadata) = entry.metadata().await {
                    if metadata.is_dir() {
                        backups.push((entry.path(), metadata.modified().ok()));
                    }
                }
            }
        }

        // Sort by modification time (newest first)
        backups.sort_by(|a, b| b.1.cmp(&a.1));

        // Remove old backups beyond retention count
        if backups.len() > retention_count {
            for (path, _) in backups.iter().skip(retention_count) {
                info!("Removing old backup: {}", path.display());
                tokio::fs::remove_dir_all(path).await
                    .context("Failed to remove old backup")?;
            }
        }

        Ok(())
    }
}

impl Default for BackupOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}