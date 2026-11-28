//! Settings' state: who this is, what the files say, the changes made to them and the runs
//! that put them in place. Its window (the sidebar, the toolbar, the page) is in shell/view.rs.

pub(crate) mod view;

pub(crate) use view::{Line, Side};

use std::path::{Path, PathBuf};

use futures::StreamExt;
use gpui::prelude::*;
use gpui::{App, Context, Entity};
use serde_json::Value;

use crate::input::TextInput;
use crate::nami::{Examples, Found, Role, Said};
use crate::{asked_page, follow_look, niri, open_file_arg, open_window, own_file};
use crate::os::{Active, Os};
use crate::{file_lines, installs, jobs, launch, look, nami, pages};

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    /// Apps and tools to get: the search, the kinds, today's app, the most used.
    Discover,
    /// What's installed, yours and everyone's, and their updates.
    Installed,
    /// Your user.nami besides its apps: the settings files, the shell's, what apps changed.
    YourSetup,
    Projects,
    Apps,
    History,
    Report,
    Updates,
    Sync,
    /// Off namiOS: what it is, trying it in a window, and a USB drive with your apps.
    MoveToNamios,
    /// An app in a download (a .deb, .rpm, AppImage, tarball), to install.
    InstallFile,
    /// Developer mode: a file, by its place in the list.
    File(usize),
    /// Changes to the computer apps asked for (`nami propose`), to apply or not.
    Requests,
    /// What nami takes of the disk, and Clean Up (from Installed).
    Storage,
    /// The system's own (namiOS's Settings: the Overview, its categories, People...).
    Os(<Active as Os>::Page),
}

/// As `Debug` says a page, the system's by their own names (`page=Settings` in nami-gui-status).
impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Page::Discover => f.write_str("Discover"),
            Page::Installed => f.write_str("Installed"),
            Page::YourSetup => f.write_str("YourSetup"),
            Page::Projects => f.write_str("Projects"),
            Page::Apps => f.write_str("Apps"),
            Page::History => f.write_str("History"),
            Page::Report => f.write_str("Report"),
            Page::Updates => f.write_str("Updates"),
            Page::Sync => f.write_str("Sync"),
            Page::MoveToNamios => f.write_str("MoveToNamios"),
            Page::InstallFile => f.write_str("InstallFile"),
            Page::File(n) => f.debug_tuple("File").field(n).finish(),
            Page::Requests => f.write_str("Requests"),
            Page::Storage => f.write_str("Storage"),
            Page::Os(p) => p.fmt(f),
        }
    }
}

/// A command running (Apply, going back a step): what it says.
pub struct Run {
    pub title: String,
    pub lines: Vec<String>,
    pub finished: Option<Result<(), String>>,
    /// What it ran, to try again.
    pub cmds: Vec<(Vec<String>, Option<String>)>,
    pub since: std::time::Instant,
    /// The command going (of `cmds`), and where its lines start in `lines`.
    pub step: usize,
    pub step_at: usize,
}

/// A change that takes something away (`edit_with_undo`): what's said asking for it, while it
/// goes ("Removing anna") and after ("anna is removed."); the change that puts it back, and
/// what's said asking for that and while it goes.
#[derive(Clone)]
pub struct Taking {
    pub about: String,
    pub doing: String,
    pub said: String,
    pub back: Value,
    pub back_about: String,
    pub back_doing: String,
}

/// What puts back a change that took something away: Undo, once it's done.
pub struct Undo {
    /// What's said with it, once the change is in place: "Cowsay is removed."
    pub said: String,
    pub how: Undoing,
    /// The change is in place (its run went well): Undo can be offered.
    pub ready: bool,
}

pub enum Undoing {
    /// The change that puts it back, what's said asking for it (an admin's password, for the
    /// computer's file), and while it goes.
    Edit { system: bool, op: Value, about: String, doing: String },
    /// An app removed: installed again, as it was listed.
    Reinstall { file: PathBuf, attr: String, entry: Value },
}

/// A change to system.nami waits for an admin's password: the desktop's box asks for
/// it (away from the window, which goes on as it was), then the changes go on, in order.
///
/// Someone who isn't an admin unlocks Settings with an admin's name and password, as on a Mac:
/// the settings for everyone can be changed then, in this window, until it's locked again, the
/// window closes, or nothing's changed for `UNLOCKED_FOR`. Each change still goes through
/// nami's daemon, which asks polkit every time (`auth_admin_keep`: the password given counts
/// for this window's process alone, for a few minutes; after that it's asked again).
#[derive(Default)]
pub struct Unlock {
    /// When it was last given: polkit doesn't ask again for 5 minutes (4, here).
    given: Option<std::time::Instant>,
    /// Asked for, not answered yet.
    pub asking: bool,
    /// What's waiting for it, in order.
    waiting: Vec<Box<dyn FnOnce(&mut Shell, &mut Context<Shell>)>>,
    /// Someone who isn't an admin, unlocked: when last unlocked or changed.
    open: Option<std::time::Instant>,
    /// Unlock was clicked (what's said if it isn't given), and its lock is being watched.
    unlocking: bool,
    watching: bool,
}

/// How long Settings stays unlocked without a change, for someone who isn't an admin.
pub const UNLOCKED_FOR: std::time::Duration = std::time::Duration::from_secs(5 * 60);

impl Unlock {
    /// Unlocked, at `now`.
    pub fn is_open(&self, now: std::time::Instant) -> bool {
        self.open.is_some_and(|t| now.saturating_duration_since(t) < UNLOCKED_FOR)
    }

    /// An admin's password given, at `now`: unlocked from then.
    fn opened(&mut self, now: std::time::Instant) {
        self.open = Some(now);
        self.given = Some(now);
    }

    /// A change made, unlocked: it stays so for `UNLOCKED_FOR` from now.
    fn touched(&mut self, now: std::time::Instant) {
        if self.is_open(now) {
            self.open = Some(now);
        }
    }

    /// Locked again: nothing's changed without the password again.
    fn locked(&mut self) {
        self.open = None;
        self.given = None;
    }
}

impl Drop for Unlock {
    /// (The window closing locks it: what polkit keeps goes too.)
    fn drop(&mut self) {
        if self.open.is_some() {
            nami::forget_password();
        }
    }
}

/// What went wrong, for the status line: a plain sentence, and what was said of it (nami's
/// words, a program's own, a path) behind Show Details. A sentence that says it all has none.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub said: String,
    pub detail: String,
}

impl Problem {
    /// `said`, with `detail` (what went wrong, as it was told) behind it.
    pub fn of(said: impl Into<String>, detail: impl std::fmt::Display) -> Problem {
        Problem { said: said.into(), detail: detail.to_string().trim().to_string() }
    }

    /// What went wrong, told under `said`: for `.err().map(…)`.
    pub fn under<E: std::fmt::Display>(said: &'static str) -> impl Fn(E) -> Problem {
        move |e| Problem::of(said, e)
    }
}

/// A sentence that says it all.
impl From<&str> for Problem {
    fn from(said: &str) -> Problem {
        Problem { said: said.to_string(), detail: String::new() }
    }
}

impl From<String> for Problem {
    fn from(said: String) -> Problem {
        Problem { said, detail: String::new() }
    }
}

pub struct Shell {
    /// Who this is is known (`role`, the files): looked up as Settings starts, before its window.
    known: bool,
    /// What nami said of who this is, as Settings started: put in place before its window opens.
    first: Option<(Whose, Option<String>)>,
    /// How many times the files were read: one read in the background, overtaken, is dropped.
    reloads: u64,
    /// What the page last showed (and which part of it: `Os::subpage`): another one starts at
    /// its top.
    showing: (Page, usize),
    pub page: Page,
    pub role: Result<Role, String>,
    /// What the files say (their own lines), as JSON.
    pub user: Value,
    /// How the page and the sidebar are scrolled: Tab brings what it gets to into view, and
    /// so do the keys that scroll and walk them.
    pub(crate) page_scroll: gpui::ScrollHandle,
    pub nav_scroll: gpui::ScrollHandle,
    /// In a narrow window, where the sidebar folds away: shown over the page, from the
    /// toolbar's button, until a page is picked (or Esc, or a click beside it).
    side_shown: bool,
    /// The window's own: keys only reach what holds the focus or is around it, so the window
    /// holds it when no field does, for Tab to start from and for its own keys.
    pub(crate) root_focus: gpui::FocusHandle,
    /// Whether the sidebar lets the wallpaper through (look::vibrant), as the window was told.
    glass: bool,
    pub system: Value,
    /// The files changed since they were applied, as they were before
    /// (Discard puts them back; None: there was none).
    pub before: Vec<(PathBuf, Option<String>)>,
    /// Files changed while a run was going, not part of it.
    edited_in_run: Vec<PathBuf>,
    /// Apply once the run that's going ends (an app's Install clicked during it).
    pub(crate) then_apply: bool,
    /// A change was just made: it's put in place once what handles the click is done (the
    /// changes one click makes go together).
    apply_queued: bool,
    /// What the next run that puts changes in place says while it goes ("Removing anna").
    next_title: Option<String>,
    /// The last change that took something away, to put back (Undo, on the status line).
    pub undo: Option<Undo>,
    /// What the status line said last, as written for tests (`note_status`).
    told_status: String,
    /// system.nami is root's: changes to it wait for an admin's password.
    pub unlock: Unlock,
    /// The apps each of those files listed then: what's installed.
    pub was: Vec<(PathBuf, Vec<String>)>,
    /// What went wrong with the last change, if something did.
    pub problem: Option<Problem>,
    /// The problem the status line said last: another starts with its details hidden.
    said_problem: Option<Problem>,
    /// What `problem` is, if it's that the person's file can't be read.
    unreadable: Option<String>,
    /// The last change's line in its file (Show in file), while it shows.
    pub said_file: Option<file_lines::Said>,
    /// A file with a mistake in it: said calmly, what it said before still showing.
    pub mistake: Option<file_lines::Mistake>,
    /// History: the steps opened to what's different, and the one asking to go back.
    pub history_view: pages::history::View,
    pub examples: Result<Examples, String>,
    pub search: Entity<TextInput>,
    pub found: Option<(String, Result<Vec<Found>, String>)>,
    pub searching: Option<String>,
    /// Flathub's apps for the same words, when they come (after the catalog's).
    pub found_flathub: Option<(String, Vec<Found>)>,
    /// New people's passwords, given to `nami apply`.
    pub passwords: Vec<(String, String)>,
    pub run: Option<Run>,
    pub report: pages::report::Report,
    pub updates: pages::updates::Updates,
    /// Move to namiOS (off namiOS).
    pub move_to: pages::move_to::MoveTo,
    pub sync: pages::sync::Sync,
    /// Sync: a folder's or computer's name, a folder, a computer's ID.
    pub sync_name: Entity<TextInput>,
    pub sync_path: Entity<TextInput>,
    pub sync_id: Entity<TextInput>,
    /// Another git repository for your setup (Sync).
    pub setup_repo: Entity<TextInput>,
    /// An app in a download, to install; and those installed from downloads.
    pub install_file: pages::install_file::InstallFile,
    /// Their updates: what nami knows of each, and the one updating now.
    pub app_updates: pages::app_updates::AppUpdates,
    /// Running from the USB drive: Install on this computer is offered.
    pub from_stick: bool,
    /// A computer nami made: its kernel can be picked (Apps).
    pub nami_os: bool,
    pub history: Option<Result<Vec<nami::Step>, String>>,
    pub dev: pages::dev::Dev,
    /// Apps: the own pick the search adds to (its folder's name), and the
    /// name of a new one.
    pub own_target: Option<String>,
    pub own_name: Entity<TextInput>,
    /// Looking around the catalog, and an app's page.
    pub explore: pages::explore::Explore,
    /// This computer's NVIDIA card and the driver package for it.
    pub gpu: Option<nami::Hardware>,
    pub projects: pages::projects::Projects,
    /// The apps' own names, by package name (`learn_names`).
    pub names: std::collections::HashMap<String, String>,
    /// What the catalog says of those apps (their line, icon), once known.
    pub catalog: std::collections::HashMap<String, Found>,
    /// The menu entry each app starts by, those on the computer.
    pub launchers: std::collections::HashMap<String, PathBuf>,
    pub(crate) asked_names: std::collections::HashSet<String>,
    /// Apps going on and off, one at a time (`jobs`), and the files set
    /// aside while one goes.
    pub jobs: Vec<jobs::Job>,
    pub(crate) next_id: u64,
    pub(crate) held: Vec<jobs::Held>,
    /// The app whose Remove is asking "Remove GIMP?".
    pub asking_remove: Option<String>,
    /// Uninstall nami…: asking, uninstalling, done.
    pub uninstall: pages::uninstall_nami::Uninstall,
    /// A Mac's nami, updated from Installed (macOS's box asks for an administrator).
    pub nami_update: pages::installed::NamiUpdate,
    /// Storage: what nami said of it, and Clean Up.
    pub storage: pages::storage::Storage,
    /// The app whose failed install shows what nami said.
    pub details: Option<String>,
    /// A run that went wrong showing what nami said (Show Details): its title and buttons
    /// say what happened first, the log only when it's asked for.
    run_details: bool,
    /// A run asked for while an app went on: after it.
    pub(crate) later: Option<(String, Vec<(Vec<String>, Option<String>)>)>,
    /// What's changed and not applied is known: an app can go on alone.
    pub ready: bool,
    /// Your Setup: what apps changed in their settings files, what's in your home to keep.
    pub your_setup: pages::your_setup::YourSetup,
    /// Installed: what's shown (all, the apps, the tools) and the words it's filtered by.
    pub installed: pages::installed::Installed,
    /// Changes to the computer apps asked for, waiting for a decision.
    pub requests: pages::requests::Requests,
    /// What the system keeps (namiOS's: its pages' forms, the category of Settings showing).
    pub os: <Active as Os>::State,
}

impl Shell {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        // What changes by itself, every second: the person's file, the apps from downloads,
        // Requests, whether the window floats. (The system's own, as Wi-Fi: `Os::state`'s.)
        cx.spawn(async move |this, cx| {
            // The person's file, changed from elsewhere (the control center, an editor): what shows follows.
            let file_time = || std::fs::metadata(crate::nami::user_file()).ok()?.modified().ok();
            let mut seen = file_time();
            // The apps from downloads, updated in the background (nami file-apps --update): what shows follows.
            let mut downloads_seen = pages::app_updates::changed_at();
            // Changes apps asked for (`nami propose`), asked or decided: Requests follows.
            let mut requests_seen = pages::requests::changed_at();
            loop {
                let now = file_time();
                if now != seen {
                    seen = now;
                    let _ = this.update(cx, |this, cx| this.reload_later(cx));
                }
                let now = pages::requests::changed_at();
                if now != requests_seen {
                    requests_seen = now;
                    let _ = this.update(cx, |_, cx| pages::requests::look(cx));
                }
                let now = pages::app_updates::changed_at();
                if now != downloads_seen {
                    downloads_seen = now;
                    let _ = this.update(cx, |this, cx| {
                        pages::install_file::reload(this);
                        cx.notify();
                    });
                }
                // Whether its window floats (Super+G, zoomed), as niri says: its buttons follow.
                // (Each of these on a thread of its own: `nami` waiting for the store, on the
                // background threads, doesn't hold up how the computer is shown.)
                let windows = crate::bounded::apart(|| niri(serde_json::json!("Windows"))).await.flatten();
                if windows.is_some_and(|w| crate::look::follow_floating(&w["Ok"]["Windows"], "nami-gui")) {
                    let _ = this.update(cx, |_, cx| cx.notify());
                }
                // (At the end: the first look is at once, as the window opens.)
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
            }
        })
        .detach();
        // Asked for from the desktop (Super+Comma, the control center, the launcher), with the
        // window open or not: started for an app from the launcher, it has none until one's asked.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(300)).await;
                // (Once who this is is known: the first look opens it then.)
                let gone = this.update(cx, |this, cx| {
                    let asked = asked_page().filter(|_| this.known);
                    if let Some(page) = asked.as_deref() {
                        this.go_asked(page, cx);
                        if cx.windows().is_empty() {
                            let me = cx.entity();
                            cx.defer(move |cx| open_window(me, cx));
                            return;
                        }
                    }
                    // Apps asked for from the launcher: once what's waiting for Apply is known,
                    // so each goes on alone.
                    if this.ready {
                        for attr in installs::take_asked() {
                            this.install_asked(&attr, false, cx);
                        }
                    }
                    // Without a window: locked again (unlocked for that window alone), and with
                    // nothing left to do, done.
                    if cx.windows().is_empty() && !this.busy() && !this.unlock.asking {
                        this.lock_settings(cx);
                    }
                    let idle = this.ready && !this.busy() && !this.install_file.busy() && !this.jobs.iter().any(|j| j.state == jobs::State::Waiting);
                    if idle && cx.windows().is_empty() {
                        cx.quit();
                    }
                });
                if gone.is_err() {
                    return;
                }
            }
        })
        .detach();
        let search = cx.new(|cx| TextInput::new(cx, &nami::search_said(), "", false));
        cx.observe(&search, |this, _, cx| pages::apps::search_changed(this, cx)).detach();
        // The window at once: what nami says (who this is, the files, …) fills it in as it comes.
        // How it looks is the person's file's, read here, so the first frame is already theirs.
        let user = own_file();
        follow_look(&user, cx);
        // (A package manager first, on another distribution; the system's first page on it.)
        let page = Active::first_page().unwrap_or(Page::Discover);
        let os = Active::state(cx);
        let asked = asked_page();
        // Who this is and their files, asked of nami as Settings started (beside gpui's start):
        // waited for here, so the window's first frame has its sidebar and page, not only their
        // backgrounds. (A moment at most: if nami's slow, the window opens and it fills in after.)
        let early = FIRST_LOOK.lock().ok().and_then(|mut f| f.take());
        let now = early.as_ref().and_then(|rx| rx.recv_timeout(std::time::Duration::from_millis(1500)).ok());
        let first = now.map(|found| (found, asked.clone()));
        let early = early.filter(|_| first.is_none());
        cx.spawn(async move |this, cx| {
            // (Unless it was in place before the window opened: main's take_first_look.)
            if !this.update(cx, |this, _| this.known).unwrap_or(true) {
                let found = cx.background_executor().spawn(async move { early.and_then(|rx| rx.recv().ok()).unwrap_or_else(look_up) }).await;
                let _ = this.update(cx, |this, cx| this.first_look(found, asked, cx));
            }
            // What's changed and not applied (`nami status` takes a moment more): after.
            let Ok(Ok(role)) = this.update(cx, |this, _| this.role.clone()) else {
                let _ = this.update(cx, |this, _| this.ready = true);
                return;
            };
            let pending = cx.background_executor().spawn(async move { pending_now(&role) }).await;
            let _ = this.update(cx, |this, cx| {
                // (Unless something's been applied since: that's newer. Only in Developer mode,
                // where the files are edited as text and wait for Apply: elsewhere each change is
                // put in place as it's made, and the next one takes the file as it is.)
                if this.dev.on && this.run.is_none() && this.jobs.iter().all(|j| j.state != jobs::State::Going) {
                    this.note_pending_from(pending);
                    cx.notify();
                }
                this.ready = true;
            });
        })
        .detach();
        Self::later(cx, nami::examples, |this, examples, _| {
            this.examples = examples;
        });
        Self::later(cx, nami::stick_and_nami_os, |this, (from_stick, nami_os), _| (this.from_stick, this.nami_os) = (from_stick, nami_os && !look::as_other_linux()));
        Self::later(cx, nami::nvidia, |this, gpu, _| {
            this.gpu = gpu;
        });
        Self::later(cx, || nami::history().ok().map(Ok), |this, history, cx| {
            // (Looked at again since: that's newer.)
            if this.history.is_none() {
                this.history = history;
                let attrs = history_attrs(&this.history);
                this.learn_names(attrs, cx);
            }
        });
        Shell {
            known: false,
            first,
            reloads: 0,
            showing: (page, Active::subpage(&os)),
            page,
            role: Err(String::new()),
            user,
            page_scroll: {
                let page = gpui::ScrollHandle::new();
                crate::look::set_scroller(page.clone());
                page
            },
            nav_scroll: gpui::ScrollHandle::new(),
            side_shown: false,
            root_focus: cx.focus_handle(),
            glass: false,
            system: Value::Null,
            before: Vec::new(),
            edited_in_run: Vec::new(),
            then_apply: false,
            apply_queued: false,
            next_title: None,
            undo: None,
            told_status: String::new(),
            unlock: Unlock::default(),
            was: Vec::new(),
            problem: None,
            said_problem: None,
            unreadable: None,
            said_file: None,
            mistake: None,
            history_view: Default::default(),
            examples: Err(String::new()),
            search,
            found: None,
            searching: None,
            found_flathub: None,
            passwords: Vec::new(),
            run: None,
            report: pages::report::Report::new(cx),
            updates: Default::default(),
            install_file: pages::install_file::InstallFile { apps: pages::install_file::installed(), ..Default::default() },
            app_updates: pages::app_updates::AppUpdates { known: pages::app_updates::read(), ..Default::default() },
            from_stick: false,
            nami_os: false,
            dev: pages::dev::Dev::new(cx),
            own_target: None,
            own_name: cx.new(|cx| TextInput::new(cx, "music", "", false)),
            move_to: pages::move_to::MoveTo::new(cx),
            sync: Default::default(),
            sync_name: cx.new(|cx| TextInput::new(cx, "Name", "", false)),
            sync_path: cx.new(|cx| TextInput::new(cx, "~/Pictures", "", false)),
            sync_id: cx.new(|cx| TextInput::new(cx, "Device ID: XXXXXXX-XXXXXXX-...", "", false)),
            setup_repo: cx.new(|cx| TextInput::new(cx, "git@example.com:you/setup.git", "", false)),
            explore: Default::default(),
            gpu: None,
            projects: pages::projects::Projects::new(cx),
            names: Default::default(),
            catalog: Default::default(),
            launchers: Default::default(),
            asked_names: Default::default(),
            jobs: Vec::new(),
            next_id: 0,
            held: Vec::new(),
            asking_remove: None,
            uninstall: Default::default(),
            nami_update: Default::default(),
            storage: Default::default(),
            details: None,
            run_details: false,
            later: None,
            ready: false,
            history: None,
            your_setup: pages::your_setup::YourSetup::new(cx),
            installed: pages::installed::Installed::new(cx),
            requests: Default::default(),
            os,
        }
    }

    /// What's slow to find out, in the background; then what it means for the window.
    pub(crate) fn later<T: Send + 'static>(cx: &mut Context<Self>, find: impl FnOnce() -> T + Send + 'static, then: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static) {
        cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async move { find() }).await;
            let _ = this.update(cx, |this, cx| {
                then(this, found, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Who this is and their files, if nami said as Settings started: in place before its
    /// window opens, so its first frame is the whole window.
    pub(super) fn take_first_look(&mut self, cx: &mut Context<Self>) {
        if let Some((found, asked)) = self.first.take() {
            self.first_look(found, asked, cx);
        }
    }

    /// Who this is and what their files say, now known: the window as it opened for.
    fn first_look(&mut self, (role, files): Whose, asked: Option<String>, cx: &mut Context<Self>) {
        self.page = match &role {
            // `--apps`: from the welcome; `--install-app`: from the launcher.
            // (Your own, an admin's too: what's for everyone is a tab away.)
            Ok(_) if std::env::args().any(|a| a == "--apps" || a == "--install-app") => Page::Discover,
            // `--sync`: your setup on all your computers, and your folders.
            Ok(_) if std::env::args().any(|a| a == "--sync") => {
                pages::sync::look(cx);
                Page::Sync
            }
            _ => self.page,
        };
        self.role = role;
        self.known = true;
        // (Before anything's said to the desktop: what it last heard is kept.)
        self.failed_before();
        if let Some(files) = files {
            self.reload_from(files, cx);
        }
        // (Those that failed before: their own names, "GeoGebra".)
        let failed: Vec<String> = self.jobs.iter().map(|j| j.attr.clone()).collect();
        self.learn_names(failed, cx);
        // Started for a page: that one, not another and then it. (`--page <name>` too.)
        let asked = asked.or_else(|| std::env::args().skip_while(|a| a != "--page").nth(1)).filter(|_| self.role.is_ok());
        let asked = match asked.as_deref() {
            Some("show") => None,
            Some("updates") => {
                self.page = if pages::store_mode() { Page::Installed } else { Page::Updates };
                None
            }
            Some("history") => {
                self.page = Page::History;
                None
            }
            Some("move-to-namios") => {
                self.go_asked("move-to-namios", cx);
                None
            }
            Some("apps") | Some("discover") => {
                self.page = Page::Discover;
                None
            }
            // (Storage, asking "Clean Up 2.3 GB?", as Clean Up… does: the review's pictures.)
            Some("storage-clean-up") => {
                self.go_asked("storage", cx);
                self.storage.cleaning = pages::storage::Cleaning::Asking;
                None
            }
            // (Installed, asking "Uninstall nami?", as Uninstall nami… does: the docs' pictures.)
            Some("uninstall-nami") => {
                self.go_asked("installed", cx);
                pages::uninstall_nami::ask(self, cx);
                None
            }
            Some("installed") | Some("setup") | Some("projects") | Some("sync") | Some("everyones-apps") | Some("requests") | Some("storage") => {
                self.go_asked(asked.as_deref().unwrap_or_default(), cx);
                None
            }
            Some(a) if a.starts_with("app:") || a.starts_with("find:") || a.starts_with("file:") || a.starts_with("install-file:") || a.starts_with("remove:") => {
                self.go_asked(a, cx);
                None
            }
            // (The system's: namiOS's Install, its Overview, People, Backups, About.)
            Some(a) if Active::first_route(self, a, cx) => None,
            _ => asked,
        };
        // Started for a download (`--open-file`): it, to install.
        if let (Some(f), true) = (open_file_arg(), self.role.is_ok()) {
            pages::install_file::open(self, f, cx);
        }
        Active::first_part(self, asked, cx);
        if self.page == Page::Updates {
            pages::updates::check(self, false, cx);
        } else {
            pages::updates::look(cx);
        }
        pages::projects::look(cx);
        pages::your_setup::look(cx);
        pages::requests::look(cx);
        // (Installed's Storage box, or Storage itself, as the window opens.)
        if matches!(self.page, Page::Installed | Page::Storage) && pages::store_mode() {
            pages::storage::look(self, cx);
        }
        if self.dev.on {
            self.set_dev(true, cx);
        }
        file_lines::watch(cx);
        cx.notify();
    }

    /// A file changed outside the window (by hand, by `nami`): what it says now shows,
    /// and Developer mode's view of it, if it has nothing unsaved.
    pub(crate) fn files_changed(&mut self, cx: &mut Context<Self>) -> bool {
        // (While a change goes in place: once it's done.)
        if !self.known || self.busy() {
            return false;
        }
        if let Some(path) = self.dev.path().cloned() {
            let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
            if on_disk != self.dev.saved && self.dev.editor.read(cx).content == self.dev.saved && !self.dev.drafts.contains_key(&path) {
                let line = self.dev.editor.read(cx).highlight;
                self.dev.saved = on_disk.clone();
                self.dev.editor.update(cx, |e, cx| {
                    e.set(cx, &on_disk);
                    e.highlight = line;
                });
            }
        }
        self.reload_later(cx);
        true
    }

    /// Developer mode on (the files, as text) or off (the pages).
    fn set_dev(&mut self, on: bool, cx: &mut Context<Self>) {
        self.dev.on = on;
        self.dev.remember();
        if on {
            // What the files say that isn't in place yet: for Apply and Discard.
            if !self.busy() {
                self.note_pending();
            }
            let at = if self.page == Page::Apps || matches!(self.page, Page::Os(p) if Active::on_system_file(p)) { 1 } else { 0 };
            pages::dev::open(self, at, cx);
            if self.page != Page::History && self.role.is_ok() {
                self.page = Page::File(self.dev.at);
            }
        } else {
            if matches!(self.page, Page::File(_)) {
                self.page = Page::Discover;
            }
            // Out of Developer mode, nothing waits for Apply: what was edited and not applied
            // stays in the file, and goes in place with the next change (or Developer mode's Apply).
            if !self.busy() {
                self.before.clear();
                self.was.clear();
            }
            self.reload(cx);
        }
        cx.notify();
    }

    /// The page the desktop asked for: Settings' own, or the apps' ("find:<words>", the
    /// launcher's More in Settings; "app:<attr>", an app's page, after it couldn't go on).
    pub(crate) fn go_asked(&mut self, page: &str, cx: &mut Context<Self>) {
        match page {
            // (Only to bring the window: it stays at its page.)
            "show" => {}
            "undo" => self.undo_last(cx),
            "move-to-namios" if pages::move_to::offered(self) => {
                self.page = Page::MoveToNamios;
                pages::move_to::look(self, cx);
            }
            _ if page.starts_with("find:") => {
                self.page = Page::Discover;
                self.explore.app = None;
                let words = page["find:".len()..].to_string();
                self.search.update(cx, |s, cx| s.set(cx, &words));
            }
            // One of your apps removed, as its row's Remove does once asked (tests/vm-quick.sh),
            // and Undo on the line below, as it's clicked.
            _ if page.starts_with("remove:") => {
                let attr = &page["remove:".len()..];
                self.page = Page::Discover;
                self.explore.app = None;
                let entry = pages::packages(&self.user).into_iter().find(|e| pages::entry_name(e) == attr);
                if let (Some(entry), Some(file)) = (entry, self.file(false)) {
                    self.uninstall(file, attr, entry, cx);
                }
            }
            _ if page.starts_with("install-file:") => {
                self.explore.app = None;
                pages::install_file::open_and_install(self, PathBuf::from(&page["install-file:".len()..]), cx);
            }
            _ if page.starts_with("file:") => {
                self.explore.app = None;
                pages::install_file::open(self, PathBuf::from(&page["file:".len()..]), cx);
            }
            _ if page.starts_with("app:") => {
                self.page = Page::Discover;
                let attr = page["app:".len()..].to_string();
                let failed = self.job(&attr).is_some_and(|j| matches!(j.state, jobs::State::Failed(_)));
                self.details = failed.then(|| attr.clone());
                let found = self.catalog.get(&attr).cloned();
                pages::explore::open(self, found, attr, cx);
            }
            _ => match app_page(page, pages::store_mode()) {
                Some(to) => {
                    self.page = to;
                    self.asked_for(to, cx);
                }
                None => {
                    if !Active::route(self, page, cx) {
                        self.page = Page::Discover;
                    }
                }
            },
        }
        cx.notify();
    }

    /// One of nami's own pages, asked for by name: what it shows, looked at again.
    fn asked_for(&mut self, page: Page, cx: &mut Context<Self>) {
        match page {
            Page::Discover => pages::install_file::reload(self),
            Page::Updates => pages::updates::check(self, false, cx),
            Page::Installed => {
                pages::install_file::reload(self);
                let attrs = self.installed_attrs();
                self.learn_names(attrs, cx);
            }
            Page::YourSetup => pages::your_setup::look(cx),
            Page::Projects => pages::projects::look(cx),
            Page::Sync => {
                self.sync = Default::default();
                pages::sync::look(cx);
            }
            // (From the notification "Claude wants to change your computer".)
            Page::Requests => pages::requests::look(cx),
            Page::Storage => pages::storage::look(self, cx),
            Page::Apps => self.explore.app = None,
            _ => {}
        }
    }

    /// Once who this is is known (the window's first look): the page it shows, and whether nami
    /// answered (`--smoke-test` says them).
    pub fn first_looked(&self) -> Option<(Page, bool)> {
        self.known.then(|| (self.page, self.role.is_ok()))
    }

    /// Whether the keys are a Mac's (Super as Command), as this person picked on namiOS;
    /// Linux's otherwise.
    pub fn mac_keys(&self) -> bool {
        self.user["namios"]["keyboard"]["keys"].as_str() == Some("mac")
    }

    pub fn admin(&self) -> bool {
        self.role.as_ref().is_ok_and(|r| r.admin)
    }

    /// Someone who isn't an admin, with Settings unlocked by an admin's name and password.
    pub fn unlocked(&self) -> bool {
        self.role.is_ok() && !self.admin() && self.unlock.is_open(std::time::Instant::now())
    }

    /// Whether this person may change what's for everyone on the computer: an admin, or
    /// someone else once it's unlocked. (Each change still asks nami's daemon, as an admin's.)
    pub fn may_change(&self) -> bool {
        self.admin() || self.unlocked()
    }

    /// Unlock, as on a Mac: the desktop asks for an admin's name and password (an admin of
    /// this computer picked, and their password typed), then what's for everyone can be changed.
    pub fn unlock_settings(&mut self, cx: &mut Context<Self>) {
        let Some(file) = self.file(true).filter(|_| !self.may_change()) else { return };
        self.unlock.unlocking = true;
        self.as_admin(&file, "Settings is being unlocked to change settings for everyone on this computer.", |g, cx| {
            g.reload(cx);
            cx.notify();
        }, cx);
    }

    /// Lock again (its button, the window closing, or `UNLOCKED_FOR` without a change): the
    /// settings for everyone go back to how anyone sees them, and polkit forgets the password.
    pub fn lock_settings(&mut self, cx: &mut Context<Self>) {
        if self.admin() || self.unlock.open.is_none() {
            return;
        }
        self.unlock.locked();
        self.system = Value::Null;
        Active::locked(self);
        cx.background_executor().spawn(async { nami::forget_password() }).detach();
        cx.notify();
    }

    /// Locks it once nothing's been changed for a while.
    fn watch_lock(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.unlock.watching, true) {
            return;
        }
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_secs(10)).await;
            let open = this.update(cx, |g, cx| {
                if g.unlock.open.is_some() && !g.unlock.is_open(std::time::Instant::now()) && !g.busy() && !g.unlock.asking {
                    g.lock_settings(cx);
                }
                g.unlock.watching = g.unlock.open.is_some();
                g.unlock.watching
            });
            if !open.unwrap_or(false) {
                return;
            }
        })
        .detach();
    }

    /// The apps and tools your file and the computer's list, for their names and icons.
    pub fn installed_attrs(&self) -> Vec<String> {
        pages::packages(&self.user).iter().chain(pages::packages(&self.system).iter()).map(pages::entry_name).collect()
    }

    /// Whether installing `attr` sets up the computer (docker, steam: a
    /// service, files in /etc), so it's only for everyone on it.
    pub fn sets_up_system(&self, attr: &str) -> bool {
        // (A Mac's nami sets nothing up on the computer yet: what's installed is each person's.)
        !look::MAC && self.examples.as_ref().is_ok_and(|e| e.sets_up_system(attr))
    }

    pub fn text(&self, field: &Entity<TextInput>, cx: &App) -> String {
        field.read(cx).content.trim().to_string()
    }

    /// Read the files again (and the settings' fields from them).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let unlocked = self.unlocked();
        let Ok(role) = &self.role else { return };
        let mut files = read_files(role, unlocked);
        // A file set aside while an app goes on: as it'll be after.
        if let Some(t) = self.held_text(&role.user) {
            files.0 = nami::get_text(t);
        }
        if let Some(t) = self.held_text(&role.system).filter(|_| role.admin || unlocked) {
            files.1 = nami::get_text(t).or(Ok(Value::Null));
        }
        self.reloads += 1;
        self.reload_from(files, cx);
    }

    /// The same, read in the background: for what changed outside the window.
    fn reload_later(&mut self, cx: &mut Context<Self>) {
        let Ok(role) = self.role.clone() else { return };
        let (at, unlocked) = (self.reloads, self.unlocked());
        Self::later(cx, move || read_files(&role, unlocked), move |this, files, cx| {
            // (Not over what was read since, nor while it's being applied, nor before what's
            // been set is written: it shows already.)
            if this.reloads == at && !this.busy() && !Active::writing() {
                this.reload_from(files, cx);
            }
        });
    }

    /// What the files say, shown: the settings' fields from them too.
    fn reload_from(&mut self, (user, system): Files, cx: &mut Context<Self>) {
        // (With a mistake in it, what showed stays, and it says so: not everything back to how it starts.)
        // A file with a mistake: a calm banner says where, and what it said before stays.
        let mut mistake = None;
        match user {
            Ok(user) => {
                self.user = user;
                if self.problem.is_some() && self.problem.as_ref().map(|p| p.detail.as_str()) == self.unreadable.take().as_deref().map(str::trim) {
                    self.problem = None;
                }
            }
            Err(e) => {
                self.unreadable = Some(e.clone());
                if let Some(file) = self.file(false) {
                    mistake = Some(file_lines::mistake_in(file_lines::name_of(self, &file), &file, &e));
                }
            }
        }
        follow_look(&self.user, cx);
        match system {
            // (Someone who isn't an admin: theirs to see only while it's unlocked; a read that
            // didn't go, polkit's password gone by now, leaves what showed.)
            _ if !self.may_change() => self.system = Value::Null,
            Ok(system) => self.system = system,
            Err(_) if !self.admin() => {}
            Err(e) => {
                if let Some(file) = self.file(true).filter(|_| mistake.is_none()) {
                    mistake = Some(file_lines::mistake_in(file_lines::name_of(self, &file), &file, &e));
                }
            }
        }
        self.mistake = mistake;
        // A line asked for says what the file says now.
        if let Some(path) = self.said_file.as_ref().filter(|s| s.asked).map(|s| s.path.clone()) {
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            self.said_file = file_lines::for_setting(self, &path);
        }
        Active::reloaded(self, cx);
        let mut attrs: Vec<String> = pages::packages(&self.user).iter().chain(&pages::packages(&self.system)).map(pages::entry_name).collect();
        attrs.extend(history_attrs(&self.history));
        self.learn_names(attrs, cx);
    }

    /// Back at the window: what changed outside it (`nami install` in a
    /// terminal, an update) shows, as what its own runs change does. (Looked at
    /// in the background: coming back to the window is never held up by it.)
    pub(super) fn look_again(&mut self, cx: &mut Context<Self>) {
        if !self.known || self.busy() {
            return;
        }
        pages::install_file::reload(self);
        let Ok(role) = self.role.clone() else { return };
        let (at, unlocked) = (self.reloads, self.unlocked());
        Self::later(cx, move || (nami::history(), nami::os_versions(), read_files(&role, unlocked)), move |this, (history, versions, files), cx| {
            if this.reloads != at || this.busy() {
                return;
            }
            this.history = Some(history);
            // (A new version may be waiting by now.)
            (this.updates.running, this.updates.waiting) = versions;
            this.asked_names.clear();
            this.reload_from(files, cx);
        });
    }

    /// A project's services, as last looked.
    pub fn services_of(&self, dir: &str) -> Vec<nami::Service> {
        self.projects.services.get(dir).cloned().unwrap_or_default()
    }

    /// Start an app, if it's on the computer; what went wrong shows below.
    pub fn open_app(&mut self, attr: &str, cx: &mut Context<Self>) {
        if let Some(entry) = self.launchers.get(attr) {
            self.problem = launch::open_app(entry).err().map(Problem::under("The app didn't open."));
            cx.notify();
        }
    }

    /// What to call an app: its own name, once known, else its package's.
    pub fn app_name(&self, attr: &str) -> String {
        self.names.get(attr).cloned().unwrap_or_else(|| plain_name(attr))
    }

    /// Ask the catalog for the names of these apps, in the background.
    pub fn learn_names(&mut self, attrs: Vec<String>, cx: &mut Context<Self>) {
        let new: Vec<String> = attrs.into_iter().filter(|a| self.asked_names.insert(a.clone())).collect();
        if new.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    nami::names(&new).into_iter().map(|f| {
                        let entry = launch::app_entry(&f.attr, &f.desktop);
                        (f.attr.clone(), f.title.clone(), entry, f)
                    }).collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |g, cx| {
                for (attr, title, entry, f) in found {
                    g.catalog.insert(attr.clone(), f);
                    if let Some(t) = title {
                        g.names.insert(attr.clone(), t);
                    }
                    match entry {
                        Some(e) => g.launchers.insert(attr, e),
                        None => g.launchers.remove(&attr),
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Changes made before this window opened (by it, another window or
    /// an editor) and not applied: as if made here, so Apply and Discard
    /// see them. Discard puts back what was applied.
    fn note_pending(&mut self) {
        let Ok(role) = &self.role else { return };
        let pending = pending_now(role);
        self.note_pending_from(pending);
    }

    fn note_pending_from(&mut self, pending: Vec<Pending>) {
        for (file, applied, listed) in pending {
            // (Changed here already, since nami was asked: how it was applied is what it was before.)
            self.was.retain(|(f, _)| *f != file);
            self.before.retain(|(f, _)| *f != file);
            self.was.push((file.clone(), listed));
            self.before.push((file, Some(applied)));
        }
    }

    /// The file the page changes: the computer's, or yours.
    /// On Apps (system.nami), not My apps (yours).
    pub fn computer_page(&self) -> bool {
        self.page == Page::Apps
    }

    pub fn file(&self, system: bool) -> Option<PathBuf> {
        let role = self.role.as_ref().ok()?;
        Some(if system { role.system.clone() } else { role.user.clone() })
    }

    /// Change a file (`nami config json`), noting how it was before.
    pub fn edit(&mut self, system: bool, op: Value, cx: &mut Context<Self>) {
        self.edit_saying(system, op, CHANGING, cx)
    }

    /// The same, saying what's changing when it asks for an admin's password
    /// (system.nami: "Turning the firewall off.").
    pub fn edit_saying(&mut self, system: bool, op: Value, about: &str, cx: &mut Context<Self>) {
        let Some(file) = self.file(system) else { return };
        // (Another change: the Undo of the one before goes.)
        self.undo = None;
        self.as_admin(&file.clone(), about, move |g, cx| g.edit_file(file, op, cx), cx)
    }

    /// A change that takes something away, put in place at once like the rest, with an Undo
    /// once it's done (`taking` says what it does, and what puts it back).
    pub fn edit_with_undo(&mut self, system: bool, op: Value, taking: Taking, cx: &mut Context<Self>) {
        self.edit_saying(system, op, &taking.about, cx);
        self.next_title = Some(taking.doing);
        self.undo = Some(Undo { said: taking.said, how: Undoing::Edit { system, op: taking.back, about: taking.back_about, doing: taking.back_doing }, ready: false });
    }

    /// Undo, on the status line: what the last change took away, back.
    pub fn undo_last(&mut self, cx: &mut Context<Self>) {
        let Some(undo) = self.undo.take() else { return };
        if self.busy() {
            self.undo = Some(undo);
            return;
        }
        match undo.how {
            Undoing::Edit { system, op, about, doing } => {
                self.edit_saying(system, op, &about, cx);
                self.next_title = Some(doing);
            }
            Undoing::Reinstall { file, attr, entry } => self.install(file, &attr, entry, None, cx),
        }
        cx.notify();
    }

    /// Put what was just changed in place, once the click that changed it is handled: the
    /// changes it made go together, in one step. (In Developer mode they wait for Apply.)
    fn apply_soon(&mut self, cx: &mut Context<Self>) {
        if self.dev.on || self.apply_queued {
            return;
        }
        self.apply_queued = true;
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |g, cx| {
                if std::mem::take(&mut g.apply_queued) {
                    g.apply_now(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Whether a change to `file` waits for an admin's password: it's the computer's (root's
    /// to write), and none was given a moment ago (or one's being asked for).
    pub fn needs_password(&self, file: &Path) -> bool {
        let theirs = self.role.as_ref().is_ok_and(|r| r.system != file) || nami::writable(file);
        let given = self.unlock.given.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(240));
        !theirs && (self.unlock.asking || !given)
    }

    /// `then`, once an admin's password is given for a change to `file` if it's the computer's
    /// (root's to write: nami writes it for them, through its daemon); at once otherwise, or
    /// if it was given a moment ago. Without it, nothing changes, and the window says so.
    pub fn as_admin(&mut self, file: &Path, about: &str, then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) {
        // (Unlocked: a change keeps it so a while longer.)
        self.unlock.touched(std::time::Instant::now());
        if !self.needs_password(file) {
            return then(self, cx);
        }
        self.unlock.waiting.push(Box::new(then));
        if self.unlock.asking {
            return;
        }
        self.unlock.asking = true;
        let (about, file) = (about.to_string(), file.to_path_buf());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let answer = cx.background_executor().spawn(async move { nami::unlock(&file, &about) }).await;
            let _ = this.update(cx, |g, cx| {
                g.unlock.asking = false;
                let unlocking = std::mem::take(&mut g.unlock.unlocking);
                let waiting = std::mem::take(&mut g.unlock.waiting);
                match answer {
                    Ok(()) => {
                        g.unlock.given = Some(std::time::Instant::now());
                        // Someone who isn't an admin: unlocked by it, as a Mac's Settings are
                        // by the password a change asked for.
                        if !g.admin() {
                            g.unlock.opened(std::time::Instant::now());
                            g.watch_lock(cx);
                        }
                        for then in waiting {
                            then(g, cx);
                        }
                    }
                    // (Unlock not given, cancelled or refused: it stays locked, and says so on
                    // its own row, as a Mac's does.)
                    Err(_) if unlocking && waiting.len() == 1 => {}
                    Err(e) => {
                        g.problem = Some(Problem::of("Nothing was changed. An administrator password is required.", &e));
                        g.then_apply = false;
                        // (Nothing changed: nothing to undo, nor to say while it goes.)
                        g.undo = None;
                        g.next_title = None;
                        // The apps waiting to go on for everyone: not without it.
                        let system = g.role.as_ref().map(|r| r.system.clone()).ok();
                        for j in g.jobs.iter_mut().filter(|j| j.state == jobs::State::Waiting && Some(&j.file) == system.as_ref()) {
                            j.state = jobs::State::Failed(e.clone());
                        }
                        g.tell_desktop();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Someone who isn't an admin, about to change the computer's folder (one of nami's
    /// packages copied in, through nami's daemon, which asks for an admin's password where it
    /// can't wait for it): that password first, then `again`. Whether it waits for it.
    pub fn unlock_first(&mut self, about: &str, again: impl FnOnce(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) -> bool {
        let Some(file) = self.file(true) else { return false };
        if self.admin() || !self.needs_password(&file) {
            return false;
        }
        self.as_admin(&file, about, again, cx);
        true
    }

    /// The same for any nami file of the repo (an own pick's package.nami).
    pub fn edit_file(&mut self, file: PathBuf, op: Value, cx: &mut Context<Self>) {
        // Set aside while an app goes on: changed as it'll be after.
        if let Some(r) = self.edit_held(&file, &op) {
            self.problem = r.err().map(Problem::under("Couldn't change the setting."));
            self.reload(cx);
            self.apply_soon(cx);
            cx.notify();
            return;
        }
        if !self.before.iter().any(|(f, _)| *f == file) {
            let text = nami::read_text(&file).ok();
            self.note_was(&file, text.as_deref());
            self.before.push((file.clone(), text));
        }
        if self.run.as_ref().is_some_and(|r| r.finished.is_none()) {
            self.edited_in_run.push(file.clone());
        }
        self.problem = nami::edit(&file, op).err().map(Problem::under("Couldn't change the setting."));
        self.reload(cx);
        // In place at once, as a Mac's Settings does it (not in Developer mode: there, Apply).
        if self.problem.is_none() {
            self.apply_soon(cx);
        }
        cx.notify();
    }

    pub(crate) fn note_was(&mut self, file: &Path, text: Option<&str>) {
        self.was.push((file.to_path_buf(), listed(text)));
    }

    /// What's changing for `attr` in `file`: (true) installed or (false)
    /// uninstalled when the files are applied, if either.
    pub fn will_change(&self, file: &Path, attr: &str, listed: bool) -> Option<bool> {
        // (Only Developer mode's changes wait: elsewhere, the app's going on or off already.)
        if !self.dev.on {
            return None;
        }
        let (_, was) = self.was.iter().find(|(f, _)| f == file)?;
        (was.iter().any(|a| a == attr) != listed).then_some(listed)
    }

    /// What applying does to apps: "GIMP will be installed; VLC uninstalled".
    fn changes_said(&self) -> Option<String> {
        let role = self.role.as_ref().ok()?;
        let (mut on, mut off) = (Vec::new(), Vec::new());
        for (file, was) in &self.was {
            let now = if *file == role.user { &self.user } else if *file == role.system { &self.system } else { continue };
            let now: Vec<String> = pages::packages(now).iter().map(pages::entry_name).collect();
            on.extend(now.iter().filter(|a| !was.contains(a)).map(|a| self.app_name(a)));
            off.extend(was.iter().filter(|a| !now.contains(a)).map(|a| self.app_name(a)));
        }
        let list = |v: &[String]| match v {
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
            [] => String::new(),
        };
        let said = match (on.is_empty(), off.is_empty()) {
            (true, true) => None,
            (false, true) => Some(format!("{} will be installed when you apply.", list(&on))),
            (true, false) => Some(format!("{} will be removed when you apply.", list(&off))),
            (false, false) => Some(format!("{} will be installed and {} removed when you apply.", list(&on), list(&off))),
        };
        said.map(|s| s[..1].to_uppercase() + &s[1..])
    }

    fn discard(&mut self, cx: &mut Context<Self>) {
        if let Some(system) = self.role.as_ref().ok().map(|r| r.system.clone()).filter(|c| self.before.iter().any(|(f, _)| f == c)) {
            return self.as_admin(&system, "This computer's settings will be reverted.", |g, cx| g.discard_now(cx), cx);
        }
        self.discard_now(cx)
    }

    fn discard_now(&mut self, cx: &mut Context<Self>) {
        self.was.clear();
        for (file, text) in std::mem::take(&mut self.before) {
            let _ = match text {
                Some(t) => nami::write(&file, &t).map_err(drop),
                None => std::fs::remove_file(&file).map_err(drop),
            };
        }
        self.passwords.clear();
        self.problem = None;
        self.reload(cx);
        cx.notify();
    }

    /// Run nami commands one after another, showing what they say.
    pub fn start(&mut self, title: &str, cmds: Vec<(Vec<String>, Option<String>)>, cx: &mut Context<Self>) {
        // An app going on: after it.
        if self.jobs.iter().any(|j| j.state == jobs::State::Going) {
            self.later = Some((title.to_string(), cmds));
            cx.notify();
            return;
        }
        self.run = Some(Run { title: title.to_string(), lines: Vec::new(), finished: None, cmds: cmds.clone(), since: std::time::Instant::now(), step: 0, step_at: 0 });
        self.run_details = false;
        cx.spawn(async move |this, cx| {
            for (step, (args, input)) in cmds.into_iter().enumerate() {
                let _ = this.update(cx, |s, _| {
                    if let Some(run) = &mut s.run {
                        (run.step, run.step_at) = (step, run.lines.len());
                    }
                });
                let mut rx = nami::run(args, input);
                let mut ok = false;
                while let Some(said) = rx.next().await {
                    let done = matches!(said, Said::Finished(_));
                    let failed = matches!(said, Said::Finished(Err(_)));
                    let gone = this
                        .update(cx, |s, cx| {
                            if let Some(run) = &mut s.run {
                                match said {
                                    Said::Line(l) => run.lines.push(l),
                                    Said::Finished(Err(e)) => run.finished = Some(Err(e)),
                                    Said::Finished(Ok(())) => {}
                                }
                            }
                            cx.notify();
                        })
                        .is_err();
                    if gone {
                        return;
                    }
                    if done {
                        ok = !failed;
                        break;
                    }
                }
                if !ok {
                    let _ = this.update(cx, |s, cx| {
                        s.then_apply = false;
                        // (It didn't go in place: nothing to undo.)
                        s.undo = None;
                        s.next_job(cx);
                    });
                    return;
                }
            }
            let _ = this.update(cx, |s, cx| {
                if let Some(run) = &mut s.run {
                    run.finished = Some(Ok(()));
                }
                // (The change that took something away is in place: its Undo can be offered.)
                if let Some(u) = &mut s.undo {
                    u.ready = true;
                }
                s.before.clear();
                s.was.clear();
                s.passwords.clear();
                // Changed while it ran: still to apply.
                let later = std::mem::take(&mut s.edited_in_run);
                if !later.is_empty() {
                    s.note_pending();
                }
                for file in later {
                    if !s.before.iter().any(|(f, _)| *f == file) {
                        let text = nami::read_text(&file).ok();
                        s.before.push((file, text));
                    }
                }
                s.history = Some(nami::history());
                s.asked_names.clear();
                Active::after_run(s, cx);
                s.updates = Default::default();
                pages::updates::look(cx);
                pages::projects::look(cx);
                pages::your_setup::look(cx);
                // (What nami uses, after a change: Installed's Storage box shows it.)
                if matches!(s.page, Page::Installed | Page::Storage) && pages::store_mode() {
                    pages::storage::look(s, cx);
                }
                s.reload(cx);
                // Apps that waited for it, then a second Apply asked for meanwhile.
                s.next_job(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Apps go on and off with one click: apply what was just changed, after
    /// the run that's going if there is one.
    pub fn apply_now(&mut self, cx: &mut Context<Self>) {
        // (After the change it follows, once its password's given.)
        if self.unlock.asking {
            self.unlock.waiting.push(Box::new(|g, cx| g.apply_now(cx)));
            return;
        }
        if self.problem.is_some() {
            return;
        }
        if self.busy() {
            self.then_apply = true;
        } else {
            self.apply(cx);
        }
    }

    /// Make the computer (and you) match the files.
    pub fn apply(&mut self, cx: &mut Context<Self>) {
        let Ok(role) = &self.role else { return };
        let mut cmds = Vec::new();
        // Anything but your own file is the computer's (a person's file, a package folder).
        if self.before.iter().any(|(f, _)| *f != role.user) {
            // (Named, for someone who isn't an admin: theirs is their own file otherwise.)
            let mut args = vec!["apply".to_string(), "--system".to_string()];
            let input = (!self.passwords.is_empty()).then(|| self.passwords.iter().map(|(l, p)| format!("{l}:{p}\n")).collect::<String>());
            if input.is_some() {
                args.push("--password-stdin".into());
            }
            cmds.push((args, input));
        }
        if self.before.iter().any(|(f, _)| *f == role.user) {
            cmds.push((vec!["apply".to_string()], None));
        }
        // (What was changed so far goes in this one.)
        self.apply_queued = false;
        let title = self.next_title.take().filter(|_| !self.dev.on).unwrap_or_else(|| MAKING.to_string());
        if cmds.is_empty() {
            return;
        }
        self.start(&title, cmds, cx);
    }
}

/// One of nami's own pages, by the name it's asked for by (`--page`, the launcher, `nami
/// desktop settings <page>`). As a package manager, the updates are Installed's, and there's
/// no System Apps.
pub(crate) fn app_page(name: &str, store: bool) -> Option<Page> {
    Some(match name {
        "apps" | "discover" => Page::Discover,
        "updates" if store => Page::Installed,
        "updates" => Page::Updates,
        "history" => Page::History,
        "installed" => Page::Installed,
        "setup" => Page::YourSetup,
        "projects" => Page::Projects,
        "sync" => Page::Sync,
        "requests" => Page::Requests,
        "storage" if store => Page::Storage,
        "everyones-apps" if !store => Page::Apps,
        _ => return None,
    })
}

/// What the person's file says, and the computer's (an admin's; else nothing).
type Files = (Result<Value, String>, Result<Value, String>);

/// Who this is, and what their files say: what Settings' window is drawn for.
type Whose = (Result<Role, String>, Option<Files>);

pub(super) fn look_up() -> Whose {
    let role = nami::role();
    let files = role.as_ref().ok().map(|r| read_files(r, false));
    (role, files)
}

/// `look_up`, started as Settings is (beside gpui's start), for its window's first frame.
pub(super) static FIRST_LOOK: std::sync::Mutex<Option<std::sync::mpsc::Receiver<Whose>>> = std::sync::Mutex::new(None);

fn read_files(role: &Role, unlocked: bool) -> Files {
    // (Both at once: Settings' window waits for them as it opens.)
    std::thread::scope(|s| {
        let system = s.spawn(|| if role.admin || unlocked { nami::get(&role.system) } else { Ok(Value::Null) });
        (nami::get(&role.user), system.join().unwrap_or_else(|_| Err("nami stopped".into())))
    })
}

/// A file changed and not applied: it, its text as applied, and the apps it listed then.
type Pending = (PathBuf, String, Vec<String>);

fn pending_now(role: &Role) -> Vec<Pending> {
    let ours = [role.user.clone(), role.system.clone()];
    nami::pending()
        .into_iter()
        .filter_map(|(path, applied)| {
            let file = ours.iter().find(|f| f.canonicalize().ok().as_ref() == Some(&path)).cloned().unwrap_or(path);
            if only_desktop_differs(&file, &applied) {
                return None;
            }
            let listed = listed(Some(&applied));
            Some((file, applied, listed))
        })
        .collect()
}

/// The apps a file's text lists.
fn listed(text: Option<&str>) -> Vec<String> {
    text.and_then(|t| nami::get_text(t).ok()).map(|v| pages::packages(&v).iter().map(pages::entry_name).collect()).unwrap_or_default()
}

/// The apps the steps changed.
fn history_attrs(history: &Option<Result<Vec<nami::Step>, String>>) -> Vec<String> {
    let Some(Ok(steps)) = history else { return Vec::new() };
    steps.iter().flat_map(|s| s.added.iter().chain(&s.removed).chain(&s.changed)).map(|e| e.attr.clone()).collect()
}

/// The run Apply starts.
const MAKING: &str = "Applying your changes";
/// What the box asking for an admin's password says, when nothing says more.
pub const CHANGING: &str = "This changes a setting for everyone on this computer.";

/// An app the catalog doesn't name, by what's left: a recipe's folder by its name, not its
/// path (`/etc/nami/packages/foo`: foo); nami's own window as it's called here (Settings in
/// namiOS, nami elsewhere).
fn plain_name(attr: &str) -> String {
    let name = if attr.contains('/') { attr.trim_end_matches('/').rsplit('/').next().unwrap_or(attr) } else { attr };
    match name {
        "nami-gui" if look::own_frame() || look::MAC => "nami".into(),
        "nami-gui" => "Settings".into(),
        _ => name.to_string(),
    }
}

/// What a file says apart from `namios:` (the desktop's own settings, that
/// follow as they're picked), if `now` and `then` say the same: then `now`'s
/// `namios:`, if it has one.
pub(crate) fn desktop_part(now: &str, then: &str) -> Option<Option<Value>> {
    let split = |text: &str| -> Option<(serde_yaml::Value, Option<serde_yaml::Value>)> {
        let mut value: serde_yaml::Value = crate::nami::yaml(text).ok()?;
        let desk = value.as_mapping_mut()?.remove("namios");
        Some((value, desk))
    };
    let ((rest, desk), (rest2, _)) = (split(now)?, split(then)?);
    (rest == rest2).then(|| desk.and_then(|d| serde_json::to_value(d).ok()))
}

/// Light or dark, as the desktop is (NAMI_THEME=dark or light says which).
/// Whether all that changed in a file is `namios:`: the desktop follows that by itself, so
/// there's nothing to apply.
fn only_desktop_differs(file: &Path, applied: &str) -> bool {
    let without = |text: &str| {
        let mut value: serde_yaml::Value = crate::nami::yaml(text).ok()?;
        value.as_mapping_mut()?.remove("namios");
        Some(value)
    };
    let now = std::fs::read_to_string(file).ok().and_then(|t| without(&t));
    now.is_some() && now == without(applied)
}

#[cfg(test)]
#[test]
fn unlocked_until_locked_or_five_minutes_without_a_change() {
    use std::time::{Duration, Instant};
    let t = Instant::now();
    let min = |m: u64| t + Duration::from_secs(m * 60);
    let mut u = Unlock::default();
    // Locked to begin with; a change made then doesn't unlock it.
    assert!(!u.is_open(t));
    u.touched(t);
    assert!(!u.is_open(t) && u.open.is_none());
    // An admin's password given: unlocked, and polkit's few minutes noted.
    u.opened(t);
    assert!(u.is_open(t) && u.is_open(min(4)) && u.given == Some(t));
    assert!(!u.is_open(min(5)), "five minutes without a change lock it");
    // A change keeps it so: five minutes from that one.
    u.touched(min(4));
    assert!(u.is_open(min(8)) && !u.is_open(min(9)));
    // Once it's locked itself, a change doesn't open it again.
    u.touched(min(10));
    assert!(!u.is_open(min(10)));
    // Lock: nothing kept, the password asked again.
    u.opened(min(11));
    u.locked();
    assert!(!u.is_open(min(11)) && u.given.is_none() && u.open.is_none());
}

#[cfg(test)]
#[test]
fn a_recipe_is_named_by_its_folder() {
    assert_eq!(plain_name("/etc/nami/packages/hello"), "hello");
    assert_eq!(plain_name("/etc/nami/packages/hello/"), "hello");
    assert_eq!(plain_name("gimp"), "gimp");
    assert!(["nami", "Settings"].contains(&plain_name("/etc/nami-config/pc/packages/nami-gui").as_str()));
}

#[cfg(test)]
#[test]
fn the_desktops_part_alone() {
    let then = "packages:\n  - gimp\nnamios:\n  appearance:\n    theme: light\n";
    let now = "packages:\n  - gimp\nnamios:\n  appearance:\n    theme: dark\n";
    assert_eq!(desktop_part(now, then), Some(Some(serde_json::json!({ "appearance": { "theme": "dark" } }))));
    assert_eq!(desktop_part("packages:\n  - gimp\n", then), Some(None));
    assert_eq!(desktop_part("packages:\n  - vlc\n", then), None);
}

#[cfg(test)]
#[test]
fn a_problem_is_a_sentence_and_what_was_said_of_it() {
    let told: Result<(), String> = Err("  couldn't parse user.nami: line 3\n".into());
    let p = told.map_err(Problem::under("Couldn't change the setting.")).unwrap_err();
    assert_eq!(p, Problem { said: "Couldn't change the setting.".into(), detail: "couldn't parse user.nami: line 3".into() });
    // (A sentence that says it all has nothing behind it.)
    assert_eq!(Problem::from("The two passwords aren't the same.").detail, "");
    assert_eq!(Problem::of("The disk couldn't be opened.", std::path::Path::new("/dev/sdb1").display()).detail, "/dev/sdb1");
}
