use crate::config::models::WINDOWS_RESERVED;
use crate::core::{validate_backup_job, CopyEngine};
use crate::state::BackupMetadata;
use anyhow::{bail, Context, Result};
use chrono::Utc;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

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

        // Sanitize source name to prevent path invalid characters
        let sanitized_name = Self::sanitize_backup_name(source_name);

        let timestamp = Utc::now().format("%Y-%m-%d_%H%M%S");

        // Add milliseconds to prevent collisions if two backups start in same second
        let millis = Utc::now().timestamp_subsec_millis();

        format!("{}_{}_{:03}", sanitized_name, timestamp, millis)
    }

    /// Sanitize backup name to prevent path invalid filesystem characters
    fn sanitize_backup_name(name: &str) -> String {
        let sanitized = name.chars()
            .map(|c| match c {
                // Path traversal attempts
                '/' | '\\' => '_',
                // Windows invalid characters
                '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
                // Null byte
                '\0' => '_',
                // Control characters
                c if c.is_control() => '_',
                // Leading/trailing dots and spaces
                '.' | ' ' if name.starts_with(c) || name.ends_with(c) => '_',
                // Valid character
                c => c,
            })
            .collect::<String>()
            .trim_matches('_')
            .chars()
            .take(255) // Filename length limit
            .collect::<String>();

        // Check if result is empty
        if sanitized.is_empty() {
            return "backup".to_string();
        }

        let base_name = sanitized
            .split('.')
            .next()
            .unwrap_or(&sanitized)
            .to_lowercase();

        if WINDOWS_RESERVED.contains(&base_name.as_str()) {
            format!("_{}", sanitized)
        } else {
            sanitized
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_backup_name_prevents_path_traversal() {
        // Test ".." attack
        let sanitized = BackupOrchestrator::sanitize_backup_name("..");
        assert_eq!(sanitized, "backup", "Should prevent .. traversal");

        // Test "."
        let sanitized = BackupOrchestrator::sanitize_backup_name(".");
        assert_eq!(sanitized, "backup", "Should prevent . as name");
    }

    #[test]
    fn test_sanitize_backup_name_removes_path_separators() {
        // Test forward slash
        let sanitized = BackupOrchestrator::sanitize_backup_name("path/to/file");
        assert!(!sanitized.contains('/'), "Should remove forward slashes");
        assert_eq!(sanitized, "path_to_file");

        // Test backslash
        let sanitized = BackupOrchestrator::sanitize_backup_name("path\\to\\file");
        assert!(!sanitized.contains('\\'), "Should remove backslashes");
        assert_eq!(sanitized, "path_to_file");
    }

    #[test]
    fn test_sanitize_backup_name_removes_invalid_chars() {
        let invalid_names = vec![
            ("file:name", "file_name"),
            ("file*name", "file_name"),
            ("file?name", "file_name"),
            ("file\"name", "file_name"),
            ("file<name", "file_name"),
            ("file>name", "file_name"),
            ("file|name", "file_name"),
        ];

        for (input, expected) in invalid_names {
            let sanitized = BackupOrchestrator::sanitize_backup_name(input);
            assert_eq!(sanitized, expected, "Failed for input: {}", input);
        }
    }

    #[test]
    fn test_sanitize_backup_name_handles_empty_after_cleaning() {
        // Only invalid characters
        let sanitized = BackupOrchestrator::sanitize_backup_name("////");
        assert_eq!(sanitized, "backup", "Should return 'backup' for empty result");

        // Only dots
        let sanitized = BackupOrchestrator::sanitize_backup_name("...");
        assert_eq!(sanitized, "backup", "Should return 'backup' for only dots");
    }

    #[test]
    fn test_sanitize_backup_name_trims_dots() {
        // Leading dots
        let sanitized = BackupOrchestrator::sanitize_backup_name("...filename");
        assert_eq!(sanitized, "filename", "Should trim leading dots");

        // Trailing dots
        let sanitized = BackupOrchestrator::sanitize_backup_name("filename...");
        assert_eq!(sanitized, "filename", "Should trim trailing dots");

        // Both
        let sanitized = BackupOrchestrator::sanitize_backup_name("...filename...");
        assert_eq!(sanitized, "filename", "Should trim both sides");
    }

    #[test]
    fn test_sanitize_backup_name_removes_control_chars() {
        let name_with_control = "file\x00name\x01test";
        let sanitized = BackupOrchestrator::sanitize_backup_name(name_with_control);
        assert_eq!(sanitized, "file_name_test", "Should remove control characters");
    }

    #[test]
    fn test_sanitize_backup_name_preserves_valid_names() {
        let valid_names = vec![
            "Documents",
            "My_Folder",
            "backup-2024",
            "folder.name",
            "test123",
        ];

        for name in valid_names {
            let sanitized = BackupOrchestrator::sanitize_backup_name(name);
            assert_eq!(sanitized, name, "Should preserve valid name: {}", name);
        }
    }

    #[test]
    fn test_generate_backup_name_security() {
        // Test path traversal attempt
        let malicious_source = Path::new("C:\\Users\\..\\..");
        let backup_name = BackupOrchestrator::generate_backup_name(malicious_source);

        // Should be sanitized to "backup"
        assert!(backup_name.starts_with("backup_"),
                "Should sanitize .. to 'backup': {}", backup_name);
        assert!(!backup_name.contains(".."),
                "Should not contain .. : {}", backup_name);
    }

    #[test]
    fn test_generate_backup_name_with_special_chars() {
        let source = Path::new("C:\\Users\\test\\my:folder*name");
        let backup_name = BackupOrchestrator::generate_backup_name(source);

        // Should replace : and *
        assert!(!backup_name.contains(':'), "Should not contain :");
        assert!(!backup_name.contains('*'), "Should not contain *");
        assert!(backup_name.contains('_'), "Should replace with _");
    }

    #[test]
    fn test_backup_name_format() {
        let source = Path::new("C:\\Users\\Documents");
        let backup_name = BackupOrchestrator::generate_backup_name(source);

        // Should follow format: name_YYYY-MM-DD_HHMMSS_mmm
        let parts: Vec<&str> = backup_name.split('_').collect();
        assert!(parts.len() >= 4, "Should have at least 4 parts: {}", backup_name);

        // Check timestamp format
        assert!(parts[1].contains('-'), "Should have date with dashes");

        // Check milliseconds (3 digits)
        let millis_part = parts.last().unwrap();
        assert_eq!(millis_part.len(), 3, "Milliseconds should be 3 digits");
        assert!(millis_part.chars().all(|c| c.is_numeric()),
                "Milliseconds should be numeric");
    }

    #[test]
    fn test_generate_backup_name_with_unicode() {
        let source = Path::new("C:\\Users\\Documents\\文档");
        let backup_name = BackupOrchestrator::generate_backup_name(source);

        // Should preserve valid unicode
        assert!(backup_name.starts_with("文档_"),
                "Should preserve unicode: {}", backup_name);
    }

    #[test]
    fn test_backup_name_length() {
        let long_name = "a".repeat(300);
        let source = Path::new(&long_name);
        let backup_name = BackupOrchestrator::generate_backup_name(source);

        // Name should be truncated but still valid
        assert!(backup_name.len() <= 300); // 255 + timestamp + micros

        // Should still have valid format
        let parts: Vec<&str> = backup_name.split('_').collect();
        assert!(parts.len() >= 4);
    }

    #[test]
    fn test_backup_name_fallback() {
        // Test with path that has no filename
        let source = Path::new("/");
        let backup_name = BackupOrchestrator::generate_backup_name(source);

        // Should use "backup" as fallback
        assert!(
            backup_name.starts_with("backup_"),
            "Should use 'backup' fallback: {}",
            backup_name
        );
    }

    #[test]
    fn test_backup_name_with_invalid_chars() {
        let source = Path::new("my<project>:test");
        let backup_name = BackupOrchestrator::generate_backup_name(source);

        // Should sanitize invalid characters
        assert!(
            backup_name.starts_with("my_project__test_"),
            "Should sanitize invalid chars: {}",
            backup_name
        );
        assert!(!backup_name.contains('<'));
        assert!(!backup_name.contains('>'));
        assert!(!backup_name.contains(':'));
    }

    #[test]
    fn test_backup_name_with_path_traversal() {
        let source = Path::new("../../../etc/passwd");
        let backup_name = BackupOrchestrator::generate_backup_name(source);

        // Should sanitize path traversal
        assert!(!backup_name.contains(".."));
        assert!(!backup_name.contains('/'));
        assert!(!backup_name.contains('\\'));
    }

    #[test]
    fn test_backup_name_uniqueness() {
        let source = Path::new("test_project");

        // Generate multiple backup names
        let name1 = BackupOrchestrator::generate_backup_name(source);
        std::thread::sleep(std::time::Duration::from_millis(5));
        let name2 = BackupOrchestrator::generate_backup_name(source);

        // Should be different due to microsecond precision
        assert_ne!(
            name1, name2,
            "Backup names should be unique: {} vs {}",
            name1, name2
        );
    }

    #[test]
    fn test_sanitize_windows_reserved_names() {
        // Exact reserved names
        assert_eq!(BackupOrchestrator::sanitize_backup_name("CON"), "_CON");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("con"), "_con");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("PRN"), "_PRN");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("AUX"), "_AUX");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("NUL"), "_NUL");

        // COM ports
        assert_eq!(BackupOrchestrator::sanitize_backup_name("COM1"), "_COM1");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("com5"), "_com5");

        // LPT ports
        assert_eq!(BackupOrchestrator::sanitize_backup_name("LPT1"), "_LPT1");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("lpt9"), "_lpt9");
    }

    #[test]
    fn test_sanitize_windows_reserved_with_extension() {
        // Windows reserves "CON.txt", "PRN.log", etc.
        assert_eq!(BackupOrchestrator::sanitize_backup_name("CON.txt"), "_CON.txt");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("prn.log"), "_prn.log");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("aux.dat"), "_aux.dat");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("COM1.backup"), "_COM1.backup");
    }

    #[test]
    fn test_sanitize_windows_reserved_partial_match() {
        // Should not modify if it's part of a name
        assert_eq!(BackupOrchestrator::sanitize_backup_name("console"), "console");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("printer"), "printer");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("mycon"), "mycon");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("aux_file"), "aux_file");
    }

    #[test]
    fn test_sanitize_windows_reserved_case_insensitive() {
        assert_eq!(BackupOrchestrator::sanitize_backup_name("CoN"), "_CoN");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("PrN"), "_PrN");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("AuX"), "_AuX");
        assert_eq!(BackupOrchestrator::sanitize_backup_name("cOm1"), "_cOm1");
    }
}