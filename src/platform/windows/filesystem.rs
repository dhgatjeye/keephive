use crate::platform::traits::{FileSystem, PathNormalizer};
use crate::platform::windows::file_ops;
use crate::platform::windows::long_path::WindowsPathNormalizer;
use anyhow::Result;
use std::path::Path;

/// Windows-specific filesystem implementation with long path support
pub struct WindowsFileSystem {
    normalizer: WindowsPathNormalizer,
}

impl WindowsFileSystem {
    pub fn new() -> Self {
        Self {
            normalizer: WindowsPathNormalizer,
        }
    }
}

impl Default for WindowsFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystem for WindowsFileSystem {
    async fn copy_file(&self, src: &Path, dst: &Path) -> Result<u64> {
        let src = self.normalizer.normalize(src);
        let dst = self.normalizer.normalize(dst);
        file_ops::copy_file(&src, &dst).await
    }
}