//! Sync: your folders kept the same on your computers (the sync package:
//! Syncthing), as ~/.config/nami-sync.conf in your user.nami says. This
//! computer's ID to give the others, the folders and how far along they
//! are, the other computers and whether they're online.

use gpui::prelude::*;
use crate::look::Press;
use gpui::{ClipboardItem, Context, Div, ElementId, FontWeight, ParentElement, StatefulInteractiveElement, Styled, div, rgb};
use crate::look::px;
use crate::look::text;
use serde_json::{Value, json};

use crate::icons::glyph;
use crate::nami::{self, SyncStatus};
use crate::ui::{button, color, pill};
use crate::Shell;

const CONF: &str = "~/.config/nami-sync.conf";

#[derive(Default)]
pub struct Sync {
    pub status: Option<Option<Result<SyncStatus, String>>>,
    pub copied: bool,
    pub adding: Option<&'static str>,
    /// Your setup on all your computers (`nami sync status`).
    pub setup: Option<Result<nami::SetupSync, String>>,
    /// Turning it on: how far along.
    pub turning_on: Option<TurningOn>,
    /// Another git repository's address, being typed.
    pub other_repo: bool,
    /// Turn Off asking first.
    pub asking_off: bool,
    /// Sync Now, Turn Off or an Undo going on.
    pub busy: Option<&'static str>,
    pub code_copied: bool,
}

#[derive(Default, Clone)]
pub struct TurningOn {
    pub said: String,
    /// The code to enter on GitHub.
    pub code: Option<String>,
    pub failed: Option<String>,
}

/// Look again, in the background.
pub fn look(cx: &mut Context<Shell>) {
    look_setup(cx);
    cx.spawn(async move |this, cx| {
        let s = cx.background_executor().spawn(async { nami::sync_status() }).await;
        let _ = this.update(cx, |g, cx| {
            g.sync.status = Some(s);
            cx.notify();
        });
    })
    .detach();
}

/// Your setup's sync, looked at again (the Sync page, History).
pub fn look_setup(cx: &mut Context<Shell>) {
    cx.spawn(async move |this, cx| {
        let s = cx.background_executor().spawn(async { nami::setup_sync() }).await;
        let _ = this.update(cx, |g, cx| {
            g.sync.setup = Some(s);
            cx.notify();
        });
    })
    .detach();
}

/// `nami sync <args>` in the background (`doing` while it goes), then a look again.
pub fn run_setup(g: &mut Shell, doing: &'static str, args: Vec<String>, failed: &'static str, cx: &mut Context<Shell>) {
    if g.sync.busy.is_some() {
        return;
    }
    g.sync.busy = Some(doing);
    cx.notify();
    cx.spawn(async move |this, cx| {
        let r = cx.background_executor().spawn(async move { nami::setup_sync_run(&args.iter().map(String::as_str).collect::<Vec<_>>()) }).await;
        let _ = this.update(cx, |g, cx| {
            g.sync.busy = None;
            if let Err(e) = r {
                g.problem = Some(crate::Problem::of(failed, e));
            }
            look_setup(cx);
            cx.notify();
        });
    })
    .detach();
}

/// Turn it on: with GitHub (signed in to in the browser), or the repository typed.
fn turn_on(g: &mut Shell, repo: Option<String>, cx: &mut Context<Shell>) {
    if g.sync.turning_on.as_ref().is_some_and(|t| t.failed.is_none()) {
        return;
    }
    let first = if repo.is_some() { "Connecting to your repository…" } else { "Signing in to GitHub…" };
    g.sync.turning_on = Some(TurningOn { said: first.into(), ..Default::default() });
    g.sync.code_copied = false;
    cx.notify();
    let mut rx = nami::setup_sync_on(repo);
    cx.spawn(async move |this, cx| {
        use futures::StreamExt;
        while let Some(e) = rx.next().await {
            let gone = this
                .update(cx, |g, cx| {
                    let Some(t) = &mut g.sync.turning_on else { return };
                    match e {
                        nami::Event::Step { step, message, .. } if step == "code" => t.code = Some(message),
                        nami::Event::Step { step, .. } if step == "done" => {}
                        nami::Event::Step { message, .. } => {
                            t.code = None;
                            t.said = message;
                        }
                        nami::Event::Finished(Ok(())) => {
                            g.sync.turning_on = None;
                            g.sync.other_repo = false;
                            look_setup(cx);
                        }
                        nami::Event::Finished(Err(why)) => t.failed = Some(why),
                    }
                    cx.notify();
                })
                .is_err();
            if gone {
                return;
            }
        }
    })
    .detach();
}

/// "2 minutes ago", "3 days ago".
fn when(t: u64) -> String {
    super::install_bundle::ago(t)
}

/// Keep This Setup on All Your Computers: your user.nami, the same on each (`nami sync`).
fn setup_part(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    let title = |t: &str| div().text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).child(t.to_string());
    let s = match &g.sync.setup {
        None => return div().child(super::looking("setup-looking")),
        Some(Err(why)) => {
            return super::failed("setup-again", "Couldn't find out whether this setup is synced.", why.clone(), |g, cx| {
                g.sync.setup = None;
                look_setup(cx);
            }, cx)
        }
        Some(Ok(s)) => s.clone(),
    };

    // Turning it on: each step, and the code to enter on GitHub.
    if let Some(t) = g.sync.turning_on.clone() {
        let mut card = crate::look::group().flex().flex_col().gap(px(10.)).p(px(18.)).child(title("Keep This Setup on All Your Computers"));
        if let Some(why) = &t.failed {
            return card.child(super::trouble("setup-on-failed", "Couldn't turn on syncing.", why.clone(), cx)).child(
                div().flex().gap(px(8.))
                    .child(button("setup-on-again", "Try Again", true, true).on_click(cx.listener(|g, _, _, cx| {
                        let repo = g.sync.other_repo.then(|| g.setup_repo.read(cx).content.trim().to_string()).filter(|r| !r.is_empty());
                        g.sync.turning_on = None;
                        turn_on(g, repo, cx);
                    })))
                    .child(button("setup-on-cancel", "Cancel", false, true).on_click(cx.listener(|g, _, _, cx| {
                        g.sync.turning_on = None;
                        cx.notify();
                    }))),
            );
        }
        match &t.code {
            Some(code) => {
                let code = code.clone();
                card = card
                    .child(muted("Your browser opens GitHub. Enter this code there to sign in:"))
                    .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).font_family("Geist Mono").child(code.clone()))
                    .child(div().flex().gap(px(8.))
                        .child(button("setup-code-copy", if g.sync.code_copied { "Copied" } else { "Copy Code" }, false, true).on_click(cx.listener(move |g, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                            g.sync.code_copied = true;
                            cx.notify();
                        })))
                        .child(button("setup-code-open", "Open GitHub", false, true).on_click(|_, _, cx| cx.open_url("https://github.com/login/device"))));
            }
            None => card = card.child(div().flex().items_center().gap(px(10.)).child(crate::motion::looking("setup-turning", "")).child(muted(t.said.clone()))),
        }
        return card;
    }

    if !s.on {
        let mut card = crate::look::group().flex().flex_col().gap(px(10.)).p(px(18.))
            .child(div().flex().items_center().gap(px(10.)).child(glyph("sync", 20., color::key())).child(title("Keep This Setup on All Your Computers")))
            .child(muted("Your apps and settings stay the same on each computer you turn this on, on namiOS and other Linux distributions alike. They're kept in a private repository of your own, in your GitHub account or on any Git server, and nowhere else."));
        if g.sync.other_repo {
            card = card
                .child(muted("Enter the repository's address. Git signs in to it as it does for you (an SSH key, or a saved password)."))
                .child(div().flex().items_center().gap(px(8.))
                    .child(div().flex_1().child(g.setup_repo.clone()))
                    .child(button("setup-other-on", "Turn On", true, true).on_click(cx.listener(|g, _, _, cx| {
                        let repo = g.setup_repo.read(cx).content.trim().to_string();
                        if !repo.is_empty() {
                            turn_on(g, Some(repo), cx);
                        }
                    })))
                    .child(button("setup-other-cancel", "Cancel", false, true).on_click(cx.listener(|g, _, _, cx| {
                        g.sync.other_repo = false;
                        cx.notify();
                    }))));
        } else {
            card = card.child(div().flex().items_center().gap(px(16.))
                .child(button("setup-github", "Sign In with GitHub…", true, true).on_click(cx.listener(|g, _, _, cx| turn_on(g, None, cx))))
                .child(div().id("setup-other").text_size(px(text::CALLOUT)).text_color(rgb(color::key())).cursor_pointer().press().hover(|s| s.underline()).child("Use Another Git Repository…").on_click(cx.listener(|g, _, _, cx| {
                    g.sync.other_repo = true;
                    cx.notify();
                }))));
        }
        return card;
    }

    // On: how it's going, the computers, and what came in lately.
    let state = match (&s.last.error, s.last.ok) {
        (Some(_), _) => "Couldn't sync the last time it tried. It tries again every 15 minutes.".to_string(),
        (None, Some(t)) => format!("Up to date · synced {}", when(t)),
        (None, None) => "Not synced yet".to_string(),
    };
    let busy = g.sync.busy;
    let mut card = crate::look::group().flex().flex_col().gap(px(12.)).p(px(18.))
        .child(div().flex().items_center().gap(px(12.))
            .child(glyph("sync", 20., color::key()))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(title("Keep This Setup on All Your Computers"))
                .child(muted(busy.map(str::to_string).unwrap_or(state))))
            .child(button("setup-now", "Sync Now", false, busy.is_none()).when(busy.is_none(), |b| b.on_click(cx.listener(|g, _, _, cx| run_setup(g, "Syncing…", vec!["now".into()], "Couldn't sync.", cx)))))
            .child(button("setup-off", "Turn Off…", false, busy.is_none()).when(busy.is_none(), |b| b.on_click(cx.listener(|g, _, _, cx| {
                g.sync.asking_off = true;
                cx.notify();
            })))));
    if let Some(e) = &s.last.error {
        card = card.child(super::trouble("setup-error", "The last sync didn't finish.", e.clone(), cx));
    }
    if let Some(e) = &s.last.apply_error {
        card = card.child(super::trouble("setup-apply-error", "Some apps from your other computers couldn't be installed here.", e.clone(), cx));
    }
    if g.sync.asking_off {
        card = card.child(div().flex().flex_col().gap(px(8.)).pt(px(10.)).border_t_1().border_color(color::hairline())
            .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child("Stop syncing this computer?"))
            .child(muted("Your apps and settings stay as they are here. Your other computers keep syncing with each other."))
            .child(div().flex().gap(px(8.))
                .child(button("setup-off-yes", "Turn Off", true, true).on_click(cx.listener(|g, _, _, cx| {
                    g.sync.asking_off = false;
                    run_setup(g, "Turning off…", vec!["off".into()], "Couldn't turn off syncing.", cx);
                })))
                .child(button("setup-off-no", "Cancel", false, true).on_click(cx.listener(|g, _, _, cx| {
                    g.sync.asking_off = false;
                    cx.notify();
                })))));
    }
    if let Some(repo) = &s.repo {
        card = card.child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(format!("Kept in {repo}")));
    }

    // The computers.
    let mut page = div().flex().flex_col().gap(px(10.)).child(card);
    if !s.computers.is_empty() {
        page = page.child(section("Your Computers"));
        let mut l = list();
        for (i, c) in s.computers.iter().enumerate() {
            let name = if c.this { format!("{} (This Computer)", c.name) } else { c.name.clone() };
            l = l.child(row(ElementId::NamedInteger("setup-comp".into(), i as u64), i == 0)
                .child(glyph("computer", 18., color::muted()))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(1.))
                    .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(name))
                    .child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(c.system.clone())))
                .child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(if c.this { s.last.ok.map(when).unwrap_or_default() } else { when(c.last) })));
        }
        page = page.child(l);
    }
    // What came in lately, each to undo (History has them too).
    let came: Vec<nami::SetupEntry> = s.entries.iter().take(5).cloned().collect();
    if !came.is_empty() {
        page = page.child(section("Recent Changes")).child(changes_list(g, &came, "setup-recent", cx));
    }
    page
}

/// Changes a sync made here, each with when, what the other computer said where both changed
/// one thing, and Undo.
pub fn changes_list(g: &Shell, entries: &[nami::SetupEntry], id: &'static str, cx: &mut Context<Shell>) -> Div {
    let busy = g.sync.busy.is_some();
    let mut l = list();
    for (i, e) in entries.iter().enumerate() {
        let n = e.id;
        let clash = e.clashes.first().map(|c| {
            let other = if c.took_theirs { c.ours.clone() } else { c.theirs.clone() }.unwrap_or_else(|| "nothing".into());
            let whose = if c.took_theirs { "this computer's" } else { "the other computer's" };
            format!("Both computers changed {}; the newer change was kept ({whose} was {other}).", c.path)
        });
        l = l.child(row(ElementId::NamedInteger(id.into(), i as u64), i == 0)
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(1.))
                .child(div().text_size(px(text::BODY)).when(e.undone, |d| d.text_color(rgb(color::muted()))).child(e.said()))
                .child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(when(e.when)))
                .children(clash.map(|c| div().text_size(px(text::META)).text_color(rgb(color::muted())).child(c))))
            .child(if e.undone {
                div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child("Undone").into_any_element()
            } else {
                button("setup-undo", "Undo", false, !busy).id(ElementId::NamedInteger(format!("{id}-undo").into(), n)).when(!busy, |b| b.on_click(cx.listener(move |g, _, _, cx| run_setup(g, "Undoing…", vec!["undo".into(), n.to_string()], "Couldn't undo the change.", cx)))).into_any_element()
            }));
    }
    l
}

/// What your file says: its folders and computers (name, value).
fn listed(g: &Shell, what: &str) -> Vec<(String, String)> {
    g.user.get("files").and_then(|f| f.get(CONF)).and_then(|c| c.get(what)).and_then(Value::as_object).map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect()).unwrap_or_default()
}

fn set_up(g: &Shell) -> bool {
    crate::pages::packages(&g.user).iter().any(|e| crate::pages::entry_name(e) == "sync")
}

fn size(bytes: u64) -> String {
    match bytes {
        0..1_000_000 => format!("{} KB", bytes.div_ceil(1000)),
        1_000_000..1_000_000_000 => format!("{} MB", bytes / 1_000_000),
        _ => format!("{:.1} GB", bytes as f64 / 1e9),
    }
}

fn muted(text: impl Into<String>) -> Div {
    div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(text.into())
}

fn section(text: &str) -> Div {
    crate::look::group_heading(text).mt(px(6.))
}

fn list() -> Div {
    crate::look::group().flex().flex_col().overflow_hidden()
}

fn row(id: ElementId, first: bool) -> gpui::Stateful<Div> {
    div().id(id).flex().items_center().gap(px(12.)).min_h(px(crate::look::shape::GROUP_ROW)).px(px(crate::look::shape::MODULE_INSET)).py(px(6.)).when(!first, |r| r.border_t_1().border_color(color::hairline()))
}

fn dot(on: bool) -> Div {
    div().size(px(8.)).flex_none().rounded_full().bg(rgb(if on { color::good() } else { color::faint() }))
}

fn remove(id: ElementId, what: &'static str, name: String, cx: &mut Context<Shell>) -> gpui::Stateful<Div> {
    div().id(id).px(px(8.)).text_size(px(text::SECTION)).text_color(rgb(color::muted())).cursor_pointer().press().hover(|s| s.text_color(rgb(color::text()))).child("×").on_click(cx.listener(move |g, _, _, cx| {
        // At once, with an Undo that lists it again.
        let was = g.user.get("files").and_then(|f| f.get(CONF)).and_then(|c| c.get(what)).and_then(|m| m.get(&name)).cloned().unwrap_or(Value::Null);
        let kind = if what == "folders" { "folder" } else { "computer" };
        let taking = crate::Taking {
            about: crate::CHANGING.into(),
            doing: format!("Taking {name} off sync"),
            said: format!("The {kind} {name} is no longer synced."),
            back: json!({ "set": { "path": ["files", CONF, what, name], "value": was } }),
            back_about: crate::CHANGING.into(),
            back_doing: format!("Syncing {name} again"),
        };
        g.edit_with_undo(false, json!({ "unset": { "path": ["files", CONF, what, name] } }), taking, cx);
    }))
}

pub fn render(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    let page = div().flex().flex_col().gap(px(16.)).child(setup_part(g, cx)).child(section("Sync Folders")).child(crate::ui::intro("Keep folders in sync across your computers, directly between them. No cloud or account needed."));
    if !set_up(g) {
        return page.child(
            crate::look::group().flex().flex_col().gap(px(10.)).p(px(18.))
                .child(div().text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).child("Sync Folders Between Your Computers"))
                .child(muted("Changes on one computer appear on the others, over your network or encrypted across the internet. Powered by Syncthing. Android phones can join with Syncthing-Fork."))
                .child(div().flex().child(button("sync-setup", "Set Up Sync", false, true).on_click(cx.listener(|g, _, _, cx| set_up_now(g, cx))))),
        );
    }
    let folders = listed(g, "folders");
    let computers = listed(g, "computers");
    let status = match g.sync.status.clone() {
        None => return page.child(super::looking("sync-looking")),
        Some(None) => None,
        Some(Some(Err(why))) => return page.child(super::failed("sync-again", "Syncthing isn't responding.", why, |g, cx| {
            g.sync.status = None;
            look(cx);
        }, cx)),
        Some(Some(Ok(s))) => Some(s),
    };

    // This computer's ID, to give the others.
    let mut page = page;
    match &status {
        Some(s) => {
            let id = s.id.clone();
            page = page.child(
                crate::look::group().flex().items_center().gap(px(14.)).p(px(16.))
                    .child(glyph("computer", 20., color::muted()))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(4.))
                        .child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child("This computer's ID. Add it on your other computers."))
                        .child(div().text_size(px(text::CALLOUT)).font_family("Geist Mono").child(crate::tip::cut(id.clone()))))
                    .child(button("sync-copy", if g.sync.copied { "Copied" } else { "Copy" }, false, true).on_click(cx.listener(move |g, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
                        g.sync.copied = true;
                        cx.notify();
                    }))),
            );
        }
        None => page = page.child(muted("Syncthing is starting. This computer's ID will appear here in a moment.")),
    }

    // The folders.
    page = page.child(section("Folders"));
    let mut l = list();
    for (i, (name, path)) in folders.iter().enumerate() {
        let st = status.as_ref().and_then(|s| s.folders.iter().find(|f| f.name == *name));
        let state = match st {
            None => "Not synced yet".to_string(),
            Some(f) if f.need > 0 => format!("Syncing, {} remaining", size(f.need)),
            Some(f) if f.state == "idle" => format!("Up to date · {}", size(f.bytes)),
            Some(f) => format!("{} · {}", f.state[..1].to_uppercase() + &f.state[1..], size(f.bytes)),
        };
        l = l.child(
            row(ElementId::NamedInteger("sync-folder".into(), i as u64), i == 0)
                .child(glyph("folder", 18., color::muted()))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(1.))
                    .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(name.clone()))
                    .child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(path.clone())))
                .child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(state))
                .child(remove(ElementId::NamedInteger("sync-folder-off".into(), i as u64), "folders", name.clone(), cx)),
        );
    }
    page = page.child(l.child(adder(g, "folders", folders.is_empty(), cx)));

    // The other computers.
    page = page.child(section("Your Other Computers"));
    let mut l = list();
    for (i, (name, id)) in computers.iter().enumerate() {
        let online = status.as_ref().and_then(|s| s.computers.iter().find(|c| c.name == *name)).is_some_and(|c| c.online);
        l = l.child(
            row(ElementId::NamedInteger("sync-comp".into(), i as u64), i == 0)
                .child(dot(online))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(1.))
                    .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(name.clone()))
                    .child(div().text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::muted())).child(crate::tip::cut(id.clone()))))
                .child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(if online { "Online" } else { "Offline" }))
                .child(remove(ElementId::NamedInteger("sync-comp-off".into(), i as u64), "computers", name.clone(), cx)),
        );
    }
    page = page.child(l.child(adder(g, "computers", computers.is_empty(), cx)));
    page.child(div().flex().items_center().gap(px(16.))
        .child(muted("Add each computer once by its ID. Every folder syncs to every computer."))
        .child(div().id("sync-web").text_size(px(text::CALLOUT)).text_color(rgb(color::key())).cursor_pointer().press().hover(|s| s.underline()).child("Open Syncthing").on_click(|_, _, cx| cx.open_url("http://127.0.0.1:8384"))))
}

/// The last row of a list: "Add a folder" / "Add a computer", or its fields.
fn adder(g: &mut Shell, what: &'static str, first: bool, cx: &mut Context<Shell>) -> gpui::Stateful<Div> {
    let open = g.sync.adding == Some(what);
    let id = ElementId::Name(format!("sync-add-{what}").into());
    if !open {
        let label = if what == "folders" { "+  Add Folder…" } else { "+  Add Computer…" };
        return row(id, first).cursor_pointer().press_row().hover(|s| s.bg(color::hover_pill())).child(div().text_size(px(text::BODY)).text_color(rgb(color::muted())).child(label)).on_click(cx.listener(move |g, _, _, cx| {
            g.sync.adding = Some(what);
            cx.notify();
        }));
    }
    let (a, b) = if what == "folders" { (g.sync_name.clone(), g.sync_path.clone()) } else { (g.sync_name.clone(), g.sync_id.clone()) };
    row(id, first)
        .child(div().w(px(150.)).child(a))
        .child(div().flex_1().child(b))
        .child(button("sync-add", "Add", true, true).on_click(cx.listener(move |g, _, _, cx| {
            let name = g.sync_name.read(cx).content.trim().to_string();
            let value = if what == "folders" { g.sync_path.read(cx).content.trim().to_string() } else { g.sync_id.read(cx).content.trim().to_uppercase() };
            if name.is_empty() || value.is_empty() {
                return;
            }
            g.edit(false, json!({ "set": { "path": ["files", CONF, what, name], "value": value } }), cx);
            for input in [g.sync_name.clone(), g.sync_path.clone(), g.sync_id.clone()] {
                input.update(cx, |t, cx| t.set(cx, ""));
            }
            g.sync.adding = None;
            cx.notify();
        })))
        .child(pill("sync-cancel".into(), "Cancel", false).h(px(30.)).on_click(cx.listener(|g, _, _, cx| {
            g.sync.adding = None;
            cx.notify();
        })))
}

/// In your user.nami: the sync package (copied next to the computer's
/// folder from nami's examples, as add-ons are), and a first folder, ~/Sync.
fn set_up_now(g: &mut Shell, cx: &mut Context<Shell>) {
    let Some(file) = g.file(false) else { return };
    let mine = file.canonicalize().unwrap_or(file);
    // laptop/ada/user.nami: the computer's folder is laptop.
    if let Some(dir) = mine.parent().and_then(|p| p.parent()).filter(|d| crate::nami::existing(d.join("system.nami")).is_file()) {
        let nami = std::env::var_os("NAMI").unwrap_or_else(|| "nami".into());
        if !dir.join("../packages/sync/package.nami").is_file() {
            if let Ok(o) = std::process::Command::new(nami).arg("new").arg(dir).args(["--from", "sync"]).output() {
                if !o.status.success() {
                    g.problem = Some(crate::Problem::of("Couldn't set up sync.", String::from_utf8_lossy(&o.stderr)));
                    cx.notify();
                    return;
                }
            }
        }
    }
    g.edit(false, json!({ "add": { "path": ["packages"], "value": "sync" } }), cx);
    g.edit(false, json!({ "set": { "path": ["files", CONF, "folders", "Sync"], "value": "~/Sync" } }), cx);
}
