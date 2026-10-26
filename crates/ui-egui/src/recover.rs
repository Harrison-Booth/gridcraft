//! AutoRecover: keeps a recovery copy of every workbook with unsaved changes in a folder, written
//! on a worker thread ([`gridcraft_engine::recover`]), and reopens what a crash left behind.
//!
//! The app sets the folder (the settings folder's `recovery`); with none (tests, the browser,
//! `GRIDCRAFT_NO_PREFS`) nothing is written.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gridcraft_engine::model::Workbook;
use gridcraft_engine::recover::{self, RecoverJob};

/// Seconds between a change and its recovery copy (Excel's default is 10 minutes; this one is
/// cheap because the snapshot is shared and the write is off the UI thread).
const GATE_MS: f64 = 10_000.0;

type Written = Vec<(u64, Arc<Workbook>)>;

#[derive(Default)]
pub struct Recovery {
    pub dir: Option<PathBuf>,
    /// The snapshot of each document that has a copy on disk.
    written: HashMap<u64, Arc<Workbook>>,
    last: f64,
    worker: Option<std::thread::JoinHandle<Written>>,
}

fn write_all(jobs: Vec<RecoverJob>, dir: PathBuf) -> Written {
    let mut done = vec![];
    for job in jobs {
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.run(&dir)));
        match run {
            Ok(Ok(_)) => done.push((job.uid, job.wb)),
            Ok(Err(e)) => log::warn!("AutoRecover could not write a copy: {e}"),
            Err(_) => log::warn!("AutoRecover failed unexpectedly"),
        }
    }
    done
}

impl Recovery {
    /// One frame's work: collect a finished write, delete the copies of documents that were saved
    /// or closed, and start a write for the ones that changed once the gate is open.
    pub fn tick(&mut self, session: &gridcraft_engine::Session, ctx: &egui::Context, now: f64) {
        let Some(dir) = self.dir.clone() else { return };
        if let Some(w) = self.worker.take() {
            if !w.is_finished() {
                self.worker = Some(w);
                return; // The worker wakes the window when it is done.
            }
            self.written.extend(w.join().unwrap_or_default());
            self.last = now;
        }
        let docs = session.documents();
        self.written.retain(|uid, _| {
            let keep = docs.iter().any(|d| d.uid == *uid && d.is_dirty());
            if !keep {
                recover::discard(&dir, *uid);
            }
            keep
        });
        let jobs: Vec<RecoverJob> =
            session.recover_jobs().into_iter().filter(|j| !self.written.get(&j.uid).is_some_and(|w| Arc::ptr_eq(w, &j.wb))).collect();
        if jobs.is_empty() {
            return;
        }
        let wait = self.last + GATE_MS - now;
        if wait > 0.0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(wait as u64 + 1));
            return;
        }
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("autorecover".into()).spawn(move || {
            let done = write_all(jobs, dir);
            ctx.request_repaint();
            done
        });
        match spawned {
            Ok(h) => self.worker = Some(h),
            Err(e) => log::warn!("AutoRecover could not start a worker thread: {e}"),
        }
        self.last = now;
    }

    /// Quitting: waits for a running write, then deletes every copy this process made (the user
    /// has just been asked about unsaved work, so nothing is left to recover).
    pub fn finish(&mut self) {
        let Some(dir) = self.dir.clone() else { return };
        if let Some(w) = self.worker.take() {
            self.written.extend(w.join().unwrap_or_default());
        }
        for uid in self.written.keys() {
            recover::discard(&dir, *uid);
        }
        self.written.clear();
    }

    /// Opens what an earlier crashed run left behind, writes fresh copies of it, then deletes the
    /// old ones, so a second crash straight away loses nothing. Returns how many were reopened.
    pub fn restore(&mut self, session: &mut gridcraft_engine::Session) -> usize {
        let Some(dir) = self.dir.clone() else { return 0 };
        let mut reopened = vec![];
        for r in recover::scan(&dir) {
            match session.open_recovered(&r) {
                Ok(_) => reopened.push(r),
                Err(e) => log::warn!("AutoRecover could not reopen {}: {e}", r.title),
            }
        }
        for j in session.recover_jobs() {
            if j.run(&dir).is_ok() {
                self.written.insert(j.uid, j.wb);
            }
        }
        if self.written.len() >= reopened.len() {
            reopened.iter().for_each(recover::Recoverable::remove);
        }
        reopened.len()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used)]
mod tests {
    use gridcraft_engine::Session;
    use serde_json::json;

    use super::*;

    fn frame(r: &mut Recovery, s: &Session, now: f64) {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ui| r.tick(s, ui.ctx(), now));
        output.textures_delta.clear();
    }

    fn settle(r: &mut Recovery) {
        if let Some(w) = r.worker.take() {
            r.written.extend(w.join().unwrap());
        }
    }

    fn files(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
    }

    #[test]
    fn copies_follow_the_edits_and_vanish_on_save_or_quit() {
        let dir = std::env::temp_dir().join(format!("gridcraft-recover-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::new();
        s.new_workbook();
        let mut r = Recovery { dir: Some(dir.clone()), ..Default::default() };
        frame(&mut r, &s, 20_000.0);
        assert_eq!(files(&dir), 0, "nothing changed");
        s.execute("cell.set", json!({"cell": "A1", "input": "one"})).unwrap();
        frame(&mut r, &s, 20_000.0);
        settle(&mut r);
        assert_eq!(files(&dir), 2, "a never-saved workbook gets a copy");
        s.execute("cell.set", json!({"cell": "A1", "input": "two"})).unwrap();
        frame(&mut r, &s, 21_000.0);
        assert!(r.worker.is_none(), "inside the gate no new write starts");
        frame(&mut r, &s, 31_000.0);
        settle(&mut r);
        assert!(Arc::ptr_eq(r.written.values().next().unwrap(), &s.active().unwrap().wb), "the copy holds the latest edit");
        // Saving makes the document clean: its copy goes.
        let uid = s.active().unwrap().uid;
        s.active_mut().unwrap().saved = s.active().unwrap().wb.clone();
        frame(&mut r, &s, 42_000.0);
        assert_eq!((files(&dir), r.written.contains_key(&uid)), (0, false));
        // Quitting deletes what is left.
        s.execute("cell.set", json!({"cell": "A1", "input": "three"})).unwrap();
        frame(&mut r, &s, 60_000.0);
        r.finish();
        assert_eq!(files(&dir), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The soak: a process "killed" at any point of its life (its files stay, owned by a process
    /// that no longer exists) always restarts with the last edit that had a copy.
    #[test]
    fn a_kill_at_any_point_restores_the_last_copy() {
        let dir = std::env::temp_dir().join(format!("gridcraft-recover-kill-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for round in 0..30u32 {
            let mut s = Session::new();
            s.new_workbook();
            let mut r = Recovery { dir: Some(dir.clone()), ..Default::default() };
            let mut copied = None;
            for step in 0..=round % 7 + 1 {
                let text = format!("r{round}s{step}");
                s.execute("cell.set", json!({"cell": "A1", "input": text})).unwrap();
                if step % 2 == 0 {
                    frame(&mut r, &s, f64::from(step + 1) * 20_000.0);
                    settle(&mut r);
                    copied = Some(text);
                }
            }
            // Kill: no `finish`. Make the files look like they belong to a dead process.
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let n = e.file_name().to_string_lossy().replacen(&std::process::id().to_string(), "4000000000", 1);
                std::fs::rename(e.path(), dir.join(n)).unwrap();
            }
            let mut next = Session::new();
            let mut again = Recovery { dir: Some(dir.clone()), ..Default::default() };
            assert_eq!(again.restore(&mut next), 1);
            let a1 = next.documents().last().unwrap().wb.sheet(0).unwrap().value(gridcraft_engine::core::CellRef::new(0, 0));
            assert_eq!(a1, gridcraft_engine::core::Value::text(copied.unwrap()), "round {round}");
            again.finish();
            assert_eq!(files(&dir), 0);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
