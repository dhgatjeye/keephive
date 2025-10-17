use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::warn;

#[derive(Debug, Clone)]
pub struct CopyProgress {
    pub bytes_copied: u64,
    pub files_copied: u64,
    pub files_skipped: u64,
    pub current_file: Option<PathBuf>,
}

pub struct CopyEngine {
    buffer_size: usize,
}

impl CopyEngine {
    pub fn new() -> Self {
        Self {
            buffer_size: 1024 * 1024, // 1MB buffer
        }
    }

    /// Copy entire directory tree with progress tracking
    pub async fn copy_directory<F>(
        &self,
        source: &Path,
        target: &Path,
        mut progress_callback: F,
    ) -> Result<CopyProgress>
    where
        F: FnMut(CopyProgress) + Send,
    {
        let mut progress = CopyProgress {
            bytes_copied: 0,
            files_copied: 0,
            files_skipped: 0,
            current_file: None,
        };

        self.copy_dir_recursive(source, target, source, &mut progress, &mut progress_callback).await?;

        Ok(progress)
    }

    /// Recursive directory copy
    fn copy_dir_recursive<'a, F>(
        &'a self,
        source_root: &'a Path,
        target_root: &'a Path,
        current_source: &'a Path,
        progress: &'a mut CopyProgress,
        progress_callback: &'a mut F,
    ) -> std::pin::Pin<Box<dyn Future<Output=Result<()>> + Send + 'a>>
    where
        F: FnMut(CopyProgress) + Send,
    {
        Box::pin(async move {
            let mut entries = tokio::fs::read_dir(current_source).await
                .context("Failed to read source directory")?;

            while let Some(entry) = entries.next_entry().await? {
                let source_path = entry.path();

                // Calculate relative path for target
                let relative_path = source_path.strip_prefix(source_root)
                    .context("Failed to calculate relative path")?;
                let target_path = target_root.join(relative_path);

                let metadata = match entry.metadata().await {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("Cannot access file metadata {}: {}", source_path.display(), e);
                        progress.files_skipped += 1;
                        continue;
                    }
                };

                if metadata.is_dir() {
                    // Create target directory
                    tokio::fs::create_dir_all(&target_path).await
                        .context("Failed to create target directory")?;

                    // Recurse into subdirectory
                    self.copy_dir_recursive(
                        source_root,
                        target_root,
                        &source_path,
                        progress,
                        progress_callback,
                    ).await?;
                } else if metadata.is_file() {
                    // Copy file
                    progress.current_file = Some(source_path.clone());

                    match self.copy_file(&source_path, &target_path).await {
                        Ok(bytes) => {
                            progress.bytes_copied += bytes;
                            progress.files_copied += 1;
                            progress_callback(progress.clone());
                        }
                        Err(e) => {
                            warn!("Failed to copy file {}: {}", source_path.display(), e);
                            progress.files_skipped += 1;
                        }
                    }
                }
            }

            Ok(())
        })
    }

    /// Copy single file with streaming
    async fn copy_file(&self, source: &Path, target: &Path) -> Result<u64> {
        // Ensure parent directory exists
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut src_file = tokio::fs::File::open(source).await
            .context("Failed to open source file")?;

        let mut dst_file = tokio::fs::File::create(target).await
            .context("Failed to create destination file")?;

        let mut buffer = vec![0u8; self.buffer_size];
        let mut total_bytes = 0u64;

        loop {
            let bytes_read = src_file.read(&mut buffer).await
                .context("Failed to read from source")?;

            if bytes_read == 0 {
                break;
            }

            dst_file.write_all(&buffer[..bytes_read]).await
                .context("Failed to write to destination")?;

            total_bytes += bytes_read as u64;
        }

        // Sync file to disk
        dst_file.sync_all().await
            .context("Failed to sync destination file")?;

        // Copy metadata
        self.copy_metadata(source, target).await?;

        Ok(total_bytes)
    }

    /// Copy file metadata
    async fn copy_metadata(&self, source: &Path, target: &Path) -> Result<()> {
        let metadata = tokio::fs::metadata(source).await?;

        #[cfg(windows)]
        {
            use std::fs::OpenOptions;
            let file = OpenOptions::new().write(true).open(target)?;
            if let Ok(modified) = metadata.modified() {
                let _ = file.set_modified(modified);
            }
        }

        Ok(())
    }
}

impl Default for CopyEngine {
    fn default() -> Self {
        Self::new()
    }
}