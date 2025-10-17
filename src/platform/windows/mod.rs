pub mod file_ops;
pub mod long_path;
pub mod service;
pub mod service_impl;

use crate::platform::traits::{FileSystem, PathNormalizer};
use anyhow::Result;
use std::path::Path;

pub use long_path::WindowsPathNormalizer;

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

    async fn sync_file(&self, path: &Path) -> Result<()> {
        let path = self.normalizer.normalize(path);
        file_ops::fsync_file(&path).await
    }
}