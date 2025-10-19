pub mod traits;

#[cfg(windows)]
pub mod windows;

pub use traits::{FileSystem, PathNormalizer};

#[cfg(windows)]
pub use windows::WindowsFileSystem;
