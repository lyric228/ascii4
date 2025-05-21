use std::{path::PathBuf, fs};

pub struct CleanupGuard(PathBuf);

impl CleanupGuard {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = fs::remove_file(&self.0);
        }
    }
}
