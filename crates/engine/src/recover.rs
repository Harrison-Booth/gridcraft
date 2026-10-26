//! AutoRecover: a copy of every workbook with unsaved changes, kept in a recovery folder, so a
//! crash, a kill or a power cut loses minutes of work instead of all of it. This covers what
//! AutoSave ([`crate::autosave`]) can't: workbooks never saved to a file, and saved ones while
//! AutoSave is off. The user's own files are never touched.
//!
//! Each document gets `<process id>-<document uid>.xlsx` plus a `.json` note with its title and
//! original path. A clean exit, a save or closing the document deletes them; whatever is left
//! belongs to a process that died, and [`scan`] finds it for the next start to reopen.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gridcraft_model::Workbook;
use serde_json::{Value as Json, json};

use crate::{DocState, EngineError, Result, Session, Trust, io};

/// A snapshot of one dirty document, writable from any thread.
#[derive(Clone, Debug)]
pub struct RecoverJob {
    pub uid: u64,
    /// The snapshot, kept to tell whether the document has changed since.
    pub wb: Arc<Workbook>,
    title: String,
    path: Option<String>,
}

fn stem(dir: &Path, pid: u32, uid: u64) -> PathBuf {
    dir.join(format!("{pid}-{uid}"))
}

fn io_err(e: std::io::Error) -> EngineError {
    EngineError::Other(format!("recovery folder: {e}"))
}

impl RecoverJob {
    /// Writes the note, then the workbook (each through a temporary file and a rename), so a kill
    /// at any point leaves the previous copy or the new one, never a half-written file.
    pub fn run(&self, dir: &Path) -> Result<usize> {
        std::fs::create_dir_all(dir).map_err(io_err)?;
        let base = stem(dir, std::process::id(), self.uid);
        let note = json!({"title": self.title, "path": self.path});
        let note_path = base.with_extension("json");
        io::write_file(&note_path.to_string_lossy(), note.to_string().as_bytes())?;
        let bytes = io::save_bytes_as(&self.wb, "recovered.xlsx", io::FileKind::Xlsx)?;
        io::write_file(&base.with_extension("xlsx").to_string_lossy(), &bytes)?;
        Ok(bytes.len())
    }
}

/// Deletes this process's recovery files for a document (it was saved, closed, or we are quitting).
pub fn discard(dir: &Path, uid: u64) {
    let base = stem(dir, std::process::id(), uid);
    for ext in ["xlsx", "json"] {
        let _ = std::fs::remove_file(base.with_extension(ext));
    }
}

/// A recovery copy left behind by a process that no longer runs.
#[derive(Clone, Debug)]
pub struct Recoverable {
    pub title: String,
    pub path: Option<String>,
    xlsx: PathBuf,
    note: PathBuf,
}

impl Recoverable {
    /// Deletes the copy (after it was reopened, or the user declined it).
    pub fn remove(&self) {
        let _ = std::fs::remove_file(&self.xlsx);
        let _ = std::fs::remove_file(&self.note);
    }
}

/// Whether the process that wrote `pid-…` still runs. Only Linux can tell cheaply without
/// `unsafe`; elsewhere every copy counts as left behind.
// shortcut: two GridCraft windows as separate processes on macOS/Windows can reopen each
// other's live copies; upgrade to a lock file per process if that is reported.
fn alive(pid: u32) -> bool {
    pid == std::process::id() || (cfg!(target_os = "linux") && Path::new(&format!("/proc/{pid}")).exists())
}

/// The recovery copies in `dir` whose process is gone, oldest first. Temporary files of a write
/// that a kill cut short are deleted.
pub fn scan(dir: &Path) -> Vec<Recoverable> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut found = vec![];
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let Some((pid, _)) = name.split_once('-') else { continue };
        let Ok(pid) = pid.parse::<u32>() else { continue };
        if alive(pid) {
            continue;
        }
        if name.ends_with(".sctmp") {
            let _ = std::fs::remove_file(&p);
            continue;
        }
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let xlsx = p.with_extension("xlsx");
        let note: Json = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Json::Null);
        let text = |k: &str| note.get(k).and_then(Json::as_str).map(str::to_string);
        if xlsx.is_file() {
            let title = text("title").unwrap_or_else(|| "Recovered".into());
            found.push(Recoverable { title, path: text("path"), xlsx, note: p });
        } else {
            // A note whose workbook was never finished: nothing to recover.
            let _ = std::fs::remove_file(&p);
        }
    }
    found.sort_by_key(|r| std::fs::metadata(&r.xlsx).and_then(|m| m.modified()).ok());
    found
}

impl Session {
    /// A job for every document with unsaved changes (the active view state is stored by the
    /// caller first). The recovered copy of a document that has a file is titled after it.
    pub fn recover_jobs(&self) -> Vec<RecoverJob> {
        let dirty = self.docs.iter().filter(|d| d.is_dirty());
        dirty.map(|d| RecoverJob { uid: d.uid, wb: d.wb.clone(), title: d.title.clone(), path: d.path.clone() }).collect()
    }

    /// Opens a recovery copy as a new workbook with no file (Save asks where to put it) that
    /// counts as having unsaved changes. Returns its document index.
    pub fn open_recovered(&mut self, r: &Recoverable) -> Result<usize> {
        let bytes = io::read_file(&r.xlsx.to_string_lossy())?;
        let (wb, _) = io::open_bytes("recovered.xlsx", &bytes)?;
        let title = format!("{} (recovered)", r.title);
        let mut d = DocState::new(wb, None, title, Trust::FromFile);
        d.saved = Arc::new(Workbook::new());
        Ok(self.add_document(d))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gridcraft-recover-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Pretends this process died: its files are renamed to a process id that isn't running.
    fn orphan(dir: &Path) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let n = e.file_name().to_string_lossy().replacen(&std::process::id().to_string(), "4000000000", 1);
            std::fs::rename(e.path(), dir.join(n)).unwrap();
        }
    }

    #[test]
    fn never_saved_workbook_comes_back_after_a_crash() {
        let dir = temp("crash");
        let mut s = Session::new();
        s.new_workbook();
        assert!(s.recover_jobs().is_empty(), "nothing changed, nothing to keep");
        s.execute("cell.set", json!({"cell": "B2", "input": "=1+2"})).unwrap();
        let jobs = s.recover_jobs();
        assert_eq!(jobs.len(), 1);
        jobs[0].run(&dir).unwrap();
        assert!(scan(&dir).is_empty(), "this process is alive: its copies aren't left behind");
        orphan(&dir);
        let found = scan(&dir);
        assert_eq!(found.len(), 1);
        let mut next = Session::new();
        let i = next.open_recovered(&found[0]).unwrap();
        let d = &next.documents()[i];
        assert!(d.is_dirty() && d.path.is_none(), "recovered work is unsaved and asks where to save");
        assert!(d.title.ends_with("(recovered)"));
        let sheet = d.wb.sheet(0).unwrap();
        assert_eq!(sheet.value(gridcraft_core::CellRef::new(1, 1)), gridcraft_core::Value::Number(3.0));
        found[0].remove();
        assert!(scan(&dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saving_or_closing_discards_the_copy_and_a_cut_short_write_is_ignored() {
        let dir = temp("discard");
        let mut s = Session::new();
        s.new_workbook();
        s.execute("cell.set", json!({"cell": "A1", "input": "x"})).unwrap();
        let job = s.recover_jobs().remove(0);
        job.run(&dir).unwrap();
        discard(&dir, job.uid);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        // A kill between the note and the workbook, and one inside the temporary file.
        job.run(&dir).unwrap();
        orphan(&dir);
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            if e.path().extension().is_some_and(|x| x == "xlsx") {
                std::fs::rename(e.path(), e.path().with_extension("xlsx.sctmp")).unwrap();
            }
        }
        assert!(scan(&dir).is_empty());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the half-written leftovers are cleaned up");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
