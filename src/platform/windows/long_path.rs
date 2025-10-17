use crate::platform::traits::PathNormalizer;
use std::path::{Path, PathBuf};

/// Windows long path limit
const WINDOWS_MAX_PATH: usize = 260;

/// Windows extended path prefix
const EXTENDED_PATH_PREFIX: &str = r"\\?\";

pub struct WindowsPathNormalizer;

impl PathNormalizer for WindowsPathNormalizer {
    fn normalize(&self, path: &Path) -> PathBuf {
        let path_str = path.to_string_lossy();

        // Already has extended prefix
        if path_str.starts_with(EXTENDED_PATH_PREFIX) {
            return path.to_path_buf();
        }

        // Check if path exceeds windows limit or will likely exceed during operations
        if path_str.len() > WINDOWS_MAX_PATH - 50 {
            // Convert to absolute path first
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .unwrap_or_default()
                    .join(path)
            };

            // Add extended prefix
            let extended = format!("{}{}", EXTENDED_PATH_PREFIX, absolute.display());
            PathBuf::from(extended)
        } else {
            path.to_path_buf()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_path_unchanged() {
        let normalizer = WindowsPathNormalizer;
        let path = Path::new(r"C:\Users\test\file.txt");
        let normalized = normalizer.normalize(path);
        assert_eq!(normalized, path);
    }

    #[test]
    fn test_long_path_gets_prefix() {
        let normalizer = WindowsPathNormalizer;

        let long_path = "C:\\".to_string() + &"verylongdirectoryname\\".repeat(12);
        let path = Path::new(&long_path);
        let normalized = normalizer.normalize(path);

        println!("Original path length: {}", long_path.len());
        println!("Normalized path: {}", normalized.display());

        assert!(normalized.to_string_lossy().starts_with(EXTENDED_PATH_PREFIX));
    }

    #[test]
    fn test_already_prefixed_path_unchanged() {
        let normalizer = WindowsPathNormalizer;
        let prefixed_path = r"\\?\C:\some\long\path";
        let path = Path::new(prefixed_path);
        let normalized = normalizer.normalize(path);
        assert_eq!(normalized.to_string_lossy(), prefixed_path);
    }

    #[test]
    fn test_relative_long_path_becomes_absolute_with_prefix() {
        let normalizer = WindowsPathNormalizer;

        let long_relative = "a\\".to_string() + &"verylongdirectoryname\\".repeat(12);
        let path = Path::new(&long_relative);
        let normalized = normalizer.normalize(path);

        let normalized_str = normalized.to_string_lossy();
        assert!(normalized_str.starts_with(EXTENDED_PATH_PREFIX));

        assert!(normalized_str.contains("\\a\\"));
    }
}