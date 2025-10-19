use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::warn;

use crate::platform::traits::FileSystem;

#[cfg(windows)]
use crate::platform::WindowsFileSystem;

#[derive(Debug, Clone)]
pub struct CopyProgress {
    pub bytes_copied: u64,
    pub files_copied: u64,
    pub files_skipped: u64,
    pub current_file: Option<PathBuf>,
}

pub struct CopyEngine {
    #[cfg(windows)]
    fs: WindowsFileSystem,
}

impl Default for CopyEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl CopyEngine {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            fs: WindowsFileSystem::new(),
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
        F: FnMut(&CopyProgress) + Send,
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
        F: FnMut(&CopyProgress) + Send,
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

                    // Ensure parent directory exists
                    if let Some(parent) = target_path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }

                    // Use platform-specific FileSystem trait
                    #[cfg(windows)]
                    let copy_result = self.fs.copy_file(&source_path, &target_path).await;

                    #[cfg(not(windows))]
                    let copy_result = tokio::fs::copy(&source_path, &target_path).await
                        .map_err(|e| anyhow::anyhow!("Failed to copy file: {}", e));

                    match copy_result {
                        Ok(bytes) => {
                            progress.bytes_copied += bytes;
                            progress.files_copied += 1;
                            progress_callback(&*progress);
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
}