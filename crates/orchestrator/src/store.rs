//! Crash-safe on-disk persistence of runs and their event logs.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::error::OrchestratorError;
use crate::model::{Run, RunEvent};
use crate::Result;

/// Result of [`RunStore::list`]: parsed runs plus warnings for entries that
/// could not be read.
#[derive(Debug, Default)]
pub struct ListResult {
    /// Successfully loaded runs, sorted by creation time.
    pub runs: Vec<Run>,
    /// One message per unreadable run directory.
    pub warnings: Vec<String>,
}

/// Stores runs under `<runs_dir>/<run_id>/{run.json,events.jsonl,logs/}`.
#[derive(Debug, Clone)]
pub struct RunStore {
    runs_dir: PathBuf,
}

impl RunStore {
    /// Creates a store rooted at `runs_dir` (created lazily).
    pub fn new(runs_dir: impl Into<PathBuf>) -> Self {
        RunStore {
            runs_dir: runs_dir.into(),
        }
    }

    /// Directory holding one run's files.
    pub fn run_dir(&self, run_id: &str) -> Result<PathBuf> {
        if run_id.is_empty()
            || run_id == "."
            || run_id.contains(['/', '\\', '\0'])
            || run_id.contains("..")
        {
            return Err(OrchestratorError::InvalidRunId(run_id.to_string()));
        }
        Ok(self.runs_dir.join(run_id))
    }

    /// Atomically writes `run.json` (tmp file, fsync, rename) and ensures
    /// the `logs/` directory exists.
    pub fn save(&self, run: &Run) -> Result<()> {
        let dir = self.run_dir(&run.id)?;
        fs::create_dir_all(dir.join("logs"))?;
        let tmp = dir.join("run.json.tmp");
        let data = serde_json::to_vec_pretty(run)?;
        {
            let mut f = File::create(&tmp)?;
            f.write_all(&data)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, dir.join("run.json"))?;
        if let Ok(d) = File::open(&dir) {
            let _ = d.sync_all();
        }
        Ok(())
    }

    /// Appends one JSON line to `events.jsonl`.
    pub fn append_event(&self, run_id: &str, event: &RunEvent) -> Result<()> {
        let dir = self.run_dir(run_id)?;
        fs::create_dir_all(dir.join("logs"))?;
        let mut line = serde_json::to_vec(event)?;
        line.push(b'\n');
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("events.jsonl"))?;
        f.write_all(&line)?;
        f.sync_data()?;
        Ok(())
    }

    /// Loads a run by id.
    pub fn load(&self, run_id: &str) -> Result<Run> {
        let path = self.run_dir(run_id)?.join("run.json");
        match fs::read(&path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(OrchestratorError::RunNotFound(run_id.to_string()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Lists all runs, skipping unreadable ones with a warning.
    pub fn list(&self) -> Result<ListResult> {
        let mut out = ListResult::default();
        let rd = match fs::read_dir(&self.runs_dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for entry in rd {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            match self.load(&name) {
                Ok(run) => out.runs.push(run),
                Err(e) => out.warnings.push(format!("skipping run {name}: {e}")),
            }
        }
        out.runs.sort_by_key(|r| r.created_at);
        Ok(out)
    }

    /// Reads all events; a malformed (e.g. torn final) line is skipped.
    pub fn events(&self, run_id: &str) -> Result<Vec<RunEvent>> {
        let path = self.run_dir(run_id)?.join("events.jsonl");
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        Ok(text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect())
    }
}
