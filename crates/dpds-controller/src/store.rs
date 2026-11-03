//! Persistence of the stored state: one JSON file, written atomically.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::model::StoredState;

pub const STATE_FILE_NAME: &str = "state.json";

#[derive(Debug, Clone)]
pub struct StoreError(String);

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

fn file(dir: &Path) -> PathBuf {
    dir.join(STATE_FILE_NAME)
}

/// A missing directory or file is the default state; a corrupt file (or one with
/// the old paired-link link shape) is an error naming the file, and left untouched.
pub fn load(dir: &Path) -> Result<StoredState, StoreError> {
    let path = file(dir);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(StoredState::default()),
        Err(e) => return Err(StoreError(format!("cannot read {}: {e}", path.display()))),
    };
    serde_json::from_slice(&bytes)
        .map_err(|e| StoreError(format!("{} is not a valid state file: {e}", path.display())))
}

/// Writes to a temporary file in the same directory, syncs and renames it over
/// the state file.
pub fn save(dir: &Path, state: &StoredState) -> Result<(), StoreError> {
    let path = file(dir);
    let err = |what: &str, e: std::io::Error| StoreError(format!("{what} {}: {e}", path.display()));
    fs::create_dir_all(dir).map_err(|e| err("cannot create the directory of", e))?;
    let tmp = dir.join(format!(".{STATE_FILE_NAME}.tmp"));
    let json = serde_json::to_vec_pretty(state)
        .map_err(|e| StoreError(format!("cannot serialise the state: {e}")))?;
    let write = || -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
        fs::rename(&tmp, &path)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        err("cannot write", e)
    })
}
