//! Apps going on and off as an app store does it: Install starts at once,
//! the app's own button says how it's going, and several wait their turn.
//! Each is made alone: the file is applied as it was last applied with just
//! that app's line changed, and any other changes waiting in it are put
//! back after, still waiting for Apply.

use std::path::{Path, PathBuf};
use std::time::Instant;

use futures::StreamExt;
use gpui::Context;
use serde_json::{Value, json};

use crate::nami::{self, Said};
use crate::{Shell, installs};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Install,
    Remove,
    /// Another version of one that's there.
    Change,
}

impl Kind {
    pub fn verb(self) -> &'static str {
        match self {
            Kind::Install => "Installing",
            Kind::Remove => "Removing",
            Kind::Change => "Updating",
        }
    }

    /// "Couldn't install GIMP".
    pub fn plain(self) -> &'static str {
        match self {
            Kind::Install => "install",
            Kind::Remove => "remove",
            Kind::Change => "update",
        }
    }

    /// "GIMP is installed."
    pub fn done(self) -> &'static str {
        match self {
            Kind::Install => "installed",
            Kind::Remove => "removed",
            Kind::Change => "updated",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum State {
    Waiting,
    Going,
    /// What nami said last.
    Failed(String),
    Done,
}

pub struct Job {
    pub id: u64,
    pub attr: String,
    /// The file it's listed in (yours, or the computer's).
    pub file: PathBuf,
    pub ops: Vec<Value>,
    pub kind: Kind,
    pub state: State,
    /// What nami said as it went.
    pub lines: Vec<String>,
    pub since: Instant,
    /// What nami left out, while the files are put back as they were
    /// applied (a second, quiet apply): then it's said.
    settling: Option<String>,
    /// When it was asked for (ms since 1970), for the desktop to know it's this one.
    pub asked: u64,
}

impl Job {
    #[cfg(test)]
    pub fn new_for_test() -> Job {
        Job { id: 1, attr: "geogebra6".into(), file: PathBuf::new(), ops: Vec::new(), kind: Kind::Install, state: State::Going, lines: Vec::new(), since: Instant::now(), settling: None, asked: 0 }
    }

    /// Why it didn't work, when that's plain: "no internet connection".
    pub fn reason(&self) -> Option<&'static str> {
        let State::Failed(log) = &self.state else { return None };
        // (The last two are Flatpak's, for an app from Flathub.)
        ["Dns Failed", "Network is unreachable", "failed to lookup address", "Connection refused", "timed out", "Could not resolve host", "Couldn't resolve host"].iter().any(|w| log.contains(w)).then_some("no internet connection")
    }

    /// "Downloading 120 MB…", while a big one comes down.
    pub fn note(&self) -> Option<String> {
        if self.state != State::Going {
            return None;
        }
        let p = crate::progress::Progress::read(&self.lines);
        let mb = p.mib * 1.048576;
        (p.total > 0 && p.done < p.total && mb >= 5.).then(|| match mb {
            mb if mb >= 1000. => format!("Downloading {:.1} GB…", mb / 1000.),
            mb if mb >= 100. => format!("Downloading {} MB…", (mb / 10.).round() * 10.),
            mb => format!("Downloading {} MB…", mb.round()),
        })
    }
}

/// A file set aside while a job runs: what's in it then (as last applied,
/// the app's line changed), how it was last applied, and what it is after,
/// with its other changes, if the job works or not.
pub struct Held {
    pub file: PathBuf,
    written: String,
    applied: String,
    pub ok: String,
    pub failed: String,
}

impl Shell {
    /// Whether nami is changing the computer now: a run, or an app's job.
    pub fn busy(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.finished.is_none()) || self.jobs.iter().any(|j| j.state == State::Going) || self.storage.cleaning()
    }

    /// The job for an app, if there's one to show.
    pub fn job(&self, attr: &str) -> Option<&Job> {
        self.jobs.iter().rev().find(|j| j.attr == attr)
    }

    /// Install `value` (`gimp`, `{gimp: "3.0"}`) in `file`, in place of what
    /// it lists for it (`old`): now, or after the ones before it.
    pub fn install(&mut self, file: PathBuf, attr: &str, value: Value, old: Option<Value>, cx: &mut Context<Self>) {
        let kind = if old.is_some() { Kind::Change } else { Kind::Install };
        let mut ops: Vec<Value> = old.into_iter().map(|o| json!({ "remove": { "path": ["packages"], "value": o } })).collect();
        ops.push(json!({ "add": { "path": ["packages"], "value": value } }));
        self.queue(file, attr, kind, ops, cx);
    }

    /// Remove an app `file` lists as `entry` (asked first, on its row).
    pub fn uninstall(&mut self, file: PathBuf, attr: &str, entry: Value, cx: &mut Context<Self>) {
        self.queue(file, attr, Kind::Remove, vec![json!({ "remove": { "path": ["packages"], "value": entry } })], cx);
    }

    /// Any other change to an app's line (`without:` on the computer's
    /// file), made the same way.
    pub fn change_alone(&mut self, file: PathBuf, attr: &str, kind: Kind, ops: Vec<Value>, cx: &mut Context<Self>) {
        self.queue(file, attr, kind, ops, cx);
    }

    fn queue(&mut self, file: PathBuf, attr: &str, kind: Kind, ops: Vec<Value>, cx: &mut Context<Self>) {
        if self.jobs.iter().any(|j| j.attr == attr && matches!(j.state, State::Waiting | State::Going)) {
            return;
        }
        self.jobs.retain(|j| j.attr != attr);
        // (Another change: the Undo of the one before goes.)
        self.undo = None;
        // What the page it was picked on knows of it: its name and icon show at once.
        if !self.catalog.contains_key(attr) {
            if let Some(f) = self.seen(attr) {
                if let Some(t) = &f.title {
                    self.names.insert(attr.to_string(), t.clone());
                }
                self.catalog.insert(attr.to_string(), f);
            }
        }
        self.next_id += 1;
        self.jobs.push(Job { id: self.next_id, attr: attr.to_string(), file, ops, kind, state: State::Waiting, lines: Vec::new(), since: Instant::now(), settling: None, asked: crate::installs::now_ms() });
        self.asking_remove = None;
        self.next_job(cx);
        cx.notify();
    }

    /// An app as the search, the catalog's pages or its own page showed it.
    fn seen(&self, attr: &str) -> Option<crate::nami::Found> {
        let e = &self.explore;
        let found = self.found.as_ref().and_then(|(_, r)| r.as_ref().ok()).into_iter().flatten().chain(self.found_flathub.iter().flat_map(|(_, f)| f));
        let listed = e.list.as_ref().and_then(|(_, r)| r.as_ref().ok()).into_iter().flatten();
        let page = e.app.iter().flat_map(|v| v.info.as_ref().and_then(|r| r.as_ref().ok()).into_iter().chain(v.first.as_ref()).chain(&v.similar));
        found.chain(listed).chain(e.featured.as_ref()).chain(page).find(|f| f.attr == attr).cloned()
    }

    /// Again, after it failed.
    pub fn retry(&mut self, attr: &str, cx: &mut Context<Self>) {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.attr == attr && matches!(j.state, State::Failed(_))) {
            j.state = State::Waiting;
            j.lines.clear();
            j.settling = None;
        }
        if self.details.as_deref() == Some(attr) {
            self.details = None;
        }
        self.next_job(cx);
        cx.notify();
    }

    /// Forget a failed one.
    pub fn dismiss(&mut self, attr: &str, cx: &mut Context<Self>) {
        self.jobs.retain(|j| !(j.attr == attr && matches!(j.state, State::Failed(_))));
        if self.details.as_deref() == Some(attr) {
            self.details = None;
        }
        self.tell_desktop();
        cx.notify();
    }

    /// How the apps go, for the desktop: the launcher's rows and what it tells.
    pub fn tell_desktop(&self) {
        let all: Vec<installs::Told> = self
            .jobs
            .iter()
            .map(|j| installs::Told {
                attr: j.attr.clone(),
                kind: j.kind.plain().to_string(),
                state: match &j.state {
                    State::Waiting => "waiting",
                    State::Going => "going",
                    State::Failed(_) => "failed",
                    State::Done => "done",
                }
                .to_string(),
                said: match &j.state {
                    State::Failed(log) => j.reason().map_or_else(|| log.clone(), str::to_string),
                    _ => String::new(),
                },
                asked: j.asked,
                file: j.file.clone(),
                ops: j.ops.clone(),
            })
            .collect();
        installs::write(&all);
    }

    /// Those that failed in a Settings that's gone since (started for the
    /// launcher, without a window): here, to try again or see why.
    pub fn failed_before(&mut self) {
        let day_ago = installs::now_ms().saturating_sub(24 * 60 * 60 * 1000);
        for t in installs::read() {
            let kind = match t.kind.as_str() {
                "install" => Kind::Install,
                "remove" => Kind::Remove,
                _ => Kind::Change,
            };
            if t.state != "failed" || t.asked < day_ago || t.ops.is_empty() || self.jobs.iter().any(|j| j.attr == t.attr) {
                continue;
            }
            self.next_id += 1;
            self.jobs.push(Job { id: self.next_id, attr: t.attr, file: t.file, ops: t.ops, kind, state: State::Failed(t.said), lines: Vec::new(), since: Instant::now(), settling: None, asked: t.asked });
        }
    }

    /// Install `attr`, asked for from the launcher (or for everyone, when
    /// `everyone`: the screen reader): where Settings' own Apps page would,
    /// at once. Yours, an admin's too; for everyone when it sets up the
    /// computer (docker, steam), which only an admin installs. (Flathub's are
    /// always yours.)
    pub fn install_asked(&mut self, attr: &str, for_system: bool, cx: &mut Context<Self>) {
        // (From the launcher, Settings may not know nami's packages yet: asked now.)
        if self.examples.is_err() {
            if let Ok(e) = nami::examples() {
                self.examples = Ok(e);
            }
        }
        let Ok(role) = &self.role else { return };
        // The system's (someone who isn't an admin: as an admin's, once they give an admin's
        // name and password, which the change asks for); Flathub's are always theirs.
        let system = !attr.starts_with("flathub:") && (for_system || self.sets_up_system(attr));
        let Some(file) = self.file(system) else { return };
        if role.child && !system {
            self.next_id += 1;
            let why = "Only an admin can install this app".to_string();
            self.jobs.retain(|j| j.attr != attr);
            self.jobs.push(Job { id: self.next_id, attr: attr.to_string(), file, ops: Vec::new(), kind: Kind::Install, state: State::Failed(why), lines: Vec::new(), since: Instant::now(), settling: None, asked: installs::now_ms() });
            self.tell_desktop();
            return;
        }
        // (Someone who isn't an admin: the password first, for what's copied in to set it up.)
        if system && self.sets_up_system(attr) {
            let a = attr.to_string();
            if self.unlock_first(&format!("{} is being installed on the system, for everyone on this computer.", self.app_name(attr)), move |g, cx| g.install_asked(&a, for_system, cx), cx) {
                return;
            }
        }
        let listed = crate::pages::packages(if system { &self.system } else { &self.user });
        let old = listed.into_iter().find(|e| crate::pages::entry_name(e) == attr);
        self.learn_names(vec![attr.to_string()], cx);
        if system && !crate::pages::explore::copy_setup(self, attr, cx) {
            return;
        }
        self.install(file, attr, json!(attr), old, cx);
    }

    /// Start the next job waiting, if nothing else is changing the computer.
    pub fn next_job(&mut self, cx: &mut Context<Self>) {
        self.tell_desktop();
        if self.busy() {
            return;
        }
        let Ok(role) = &self.role else { return };
        let user = role.user.clone();
        let Some(i) = self.jobs.iter().position(|j| j.state == State::Waiting) else {
            // What was asked for while the apps went on: a run, then the files' Apply.
            if let Some((title, cmds)) = self.later.take() {
                self.start(&title, cmds, cx);
                return;
            }
            if std::mem::take(&mut self.then_apply) && !self.before.is_empty() {
                self.apply(cx);
            }
            return;
        };
        let (id, file, ops) = (self.jobs[i].id, self.jobs[i].file.clone(), self.jobs[i].ops.clone());
        // For everyone: once an admin's password is given (system.nami is root's).
        if self.needs_password(&file) {
            let name = self.app_name(&self.jobs[i].attr);
            let about = match self.jobs[i].kind {
                Kind::Install => format!("{name} is being installed on the system, for everyone on this computer."),
                Kind::Remove => format!("{name} is being removed from the system, for everyone on this computer."),
                Kind::Change => format!("{name} is being updated on the system, for everyone on this computer."),
            };
            return self.as_admin(&file, &about, |g, cx| g.next_job(cx), cx);
        }
        let held = match self.set_aside(&file, &ops, file == user) {
            Ok(h) => h,
            Err(e) => {
                self.jobs[i].state = State::Failed(e);
                self.next_job(cx);
                return;
            }
        };
        self.held = held;
        let job = &mut self.jobs[i];
        job.state = State::Going;
        job.since = Instant::now();
        self.tell_desktop();
        self.run_job(id, cx);
        cx.notify();
    }

    /// nami applying a job's file: yours alone, or the computer's (through nami's daemon).
    fn run_job(&mut self, id: u64, cx: &mut Context<Self>) {
        let user = self.role.as_ref().map(|r| r.user.clone()).unwrap_or_default();
        let Some(file) = self.jobs.iter().find(|j| j.id == id).map(|j| j.file.clone()) else { return };
        // (Yours: `nami apply`; the computer's: `--system`, through the daemon, as sudo would.)
        let args = if file == user { vec!["apply".to_string()] } else { vec!["apply".to_string(), "--system".to_string()] };
        cx.spawn(async move |this, cx| {
            let mut rx = nami::run(args, None);
            let mut end = Err("nami stopped".to_string());
            while let Some(said) = rx.next().await {
                match said {
                    Said::Line(l) => {
                        let gone = this
                            .update(cx, |s, cx| {
                                if let Some(j) = s.jobs.iter_mut().find(|j| j.id == id && j.settling.is_none()) {
                                    j.lines.push(l);
                                }
                                cx.notify();
                            })
                            .is_err();
                        if gone {
                            return;
                        }
                    }
                    Said::Finished(r) => {
                        end = r;
                        break;
                    }
                }
            }
            let _ = this.update(cx, |s, cx| s.job_done(id, end, cx));
        })
        .detach();
    }

    /// The files as nami will apply them: `file` as last applied with
    /// `ops` made, and (the computer's) its other files as last applied.
    fn set_aside(&mut self, file: &Path, ops: &[Value], yours: bool) -> Result<Vec<Held>, String> {
        let now = match nami::read_text(file) {
            // Yours, not there yet (nothing installed from the command line first): made as
            // `nami install` makes it.
            Err(e) if yours && e.kind() == std::io::ErrorKind::NotFound => nami::new_user_text(),
            other => other.map_err(|e| format!("reading {}: {e}", file.display()))?,
        };
        let applied = self.before.iter().find(|(f, _)| f == file).and_then(|(_, t)| t.clone()).unwrap_or_else(|| now.clone());
        let ok = nami::edit_text(&now, ops)?;
        let written = if applied == now { ok.clone() } else { nami::edit_text(&applied, ops)? };
        let mut held = vec![Held { file: file.to_path_buf(), written, applied, ok, failed: now }];
        // The computer's apply reads all of its files: those waiting go as they were too.
        let user = self.role.as_ref().map(|r| r.user.clone()).unwrap_or_default();
        for (f, was) in &self.before {
            let (Some(was), false, false) = (was, f == file, yours || *f == user) else { continue };
            let now = nami::read_text(f).unwrap_or_default();
            if now != *was {
                held.push(Held { file: f.clone(), written: was.clone(), applied: was.clone(), ok: now.clone(), failed: now });
            }
        }
        for h in &held {
            if let Err(e) = nami::write(&h.file, &h.written) {
                // Back as they were.
                for h in &held {
                    let _ = nami::write(&h.file, &h.failed);
                }
                return Err(e);
            }
        }
        Ok(held)
    }

    /// A job ended: the files back with their other changes, and what's
    /// applied noted; then the next one.
    fn job_done(&mut self, id: u64, end: Result<(), String>, cx: &mut Context<Self>) {
        // nami sets up the rest and leaves out what it can't get ("leaving out
        // krita: no binary cache has it"): for this app, that's not working.
        let settled = self.jobs.iter().find(|j| j.id == id).and_then(|j| j.settling.clone());
        let end = match settled {
            Some(e) => Err(e),
            None => end,
        };
        let end = end.and_then(|()| match self.jobs.iter().find(|j| j.id == id) {
            Some(j) if j.kind != Kind::Remove => {
                let left = format!("leaving out {}:", j.attr);
                match j.lines.iter().find(|l| l.contains(&left)) {
                    Some(l) => Err(l.clone()),
                    None => Ok(()),
                }
            }
            _ => Ok(()),
        });
        let worked = end.is_ok();
        // Left out: nami took the file as applied. Applied again as it was,
        // so nami and the window agree on what's waiting for Apply.
        if let (Err(e), Some(j)) = (&end, self.jobs.iter_mut().find(|j| j.id == id)) {
            if j.settling.is_none() && e.contains(&format!("leaving out {}:", j.attr)) {
                j.settling = Some(e.clone());
                for h in &mut self.held {
                    h.written = h.applied.clone();
                    let _ = nami::write(&h.file, &h.written);
                }
                self.run_job(id, cx);
                return;
            }
        }
        for h in std::mem::take(&mut self.held) {
            let mut after = if worked { h.ok } else { h.failed };
            let applied = if worked { h.written.clone() } else { h.applied };
            // Changed while it ran (the desktop's own settings, as you pick them).
            if let Some(disk) = nami::read_text(&h.file).ok().filter(|d| *d != h.written) {
                after = match crate::desktop_part(&disk, &h.written) {
                    Some(namios) => {
                        let op = match namios {
                            Some(v) => json!({ "set": { "path": ["namios"], "value": v } }),
                            None => json!({ "unset": { "path": ["namios"] } }),
                        };
                        nami::edit_text(&after, &[op]).unwrap_or(disk)
                    }
                    None => {
                        self.problem = Some(crate::Problem::of("Your file was changed elsewhere. Your version was kept.", h.file.display()));
                        disk
                    }
                };
            }
            let _ = nami::write(&h.file, &after);
            self.before.retain(|(f, _)| *f != h.file);
            self.was.retain(|(f, _)| *f != h.file);
            if after != applied {
                self.note_was(&h.file, Some(&applied));
                self.before.push((h.file, Some(applied)));
            }
        }
        let mut attr = None;
        // An app removed: Undo puts it back, as it was listed.
        let removed = self.jobs.iter().find(|j| j.id == id && j.kind == Kind::Remove && worked).and_then(|j| {
            let entry = j.ops.iter().find_map(|o| o.get("remove").filter(|r| r["path"] == json!(["packages"])).map(|r| r["value"].clone()))?;
            Some((j.file.clone(), j.attr.clone(), entry))
        });
        if let Some((file, attr, entry)) = removed {
            let said = format!("{} was removed.", self.app_name(&attr));
            self.undo = Some(crate::Undo { said, how: crate::Undoing::Reinstall { file, attr, entry }, ready: true });
        }
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
            attr = Some(j.attr.clone());
            j.state = match end {
                Ok(()) => State::Done,
                Err(e) => {
                    // What nami said, without the paths that landed; what went wrong last.
                    let mut said: Vec<&str> = j.lines.iter().map(String::as_str).filter(|l| !l.trim().is_empty() && crate::progress::landed(l).is_none()).collect();
                    for l in e.lines() {
                        if !said.contains(&l) {
                            said.push(l);
                        }
                    }
                    State::Failed(said.join("\n"))
                }
            };
        }
        if worked {
            self.history = Some(nami::history());
            <crate::os::Active as crate::os::Os>::after_job(self);
            self.updates = Default::default();
            crate::pages::updates::look(cx);
        }
        // Its menu entry, there now (or gone): Open follows.
        if let Some(a) = attr {
            self.asked_names.remove(&a);
            self.launchers.remove(&a);
            self.learn_names(vec![a], cx);
        }
        self.reload(cx);
        self.next_job(cx);
        cx.notify();
    }

    /// While a job has a file set aside: what's in it for the window (as
    /// it will be, if the job works).
    pub fn held_text(&self, file: &Path) -> Option<&str> {
        self.held.iter().find(|h| h.file == file).map(|h| h.ok.as_str())
    }

    /// An edit to a file set aside: made to what it'll be after, either way.
    pub fn edit_held(&mut self, file: &Path, op: &Value) -> Option<Result<(), String>> {
        let h = self.held.iter_mut().find(|h| h.file == file)?;
        let edit = |t: &str| nami::edit_text(t, std::slice::from_ref(op));
        Some(edit(&h.ok).and_then(|ok| Ok((ok, edit(&h.failed)?))).map(|(ok, failed)| {
            h.ok = ok;
            h.failed = failed;
        }))
    }
}
