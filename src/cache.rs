use eyre::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub size: u64,
    pub mtime: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DirSnapshot {
    pub entries: HashMap<String, FileEntry>,
    pub scanned_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Cache {
    pub config_hash: String,
    pub dirs: HashMap<String, DirSnapshot>,
}

impl Cache {
    /// Load the cache from disk, returning an empty cache on any failure.
    pub fn load() -> Self {
        let path = Self::cache_path();
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save the cache to disk atomically (write to temp file, then rename).
    pub fn save(&self) -> Result<()> {
        let path = Self::cache_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create cache directory")?;
        }

        let tmp_path = path.with_extension("json.tmp");
        let content = serde_json::to_string_pretty(self).context("Failed to serialize cache")?;
        fs::write(&tmp_path, &content).context("Failed to write cache temp file")?;
        fs::rename(&tmp_path, &path).context("Failed to rename cache temp file")?;

        log::info!("Cache saved to {}", path.display());
        Ok(())
    }

    /// Check if a source directory is unchanged since the last scan.
    pub fn is_unchanged(&self, source_dir: &Path) -> bool {
        let key = source_dir.to_string_lossy().to_string();
        let cached = match self.dirs.get(&key) {
            Some(snapshot) => snapshot,
            None => return false,
        };

        let current = match Self::snapshot_dir(source_dir) {
            Ok(snapshot) => snapshot,
            Err(_) => return false,
        };

        if cached.entries.len() != current.entries.len() {
            return false;
        }

        for (name, cached_entry) in &cached.entries {
            match current.entries.get(name) {
                Some(current_entry) => {
                    if cached_entry.size != current_entry.size
                        || cached_entry.mtime != current_entry.mtime
                    {
                        return false;
                    }
                }
                None => return false,
            }
        }

        true
    }

    /// Take a snapshot of all files in a directory (not recursive).
    pub fn snapshot_dir(dir: &Path) -> Result<DirSnapshot> {
        let mut entries = HashMap::new();
        let read_dir =
            fs::read_dir(dir).context(format!("Failed to read directory {}", dir.display()))?;

        for entry in read_dir {
            let entry = entry?;
            let path = entry.path();

            if !path.is_file() || path.is_symlink() {
                continue;
            }

            if let Some(filename) = path.file_name() {
                let meta = fs::metadata(&path)
                    .context(format!("Failed to stat {}", path.display()))?;
                let mtime = meta
                    .modified()
                    .unwrap_or(SystemTime::UNIX_EPOCH)
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                entries.insert(
                    filename.to_string_lossy().to_string(),
                    FileEntry {
                        size: meta.len(),
                        mtime,
                    },
                );
            }
        }

        let scanned_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Ok(DirSnapshot {
            entries,
            scanned_at,
        })
    }

    /// Update the cache for a given directory with a fresh snapshot.
    pub fn update_dir(&mut self, dir: &Path) -> Result<()> {
        let snapshot = Self::snapshot_dir(dir)?;
        self.dirs
            .insert(dir.to_string_lossy().to_string(), snapshot);
        Ok(())
    }

    /// Compute SHA-256 hash of serialized config.
    pub fn hash_config_content(config: &impl serde::Serialize) -> String {
        let content = serde_json::to_string(config).unwrap_or_default();
        let hash = Sha256::digest(content.as_bytes());
        format!("{:x}", hash)
    }

    fn cache_path() -> PathBuf {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from(".cache"))
            .join("kondo")
            .join("state.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn create_test_file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.join(name);
        let mut f = fs::File::create(&path).expect("create test file");
        f.write_all(content).expect("write test file");
        path
    }

    #[test]
    fn test_snapshot_dir_captures_files() {
        let dir = TempDir::new().expect("temp dir");
        create_test_file(dir.path(), "a.txt", b"hello");
        create_test_file(dir.path(), "b.png", b"image data");

        let snapshot = Cache::snapshot_dir(dir.path()).expect("snapshot");
        assert_eq!(snapshot.entries.len(), 2);
        assert!(snapshot.entries.contains_key("a.txt"));
        assert!(snapshot.entries.contains_key("b.png"));
        assert_eq!(snapshot.entries["a.txt"].size, 5);
        assert_eq!(snapshot.entries["b.png"].size, 10);
    }

    #[test]
    fn test_is_unchanged_empty_cache() {
        let cache = Cache::default();
        let dir = TempDir::new().expect("temp dir");
        assert!(!cache.is_unchanged(dir.path()));
    }

    #[test]
    fn test_is_unchanged_after_update() {
        let dir = TempDir::new().expect("temp dir");
        create_test_file(dir.path(), "a.txt", b"hello");

        let mut cache = Cache::default();
        cache.update_dir(dir.path()).expect("update");
        assert!(cache.is_unchanged(dir.path()));
    }

    #[test]
    fn test_is_changed_after_new_file() {
        let dir = TempDir::new().expect("temp dir");
        create_test_file(dir.path(), "a.txt", b"hello");

        let mut cache = Cache::default();
        cache.update_dir(dir.path()).expect("update");

        // Add a new file
        create_test_file(dir.path(), "b.txt", b"world");
        assert!(!cache.is_unchanged(dir.path()));
    }

    #[test]
    fn test_is_changed_after_file_removed() {
        let dir = TempDir::new().expect("temp dir");
        let file = create_test_file(dir.path(), "a.txt", b"hello");

        let mut cache = Cache::default();
        cache.update_dir(dir.path()).expect("update");

        fs::remove_file(&file).expect("remove");
        assert!(!cache.is_unchanged(dir.path()));
    }

    #[test]
    fn test_config_hash_changes() {
        let hash1 = Cache::hash_config_content(&"dashify: true");
        let hash2 = Cache::hash_config_content(&"dashify: false");
        assert_ne!(hash1, hash2);

        let hash3 = Cache::hash_config_content(&"dashify: true");
        assert_eq!(hash1, hash3);
    }
}
