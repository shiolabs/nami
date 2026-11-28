//! Apps: yours (My apps: ~/user.nami) or everyone's (the computer's, for
//! admins: system.nami, with nami's add-ons set up on it). What the file
//! lists, each to take off, and any app of the catalog to add by searching.

use std::time::Duration;

use gpui::prelude::*;
use crate::look::Press;
use gpui::{Context, Div, ElementId, FontWeight, ParentElement, Stateful, StatefulInteractiveElement, Styled, div, rgb};
use crate::look::px;
use crate::look::text;
use serde_json::{Value, json};

use super::explore::{small_action, Explorer};
use super::{entry_name, explore, is_folder, packages};
use crate::nami::{self, Found};
use crate::ui::{addon, addon_list, addons, app_chip, app_row_with, choice, color, need_row, pick_frame, pick_head, pill, KERNELS, NEEDS, NVIDIA};
use crate::{jobs, Shell};

/// A chip's label, with how its app goes if it's going on or off.
fn chip_label(g: &Shell, attr: &str, label: &str) -> String {
    match g.job(attr).map(|j| (&j.state, j.kind)) {
        Some((jobs::State::Waiting, _)) => format!("{label}: waiting"),
        Some((jobs::State::Going, k)) => format!("{label}: {}…", k.verb().to_lowercase()),
        Some((jobs::State::Failed(_), k)) => format!("{label}: couldn't {}", k.plain()),
        _ => label.to_string(),
    }
}

/// The search words changed: search once they've stayed a moment.
pub fn search_changed(g: &mut Shell, cx: &mut Context<Shell>) {
    let q = g.text(&g.search, cx);
    if q.chars().count() < 2 {
        g.found_flathub = None;
        if g.found.take().is_some() {
            cx.notify();
        }
        return;
    }
    if g.found.as_ref().is_some_and(|(w, _)| *w == q) || g.searching.as_deref() == Some(q.as_str()) {
        return;
    }
    g.searching = Some(q.clone());
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(Duration::from_millis(250)).await;
        if !this.update(cx, |g, cx| g.text(&g.search, cx) == q).unwrap_or(false) {
            let _ = this.update(cx, |g, _| g.searching = None);
            return;
        }
        let words = q.clone();
        let found = cx.background_executor().spawn(async move { nami::search_all(&words) }).await;
        let still = this
            .update(cx, |g, cx| {
                g.searching = None;
                let still = g.text(&g.search, cx) == q;
                if still {
                    g.found = Some((q.clone(), found));
                    cx.notify();
                }
                still && g.found_flathub.as_ref().is_none_or(|(w, _)| *w != q)
            })
            .unwrap_or(false);
        // Then Flathub's, which take longer (asked of Flathub, kept a day).
        if still {
            let words = q.clone();
            let flathub = cx.background_executor().spawn(async move { nami::search_flathub(&words) }).await;
            let _ = this.update(cx, |g, cx| {
                if g.text(&g.search, cx) == q {
                    g.found_flathub = Some((q, flathub.unwrap_or_default()));
                    cx.notify();
                }
            });
        }
    })
    .detach();
}

fn section(text: &str) -> Div {
    crate::look::group_heading(text)
}

/// Set up one of nami's add-ons on the computer (copied next to its
/// folder by `nami new`, then listed), or take it off.
fn toggle_addon(g: &mut Shell, name: &str, entry: Option<Value>, cx: &mut Context<Shell>) {
    match entry {
        Some(e) => g.edit(true, json!({ "remove": { "path": ["packages"], "value": e } }), cx),
        None => add_example(g, name, cx),
    }
    g.apply_now(cx);
}

/// A new own pick: packages/<name>/package.nami, listed in system.nami;
/// the search adds to it after.
fn make_pick(g: &mut Shell, cx: &mut Context<Shell>) {
    let name = g.own_name.read(cx).content.trim().to_string();
    let ok = !name.is_empty() && name.len() <= 32 && name.starts_with(|c: char| c.is_ascii_lowercase()) && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let taken = g.examples.as_ref().is_ok_and(|e| e.has(&name));
    let Some(dir) = g.file(true).and_then(|f| f.parent().map(|p| p.to_path_buf())) else { return };
    let file = dir.join("../packages").join(&name).join("package.nami");
    g.problem = if !ok {
        Some("Use lowercase letters, numbers and hyphens.".into())
    } else if taken || file.exists() {
        Some(format!("A package named {name} already exists. Choose another name.").into())
    } else {
        None
    };
    if g.problem.is_some() {
        cx.notify();
        return;
    }
    let text = format!("# {name}: my own pick of apps (made with nami-gui). List it in any\n# system.nami of this repo: `- {name}`.\npackages: []\n");
    if let Err(e) = std::fs::create_dir_all(file.parent().unwrap()).and_then(|_| std::fs::write(&file, text)) {
        g.problem = Some(crate::Problem::of("Couldn't create the package.", format!("{}: {e}", file.display())));
        cx.notify();
        return;
    }
    g.before.push((file, None));
    g.own_name.update(cx, |t, cx| t.set(cx, ""));
    g.own_target = Some(name.clone());
    g.edit(true, json!({ "add": { "path": ["packages"], "value": name } }), cx);
}

/// One of nami's example packages, copied next to the computer's folder
/// (`nami new`) and listed.
pub fn add_example(g: &mut Shell, name: &str, cx: &mut Context<Shell>) {
    let again = name.to_string();
    if g.unlock_first(&format!("{name} is being set up on the system, for everyone on this computer."), move |g, cx| add_example(g, &again, cx), cx) {
        return;
    }
    let Some(dir) = g.file(true).and_then(|f| f.parent().map(|p| p.to_path_buf())) else { return };
    let out = std::process::Command::new(std::env::var_os("NAMI").unwrap_or_else(|| "nami".into())).arg("new").arg(&dir).args(["--from", name]).output();
    match out {
        Ok(o) if o.status.success() => {
            let entry = name.to_string();
            g.edit(true, json!({ "add": { "path": ["packages"], "value": entry } }), cx)
        }
        Ok(o) => {
            g.problem = Some(crate::Problem::of(format!("Couldn't add {name}."), String::from_utf8_lossy(&o.stderr)));
            cx.notify();
        }
        Err(e) => {
            g.problem = Some(crate::Problem::of(format!("Couldn't add {name}."), format!("nami: {e}")));
            cx.notify();
        }
    }
}

/// The computer's kernel: the one listed taken off, `name`'s listed
/// (none: the usual one).
fn pick_kernel(g: &mut Shell, name: &'static str, listed: Vec<Value>, cx: &mut Context<Shell>) {
    for e in listed {
        g.edit(true, json!({ "remove": { "path": ["packages"], "value": e } }), cx);
    }
    if !name.is_empty() {
        add_example(g, name, cx);
    }
}

/// What the catalog says of an app of the files, as far as it's known yet.
fn found_for(g: &Shell, attr: &str) -> Found {
    g.catalog.get(attr).cloned().unwrap_or_else(|| serde_json::from_value(json!({ "attr": attr, "title": g.app_name(attr) })).expect("a Found needs only its attr"))
}

/// An app the file lists (or one on its way), as a row: its icon, name and
/// line; Open and Remove, or how installing or removing it goes. Remove
/// asks first, on the row.
fn own_row(g: &Shell, n: usize, attr: &str, entry: Option<Value>, system: bool, cx: &mut Context<Shell>) -> Stateful<Div> {
    let f = found_for(g, attr);
    let id = |what: &str| ElementId::NamedInteger(format!("own-{what}").into(), n as u64);
    let doing = g.doing(&f);
    let action = match doing.as_ref().and_then(|d| explore::doing_button(id("act"), d, attr, cx)) {
        Some(b) => b,
        None if g.asking_remove.as_deref() == Some(attr) => {
            let (a, file) = (attr.to_string(), g.file(system));
            div().flex().flex_none().items_center().gap(px(8.))
                .child(div().text_size(px(text::CALLOUT)).font_weight(FontWeight::MEDIUM).mr(px(2.)).child(format!("Remove {}?", f.name())))
                .child(small_action(id("keep"), "Cancel".into()).on_click(cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.asking_remove = None;
                    cx.notify();
                })))
                .child(small_action(id("off"), "Remove".into()).bg(rgb(color::bad())).border_color(rgb(color::bad())).text_color(rgb(color::on_bad())).hover(|s| s.opacity(0.85)).on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if let (Some(file), Some(entry)) = (file.clone(), entry.clone()) {
                        this.uninstall(file, &a, entry, cx);
                    }
                })))
                .into_any_element()
        }
        None => {
            let a = attr.to_string();
            let open = g.launchers.get(attr).cloned();
            div().flex().flex_none().items_center().gap(px(6.))
                .child(
                    div().id(id("ask-off")).px(px(10.)).h(px(28.)).flex().items_center().rounded_full().text_size(px(text::META)).text_color(rgb(color::muted())).cursor_pointer().press_row().hover(|s| s.text_color(rgb(color::text())).bg(color::push())).child("Remove").on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.asking_remove = Some(a.clone());
                        cx.notify();
                    })),
                )
                .children(open.map(|entry| {
                    small_action(id("open"), "Open".into()).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.problem = crate::launch::open_app(&entry).err().map(crate::Problem::under("Couldn't open the app."));
                        cx.notify();
                    }))
                }))
                .into_any_element()
        }
    };
    let line = explore::line_of_app(g, id("line"), &f, false, cx);
    let found = f.clone();
    let a = attr.to_string();
    app_row_with(id("row"), &f, Some(line), action).rounded(px(0.)).px(px(crate::look::shape::MODULE_INSET)).py(px(8.)).on_click(cx.listener(move |this, _, _, cx| explore::open(this, Some(found.clone()), a.clone(), cx)))
}

pub fn render(g: &mut Shell, system: bool, cx: &mut Context<Shell>) -> Div {
    if g.explore.app.is_some() {
        return explore::app_page(g, cx);
    }
    let file = if system { g.system.clone() } else { g.user.clone() };
    let listed = packages(&file);
    let mut page = div().flex().flex_col().gap(px(16.));
    // Locked, for someone who isn't an admin: system.nami, which lists them, is an
    // admin's to read; once it's unlocked, they're here to change as an admin's are.
    if system && !g.may_change() {
        return page
            .child(crate::ui::intro("Apps for everyone on this computer. Each person can also install their own."))
            .children(super::lock_row(g, "apps-unlock", cx))
            .child(super::locked(true, div().flex().flex_col().gap(px(8.)).child(section("System Apps")).child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child("Unlock to see and change the apps, add-ons and kernel for everyone on this computer."))));
    }
    page = page.child(if system {
        crate::ui::intro("Apps for everyone on this computer. Each person can also install their own.")
    } else if g.admin() {
        crate::ui::intro("Apps you install are only for you. To install an app for everyone on this computer, open it and click Install on System.")
    } else {
        crate::ui::intro("Apps only you can use. Other people on this computer don't get them.")
    });
    // What the file lists (nami's add-ons are below), and what's on its way.
    let dir = g.file(system).and_then(|f| f.parent().map(|p| p.to_path_buf()));
    let path = g.file(system);
    // (Not the libraries an app from a download needed: they go with it.)
    let for_downloads: Vec<String> = if system { Vec::new() } else { g.install_file.apps.iter().flat_map(|a| a.libraries.clone()).collect() };
    let mut own: Vec<(String, Option<Value>)> = listed.iter().filter(|e| !(system && is_folder(dir.as_deref(), e)) && !for_downloads.contains(&entry_name(e))).map(|e| (entry_name(e), Some(e.clone()))).collect();
    for j in &g.jobs {
        // On its way in, or out but not gone yet.
        let coming = j.state != jobs::State::Done && j.ops.iter().any(|o| o.to_string().contains("[\"packages\"]"));
        if coming && Some(&j.file) == path.as_ref() && !own.iter().any(|(a, _)| *a == j.attr) {
            own.push((j.attr.clone(), None));
        }
    }
    page = page.child(section(if system { "System Apps" } else { "Installed" }));
    if own.is_empty() {
        page = page.child(div().mt(px(-8.)).text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child("No apps yet. Find apps to install below."));
    } else {
        let rows: Vec<_> = own.into_iter().enumerate().map(|(n, (attr, entry))| own_row(g, n, &attr, entry, system, cx)).collect();
        page = page.child(
            crate::look::group().flex().flex_col().overflow_hidden()
                .children(rows.into_iter().enumerate().map(|(i, r)| r.when(i > 0, |r| r.border_t_1().border_color(color::hairline())))),
        );
    }

    // Apps installed from downloads (a .deb, .rpm, AppImage, tarball): yours too.
    if !g.install_file.apps.is_empty() {
        let rows = crate::pages::install_file::rows(g, cx);
        page = page
            .child(section("From Downloads").mt(px(6.)))
            .child(div().mt(px(-8.)).text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child("Apps you installed from downloaded files, such as nami bundles or packages made for other distributions. To install another, open its file."))
            .child(
                crate::look::group().flex().flex_col().overflow_hidden()
                    .children(rows.into_iter().enumerate().map(|(i, r)| r.when(i > 0, |r| r.border_t_1().border_color(color::hairline())))),
            );
    }

    page = page.child(section("Find Apps").mt(px(10.))).child(div().mt(px(-6.)).child(super::discover::search_field(g, cx)));
    if let Some(pick) = g.own_target.clone().filter(|_| system) {
        page = page.child(
            div().mt(px(-8.)).flex().items_center().gap(px(8.))
                .child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(format!("Apps you find are added to {pick}.")))
                .child(pill("pick-done".into(), "Done", false).h(px(26.)).text_size(px(text::META)).on_click(cx.listener(|this, _, _, cx| {
                    this.own_target = None;
                    cx.notify();
                }))),
        );
    }
    let have: Vec<String> = packages(&file).iter().map(entry_name).collect();
    let pick_file = |name: &str| dir.as_ref().map(|d| d.join("../packages").join(name).join("package.nami"));
    match &g.found {
        Some((q, Ok(found))) if found.is_empty() => page = page.child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(format!("No results for “{q}”. Try a different search."))),
        Some((_, Ok(found))) => {
            let mut list = div().flex().flex_col();
            for (n, f) in found.iter().take(8).cloned().enumerate() {
                let action = explore::action_of(g, ElementId::NamedInteger("found-add".into(), n as u64), &f, cx);
                let line = explore::line_of_app(g, ElementId::NamedInteger("found-line".into(), n as u64), &f, false, cx);
                let (found, attr) = (f.clone(), f.attr.clone());
                list = list.child(app_row_with(ElementId::NamedInteger("found".into(), n as u64), &f, Some(line), action).on_click(cx.listener(move |this, _, _, cx| explore::open(this, Some(found.clone()), attr.clone(), cx))));
            }
            page = page.child(list);
        }
        Some((_, Err(why))) => {
            page = page.child(super::failed("search-again", "Couldn't search the catalog.", why.clone(), |g, cx| {
                g.found = None;
                search_changed(g, cx);
            }, cx))
        }
        None if g.text(&g.search, cx).chars().count() < 2 => page = page.child(explore::browse(g, cx)),
        // (Searching: said only if it takes a moment.)
        None => page = page.child(super::looking("search-looking")),
    }
    // Flathub's (yours only: they're each person's own), below the catalog's.
    let words = g.text(&g.search, cx);
    if let Some((_, flathub)) = g.found_flathub.clone().filter(|(w, f)| !system && *w == words && !f.is_empty()) {
        let mut list = div().flex().flex_col();
        for (n, f) in flathub.into_iter().take(6).enumerate() {
            let action = explore::action_of(g, ElementId::NamedInteger("flathub-add".into(), n as u64), &f, cx);
            let line = explore::line_of_app(g, ElementId::NamedInteger("flathub-line".into(), n as u64), &f, false, cx);
            let (found, attr) = (f.clone(), f.attr.clone());
            list = list.child(app_row_with(ElementId::NamedInteger("flathub".into(), n as u64), &f, Some(line), action).on_click(cx.listener(move |this, _, _, cx| explore::open(this, Some(found.clone()), attr.clone(), cx))));
        }
        // (On a Mac, Homebrew's casks: nami::search_flathub.)
        let (title, said) = if crate::look::MAC {
            ("Mac Apps", super::discover::MAC_CASKS)
        } else {
            ("From Flathub", "Apps from Flathub, published by their developers. Each runs in its own sandbox and is installed only for you.")
        };
        page = page
            .child(section(title).mt(px(6.)))
            .child(div().mt(px(-8.)).text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(said))
            .child(list);
    }
    if system {
        let without: Vec<String> = crate::pages::list_at(&file, "without");
        let known = |n: &str| g.examples.as_ref().is_ok_and(|e| e.packages.iter().any(|p| p.name == n));
        let rows = NEEDS.iter().filter(|n| known(n.name)).map(|need| {
            let name = need.name;
            let entry = listed.iter().find(|e| is_folder(dir.as_deref(), e) && entry_name(e) == name).cloned();
            let chips = need.apps.iter().map(|&(attr, label)| {
                let off = without.iter().any(|w| w == attr);
                app_chip(ElementId::Name(format!("need-{name}-{attr}").into()), attr, &chip_label(g, attr, label), !off).on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let Some(file) = this.file(true) else { return };
                    let (op, kind) = if off { ("remove", jobs::Kind::Install) } else { ("add", jobs::Kind::Remove) };
                    this.change_alone(file, attr, kind, vec![json!({ op: { "path": ["without"], "value": attr } })], cx);
                }))
            }).collect();
            let more = need.more.iter().map(|&(attr, label)| {
                let added = have.iter().any(|h| h == attr);
                app_chip(ElementId::Name(format!("more-{name}-{attr}").into()), attr, &chip_label(g, attr, label), added).on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let Some(file) = this.file(true) else { return };
                    if added {
                        this.uninstall(file, attr, json!(attr), cx);
                    } else {
                        this.install(file, attr, json!(attr), None, cx);
                    }
                }))
            }).collect();
            need_row(ElementId::Name(format!("need-{name}").into()), need, entry.is_some(), chips, more).on_click(cx.listener(move |this, _, _, cx| toggle_addon(this, name, entry.clone(), cx)))
        });
        // Your own picks: package folders of the repo that aren't nami's.
        let nami_s = |n: &str| g.examples.as_ref().is_ok_and(|e| e.has(n));
        let picks: Vec<(String, Value)> = listed.iter().filter(|e| is_folder(dir.as_deref(), e) && !nami_s(&entry_name(e))).map(|e| (entry_name(e), e.clone())).collect();
        let own_rows = picks.into_iter().map(|(name, entry)| {
            let pfile = pick_file(&name);
            let apps: Vec<String> = pfile.as_ref().and_then(|f| crate::nami::get(f).ok()).map(|v| packages(&v).iter().map(entry_name).collect()).unwrap_or_default();
            let picked = g.own_target.as_deref() == Some(name.as_str());
            let chips = apps.into_iter().map(|app| {
                let (pfile, a) = (pfile.clone(), app.clone());
                app_chip(ElementId::Name(format!("pick-{name}-{app}").into()), &app, &app, true).on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if let Some(f) = pfile.clone() {
                        this.edit_file(f, json!({ "remove": { "path": ["packages"], "value": a } }), cx);
                        this.apply_now(cx);
                    }
                }))
            });
            let off = {
                let entry = entry.clone();
                pill(ElementId::Name(format!("pick-off-{name}").into()), "Remove from This Computer", false).h(px(26.)).text_size(px(text::META)).on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.own_target = None;
                    this.edit(true, json!({ "remove": { "path": ["packages"], "value": entry } }), cx);
                    this.apply_now(cx);
                }))
            };
            let hint = if picked { format!("Apps you find are added to {name}") } else { "Click to add apps using search".to_string() };
            let n = name.clone();
            pick_frame(ElementId::Name(format!("pick-{name}").into()), picked)
                .child(pick_head(ElementId::Name(format!("pick-{name}-tick").into()), true, &name, &format!("Your package, in packages/{name}")))
                .child(div().pl(px(30.)).flex().flex_wrap().items_center().gap(px(6.)).children(chips).child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(hint)))
                .child(div().pl(px(30.)).flex().child(off))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.own_target = if this.own_target.as_deref() == Some(n.as_str()) { None } else { Some(n.clone()) };
                    cx.notify();
                }))
        });
        // A new one: its name, then apps from the search.
        let make = pick_frame("pick-new".into(), false)
            .cursor_default()
            .child(
                div().flex().items_center().gap(px(10.))
                    .child(div().size(px(20.)).flex_none().rounded(px(crate::look::shape::SMALL)).border_1().border_color(color::hairline()).flex().items_center().justify_center().text_size(px(text::BODY)).text_color(rgb(color::muted())).child("+"))
                    .child(div().flex().flex_col().flex_1().min_w_0().child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child("New Package")).child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child("Name a package, then add apps to it using search. You can use it on your other computers too."))),
            )
            .child(
                div().pl(px(30.)).flex().items_center().gap(px(8.))
                    .child(div().w(px(220.)).child(g.own_name.clone()))
                    .child(pill("pick-make".into(), "Create", true).on_click(cx.listener(|this, _, _, cx| make_pick(this, cx)))),
            );
        page = page
            .child(section("Uses").mt(px(10.)))
            .child(div().mt(px(-8.)).text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child("Select what this computer is used for. Each adds a set of apps, which you can change."))
            .child(div().flex().flex_col().gap(px(8.)).children(rows).children(own_rows).child(make));
        let has = |n: &str| g.examples.as_ref().is_ok_and(|e| e.packages.iter().any(|p| p.name == n));
        let offered = addon_list(has, g.gpu.as_ref(), true);
        page = page.child(section("Set Up on This Computer").mt(px(10.))).child(addons(offered.into_iter().map(|(name, title, line)| {
            // Either NVIDIA driver listed is the NVIDIA card's, on.
            let same = |n: &str| n == name || name.starts_with("nvidia") && NVIDIA.iter().any(|(d, _, _)| *d == n);
            let entry = listed.iter().find(|e| is_folder(dir.as_deref(), e) && same(&entry_name(e))).cloned();
            addon(ElementId::Name(format!("addon-{name}").into()), &title, &line, entry.is_some()).on_click(cx.listener(move |this, _, _, cx| toggle_addon(this, &name, entry.clone(), cx)))
        }).collect()));
    }
    // The kernel, on a computer nami made (elsewhere the distro's is kept).
    let kernels: Vec<_> = g.examples.as_ref().map(|e| KERNELS.iter().filter(|(n, _, _)| n.is_empty() || e.packages.iter().any(|p| p.name == *n)).copied().collect()).unwrap_or_default();
    if system && g.nami_os && kernels.len() > 1 {
        let on: Vec<Value> = listed.iter().filter(|e| is_folder(dir.as_deref(), e) && entry_name(e).starts_with("kernel-")).cloned().collect();
        let names: Vec<String> = on.iter().map(entry_name).collect();
        let cards = kernels.into_iter().map(|(name, title, line)| {
            let picked = if name.is_empty() { names.is_empty() } else { names.iter().any(|n| n == name) };
            let on = on.clone();
            choice(ElementId::Name(format!("kernel-{name}").into()), title, line, picked).w_full().min_w_0().when(!picked, |c| c.on_click(cx.listener(move |this, _, _, cx| pick_kernel(this, name, on.clone(), cx))))
        });
        page = page
            .child(section("Kernel").mt(px(10.)))
            .child(div().mt(px(-8.)).text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child("The kernel is used after you apply and restart. The previous one stays in the startup menu."))
            .child(crate::fit::grid(260., 8., 2).children(cards));
    }
    // (Unlocked, for someone who isn't an admin: Lock, at the foot of what it unlocked.)
    let lock = (system && !g.admin()).then(|| super::lock_row(g, "apps-unlock", cx)).flatten();
    page.children(lock).when(!system && !g.admin(), |p| p.child(div().text_size(px(text::META)).text_color(rgb(color::muted())).mt(px(8.)).child("Apps for everyone on this computer are in System Apps. An administrator's name and password are needed to change them.")))
}

/// On another distribution, what System covers there, in a line: the
/// computer's apps and services, while its people and settings stay the
/// distribution's (on namiOS, System is the whole computer). None on namiOS.
pub fn everyone_here() -> Option<String> {
    (!crate::look::on_namios()).then(|| everyone_on(&super::move_to::linux_name().unwrap_or_else(|| "this distribution".into())))
}

fn everyone_on(distro: &str) -> String {
    format!("On {distro}, System covers apps and services. People, settings and the desktop stay {distro}'s own.")
}

#[cfg(test)]
mod tests {
    #[test]
    fn what_everyone_covers_on_another_distribution() {
        assert_eq!(super::everyone_on("Ubuntu"), "On Ubuntu, System covers apps and services. People, settings and the desktop stay Ubuntu's own.");
        assert_eq!(super::everyone_on("this distribution"), "On this distribution, System covers apps and services. People, settings and the desktop stay this distribution's own.");
    }
}
