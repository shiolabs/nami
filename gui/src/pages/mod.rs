//! The window's pages: yours (My apps), the computer's for admins
//! (Apps, People, Settings), History, and installing from the USB drive.

pub mod app_updates;
pub mod apps;
pub mod dev;
pub mod discover;
pub mod explore;
pub mod history;
pub mod install_bundle;
pub mod install_file;
pub mod installed;
pub mod move_to;
pub mod projects;
pub mod report;
pub mod requests;
pub mod storage;
pub mod sync;
pub mod uninstall_nami;
pub mod updates;
pub mod your_setup;

use serde_json::Value;
use crate::look::text;

/// Esc, where nothing nearer took it: one step out, as a Mac's sheets and pages go. What's
/// being asked ("Remove?", "Keep these display settings?") is answered no, a failure's said
/// is put away, an app's page goes back to its list, a page General lists back to General.
/// Whether there was a step to take (at the top, Esc leaves the window as it is).
pub fn back(g: &mut crate::Shell, cx: &mut gpui::Context<crate::Shell>) -> bool {
    use crate::Page;
    use crate::os::{Active, Os};
    let page = g.page;
    if uninstall_nami::back(g) || storage::back(g) || g.asking_remove.take().is_some() || g.install_file.asking.take().is_some() || std::mem::take(&mut g.install_file.asking_remove) || g.history_view.asking.take().is_some() || Active::back_asking(g, cx) {
    } else if g.run.as_ref().is_some_and(|r| matches!(r.finished, Some(Err(_)))) {
        g.run = None;
    } else if g.details.take().is_some() {
    } else if matches!(page, Page::Discover | Page::Installed | Page::Apps | Page::Storage) && g.explore.app.take().is_some() {
    } else if page == Page::Storage {
        // (Storage is Installed's: back to it.)
        g.page = Page::Installed;
    } else if page == Page::InstallFile {
        install_file::go_back(g);
    } else if Active::back_last(g, cx) {
    } else {
        return false;
    }
    cx.notify();
    true
}

/// nami as a package manager first, as on another distribution (Ubuntu, Fedora...), or as it's
/// previewed there (`--as-other-linux`): its own pages only, none of namiOS's Settings.
pub fn store_mode() -> bool {
    use crate::os::Os;
    !crate::os::Active::on()
}

/// On a Mac, where an app with a window from nixpkgs is once it's installed: in Applications,
/// in nami Apps (nami's src/mac_apps.rs), and opened from its page here too (launch.rs's `mac_app`).
pub const MAC_APPS: &str = "If this app has a window, nami adds it to Applications, in nami Apps, once it's installed, and takes it away when you remove it. Command-line tools are ready in Terminal.";

/// What's said over the settings for everyone, locked, to someone who isn't an admin.
pub const LOCKED: &str = "An administrator's name and password are needed to change these settings.";
/// And once they're unlocked.
pub const UNLOCKED: &str = "Settings are unlocked. They lock again after 5 minutes without a change.";

/// Over what only an admin may change, for someone who isn't one, as a Mac's Settings have it
/// since Ventura: a quiet line saying why it's greyed, and Unlock… (a normal button: the page's
/// own main action keeps the accent); once unlocked, Lock. None for an admin. `id`: the page's.
pub fn lock_row(g: &crate::Shell, id: &'static str, cx: &mut gpui::Context<crate::Shell>) -> Option<gpui::Div> {
    use crate::look::{Stop, px};
    use gpui::prelude::*;
    if g.admin() || g.role.is_err() {
        return None;
    }
    let open = g.unlocked();
    let asking = g.unlock.asking;
    let label = if open { "Lock" } else if asking { "Unlocking…" } else { "Unlock…" };
    let button = crate::ui::button(id, label, false, !asking).stop(cx).aria_label(label).when(!asking, |b| {
        b.on_click(cx.listener(move |g, _, _, cx| {
            if open { g.lock_settings(cx) } else { g.unlock_settings(cx) }
        }))
    });
    Some(crate::look::group().child(
        crate::look::group_row(true)
            .child(
                gpui::div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .flex_1()
                    .min_w_0()
                    .child(crate::icons::glyph(if open { "unlock" } else { "lock" }, 15., crate::ui::color::muted()))
                    .child(gpui::div().flex_1().min_w_0().text_size(px(text::CALLOUT)).text_color(gpui::rgb(crate::ui::color::muted())).child(if open { UNLOCKED } else { LOCKED })),
            )
            .child(button),
    ))
}

/// What only an admin may change, greyed and out of reach while it's locked (`on`): seen as it
/// is, as a Mac's locked settings are; as it is otherwise.
pub fn locked(on: bool, what: impl gpui::IntoElement) -> gpui::AnyElement {
    use gpui::prelude::*;
    if !on {
        return what.into_any_element();
    }
    gpui::div().relative().child(gpui::div().opacity(0.45).child(what)).child(gpui::div().absolute().top_0().left_0().size_full().occlude()).into_any_element()
}

/// Something a page is still finding out: "Loading…", said only once it's taken a moment (300 ms),
/// so what's quick never flashes it.
pub fn looking(id: impl Into<gpui::ElementId>) -> gpui::AnyElement {
    crate::motion::looking(id, "Loading…")
}

/// What couldn't be found out, in plain words, what was said about it behind Show Details,
/// and a way to try again.
pub fn failed<V: 'static>(id: &'static str, said: impl Into<gpui::SharedString>, detail: impl Into<gpui::SharedString>, again: impl Fn(&mut V, &mut gpui::Context<V>) + 'static, cx: &mut gpui::Context<V>) -> gpui::Div {
    use crate::look::Stop;
    use gpui::prelude::*;
    let again = crate::ui::pill(gpui::ElementId::Name(id.into()), "Try Again", false).stop(cx).on_click(cx.listener(move |g, _, _, cx| {
        again(g, cx);
        cx.notify();
    }));
    told(id, said.into(), detail.into(), Some(again.into_any_element()), cx)
}

/// What went wrong, in plain words first, and what the program said of it (nami's, restic's,
/// Syncthing's own words) only behind Show Details, as a run that went wrong has it.
pub fn trouble(id: &'static str, said: impl Into<gpui::SharedString>, detail: impl Into<gpui::SharedString>, cx: &gpui::App) -> gpui::Div {
    told(id, said.into(), detail.into(), None, cx)
}

thread_local! {
    /// The troubles whose details are showing, by id: kept while the page is drawn again.
    static DETAILED: std::cell::RefCell<std::collections::HashSet<&'static str>> = std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Whether the trouble of this id is showing its details.
pub fn showing_details(id: &'static str) -> bool {
    DETAILED.with(|d| d.borrow().contains(id))
}

/// The trouble of this id is another now: its details hidden, as they start.
pub fn hide_details(id: &'static str) {
    DETAILED.with(|d| d.borrow_mut().remove(id));
}

/// Show Details pressed (or Hide Details): the other way, drawn again.
pub fn flip_details(id: &'static str, window: &mut gpui::Window) {
    DETAILED.with(|d| {
        let mut d = d.borrow_mut();
        if !d.remove(id) {
            d.insert(id);
        }
    });
    window.refresh();
}

fn told(id: &'static str, said: gpui::SharedString, detail: gpui::SharedString, again: Option<gpui::AnyElement>, cx: &gpui::App) -> gpui::Div {
    use crate::look::{Stop, color, px};
    use gpui::prelude::*;
    let detail = detail.trim().to_string();
    let shown = !detail.is_empty() && showing_details(id);
    let toggle = (!detail.is_empty()).then(|| {
        crate::ui::pill(gpui::ElementId::Name(format!("{id}-details").into()), if shown { "Hide Details" } else { "Show Details" }, false).stop(cx).on_click(move |_, window, _| flip_details(id, window))
    });
    gpui::div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            gpui::div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(crate::icons::glyph("report", 15., color::muted()))
                .child(gpui::div().flex_1().min_w_0().text_size(px(text::CALLOUT)).text_color(gpui::rgb(color::muted())).child(said))
                .children(toggle)
                .children(again),
        )
        // (Under the words, in from the mark, as it was said.)
        .when(shown, |d| d.child(gpui::div().pl(px(27.)).text_size(px(text::META)).font_family("Geist Mono").text_color(gpui::rgb(color::muted())).child(detail)))
}

/// The name of a `packages:` entry: `gimp`, `zsh` for `- zsh: {...}`, a
/// package folder's name for `../packages/tailscale`.
pub fn entry_name(entry: &Value) -> String {
    let name = match entry {
        Value::String(s) => s.clone(),
        Value::Object(m) => m.keys().next().cloned().unwrap_or_default(),
        _ => String::new(),
    };
    name.rsplit('/').next().unwrap_or(&name).to_string()
}

/// Whether a `packages:` entry is a package folder, not a catalog name: a
/// path, or a name with a folder in the repo's packages/ (`dir` is the
/// file's folder: a computer's, so ../packages).
pub fn is_folder(dir: Option<&std::path::Path>, entry: &Value) -> bool {
    let name = match entry {
        Value::String(s) => s.as_str(),
        Value::Object(m) => m.keys().next().map(String::as_str).unwrap_or(""),
        _ => return false,
    };
    name.contains('/') || dir.is_some_and(|dir| crate::nami::existing(dir.join("../packages").join(name).join("package.nami")).is_file())
}

/// A file's list at `key` (`without:`), as names.
pub fn list_at(file: &Value, key: &str) -> Vec<String> {
    file.get(key).and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default()
}

/// A file's `packages:` entries.
pub fn packages(file: &Value) -> Vec<Value> {
    file.get("packages").and_then(Value::as_array).cloned().unwrap_or_default()
}
