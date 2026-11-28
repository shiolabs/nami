//! Projects: your code's folders, each with the tools it needs (its
//! project.nami). Its tasks to run, its services to start and stop, and
//! a new one from a starter.

use std::collections::HashMap;

use gpui::prelude::*;
use crate::look::Press;
use serde_json::json;
use gpui::{ClipboardItem, Context, Div, ElementId, Entity, FontWeight, ParentElement, StatefulInteractiveElement, Styled, div, rgb};
use crate::look::px;
use crate::look::text;

use crate::input::TextInput;
use crate::nami::{self, Project, Service};
use crate::ui::{button, catalog_icon, color, icon, pill};
use crate::Shell;

/// Under a project's services on a Mac, where nami doesn't run them yet.
pub const MAC_SERVICES: &str = "On a Mac, nami doesn't start a project's services yet. Open the project in Terminal and start them there.";

/// A task running, or that ran: what it said, and how it ended.
pub struct Task {
    pub lines: Vec<String>,
    pub group: Option<u32>,
    pub finished: Option<Result<(), String>>,
    pub stopped: bool,
}

pub struct Projects {
    pub list: Option<Result<Vec<Project>, String>>,
    /// Tasks by project folder and name.
    pub tasks: HashMap<(String, String), Task>,
    pub services: HashMap<String, Vec<Service>>,
    /// Starting a new one: the starter picked, and where.
    pub new: Option<String>,
    pub folder: Entity<TextInput>,
    /// The starter's language's release lines (newest first), by package,
    /// and the one picked (none: the newest, kept up to date).
    pub lines: HashMap<String, Result<Vec<String>, String>>,
    pub version: Option<String>,
    /// Adding a tool to a project (its folder): the search, and what it found.
    pub adding: Option<String>,
    pub tool_search: Entity<TextInput>,
    pub tool_found: Option<(String, Result<Vec<nami::Found>, String>)>,
    /// The project shown (its folder; None: the first).
    pub picked: Option<String>,
    /// A service's output, followed.
    pub logs: Option<Logs>,
    /// The project whose services to add are showing.
    pub adding_service: Option<String>,
}

impl Projects {
    pub fn new(cx: &mut Context<Shell>) -> Self {
        let tool_search = cx.new(|cx| TextInput::new(cx, "Search tools, like nodejs, postgresql, ruff…", "", false));
        cx.observe(&tool_search, |g, _, cx| tool_search_changed(g, cx)).detach();
        Projects {
            list: None,
            tasks: HashMap::new(),
            services: HashMap::new(),
            new: None,
            lines: HashMap::new(),
            version: None,
            folder: cx.new(|cx| TextInput::new(cx, "~/code/shop", "", false)),
            adding: None,
            tool_search,
            tool_found: None,
            picked: None,
            logs: None,
            adding_service: None,
        }
    }
}

/// The tool search, typed: looked up once typing stops a moment.
fn tool_search_changed(g: &mut Shell, cx: &mut Context<Shell>) {
    let q = g.text(&g.projects.tool_search.clone(), cx);
    if q.chars().count() < 2 {
        g.projects.tool_found = None;
        return cx.notify();
    }
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
        if !this.update(cx, |g, cx| g.text(&g.projects.tool_search.clone(), cx) == q).unwrap_or(false) {
            return;
        }
        let words = q.clone();
        let found = cx.background_executor().spawn(async move { nami::search_tools(&words) }).await;
        let _ = this.update(cx, |g, cx| {
            if g.text(&g.projects.tool_search.clone(), cx) == q {
                g.projects.tool_found = Some((q, found));
                cx.notify();
            }
        });
    })
    .detach();
}

/// Change a project's project.nami (`nami config json`), then look again.
fn change(g: &mut Shell, title: String, file: String, op: serde_json::Value, cx: &mut Context<Shell>) {
    g.start(&title, vec![(vec!["config".into(), "json".into(), op.to_string(), "--file".into(), file], None)], cx);
}

/// Look again, in the background: the projects, and their services.
pub fn look(cx: &mut Context<Shell>) {
    cx.spawn(async move |this, cx| {
        let (list, services) = cx
            .background_executor()
            .spawn(async move {
                let list = nami::projects();
                let services = list.as_ref().map(|l| l.iter().filter(|p| !p.services.is_empty()).map(|p| (p.dir.clone(), nami::services(&p.dir))).collect()).unwrap_or_default();
                (list, services)
            })
            .await;
        let _ = this.update(cx, |g, cx| {
            let attrs = list.as_ref().map(|l| l.iter().flat_map(|p| p.packages.clone()).collect()).unwrap_or_default();
            g.projects.list = Some(list);
            g.projects.services = services;
            g.learn_names(attrs, cx);
            cx.notify();
        });
    })
    .detach();
}

/// Start a task by itself; its output goes under its project.
fn start_task(g: &mut Shell, dir: String, task: String, cx: &mut Context<Shell>) {
    let (mut rx, group) = nami::run_task(&dir, &task);
    let key = (dir, task);
    g.projects.tasks.insert(key.clone(), Task { lines: Vec::new(), group, finished: None, stopped: false });
    cx.spawn(async move |this, cx| {
        use futures::StreamExt;
        while let Some(said) = rx.next().await {
            let gone = this
                .update(cx, |g, cx| {
                    if let Some(t) = g.projects.tasks.get_mut(&key) {
                        match said {
                            nami::Said::Line(l) => t.lines.push(l),
                            nami::Said::Finished(end) => t.finished = Some(end),
                        }
                    }
                    cx.notify();
                })
                .is_err();
            if gone {
                break;
            }
        }
    })
    .detach();
}

/// A task's last lines, under its project: running, done or failed.
fn task_log(i: usize, n: usize, task: &str, t: &Task, key: (String, String), cx: &mut Context<Shell>) -> gpui::Stateful<Div> {
    let (state, tone) = match &t.finished {
        None => ("running".to_string(), color::good()),
        Some(Ok(())) => ("done".to_string(), color::muted()),
        Some(Err(_)) if t.stopped => ("stopped".to_string(), color::muted()),
        Some(Err(_)) => ("failed".to_string(), color::bad()),
    };
    let tail: Vec<String> = t.lines.iter().rev().take(8).rev().cloned().collect();
    div()
        .id(ElementId::NamedInteger(format!("log-{i}").into(), n as u64))
        .flex()
        .flex_col()
        .gap(px(6.))
        .p(px(10.))
        .rounded(px(crate::look::shape::ROW))
        .bg(color::push())
        .border_1()
        .border_color(color::hairline())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(if t.finished.is_none() { crate::motion::dot(ElementId::NamedInteger(format!("task-dot-{i}").into(), n as u64), true, tone) } else { div().size(px(8.)).flex_none().rounded_full().bg(rgb(tone)).into_any_element() })
                .child(div().text_size(px(text::CALLOUT)).font_weight(FontWeight::SEMIBOLD).child(task.to_string()))
                .child(div().flex_1().text_size(px(text::META)).text_color(rgb(color::muted())).child(state))
                .when(t.finished.is_some(), |d| {
                    d.child(div().id(ElementId::NamedInteger(format!("log-close-{i}").into(), n as u64)).text_size(px(text::META)).text_color(rgb(color::muted())).cursor_pointer().press().hover(|s| s.text_color(rgb(color::text()))).child("Close").on_click(cx.listener(move |g, _, _, cx| {
                        g.projects.tasks.remove(&key);
                        cx.notify();
                    })))
                }),
        )
        .when(!tail.is_empty(), |d| d.child(div().text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::muted())).child(tail.join("\n"))))
}

/// The packages a starter's version pins: its language's (Rust: the
/// compiler and cargo, which go together).
fn pins(starter: &str) -> &'static [&'static str] {
    match starter {
        "node" => &["nodejs"],
        "python" => &["python3"],
        "rust" => &["rustc", "cargo"],
        "go" => &["go"],
        "c" => &["gcc"],
        _ => &[],
    }
}

/// A version's release line as it's pinned: Node and GCC by their major (22, 14),
/// the others by two numbers (3.12, 1.95).
fn line(attr: &str, version: &str) -> String {
    let mut parts = version.split(['.', '-']);
    let major = parts.next().unwrap_or_default();
    match (attr, parts.next()) {
        ("nodejs" | "gcc", _) | (_, None) => major.to_string(),
        (_, Some(minor)) => format!("{major}.{minor}"),
    }
}

/// `3.10` > `3.9`: numbers part by part.
fn newer(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |v: &str| v.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    key(a).cmp(&key(b))
}

/// Look up the release lines of a starter's language, in the background.
fn look_lines(g: &mut Shell, starter: &str, cx: &mut Context<Shell>) {
    let Some(attr) = pins(starter).first().map(|a| a.to_string()) else { return };
    if g.projects.lines.contains_key(&attr) {
        return;
    }
    cx.spawn(async move |this, cx| {
        let key = attr.clone();
        let found = cx
            .background_executor()
            .spawn(async move {
                nami::versions(&attr).map(|list| {
                    let mut lines: Vec<String> = list.iter().filter(|v| v.version.chars().next().is_some_and(|c| c.is_ascii_digit())).map(|v| line(&attr, &v.version)).collect();
                    lines.sort_by(|a, b| newer(b, a));
                    lines.dedup();
                    lines.truncate(6);
                    lines
                })
            })
            .await;
        let _ = this.update(cx, |g, cx| {
            g.projects.lines.insert(key, found);
            cx.notify();
        });
    })
    .detach();
}

/// A service as people know it.
fn service_title(name: &str) -> String {
    match name {
        "postgres" => "PostgreSQL".into(),
        "mysql" => "MySQL".into(),
        "redis" => "Redis".into(),
        "mailpit" => "Mailpit".into(),
        n => starter_title(n),
    }
}

/// A starter's language, by the package its picture is.
fn starter_icon(name: &str) -> &str {
    match name {
        "node" => "nodejs",
        "python" => "python3",
        "rust" => "rustc",
        "c" => "gcc",
        other => other,
    }
}

fn starter_title(name: &str) -> String {
    match name {
        "node" => "Node.js".into(),
        "c" => "C".into(),
        "empty" => "Empty".into(),
        n => {
            let mut c = n.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

fn label(text: &str) -> Div {
    crate::look::group_heading(text)
}

/// A project's language, in a few letters for its picture, by the tools it has.
pub fn language_of(packages: &[String]) -> String {
    let names: Vec<&str> = packages.iter().map(|p| tool_name(p).0).collect();
    let has = |n: &str| names.iter().any(|x| *x == n || x.starts_with(&format!("{n}_")));
    // (A project's own language before what it uses alongside: Rust's build scripts may want
    // Python, a Node project rarely Rust.)
    let short = if has("rustc") || has("cargo") {
        "rs"
    } else if has("go") {
        "go"
    } else if has("nodejs") || has("pnpm") || has("yarn") || has("bun") || has("deno") {
        "js"
    } else if has("python3") || has("python") || has("uv") {
        "py"
    } else if has("ruby") {
        "rb"
    } else if has("php") {
        "php"
    } else if has("gcc") || has("clang") {
        "c"
    } else if has("openjdk") || has("jdk") {
        "java"
    } else {
        ""
    };
    if !short.is_empty() {
        return short.to_string();
    }
    names.first().map(|n| n.chars().filter(|c| c.is_alphanumeric()).take(2).collect()).unwrap_or_else(|| "—".to_string())
}

/// A tool as a project lists it, `nodejs`, `nodejs: 22` or `nodejs@22`: its name, and the
/// version it's kept at.
pub fn tool_name(listed: &str) -> (&str, Option<&str>) {
    match listed.split_once([':', '@']) {
        Some((n, v)) => (n.trim(), Some(v.trim().trim_matches('"')).filter(|v| !v.is_empty())),
        None => (listed.trim(), None),
    }
}

/// How a service is, in a line: `Running on port 5432`, `Stopped`.
pub fn service_line(s: Option<&Service>) -> String {
    match s {
        Some(s) if s.running && s.ports.len() == 1 => format!("Running on port {}", s.ports[0]),
        Some(s) if s.running && !s.ports.is_empty() => format!("Running on ports {}", s.ports.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")),
        Some(s) if s.running => "Running".into(),
        Some(s) if !s.state.is_empty() && s.state != "stopped" => {
            let mut c = s.state.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
        _ => "Stopped".into(),
    }
}

/// A service's output, followed while it's open.
pub struct Logs {
    pub dir: String,
    pub service: String,
    pub lines: Vec<String>,
    pub group: Option<u32>,
}

/// Follow a service's output (`nami logs`), under its line; again, it's put away.
fn toggle_logs(g: &mut Shell, dir: String, service: String, cx: &mut Context<Shell>) {
    if let Some(l) = g.projects.logs.take() {
        if let Some(group) = l.group {
            nami::stop_task(group);
        }
        if l.dir == dir && l.service == service {
            return cx.notify();
        }
    }
    let (mut rx, group) = nami::follow_logs(&dir, &service);
    g.projects.logs = Some(Logs { dir: dir.clone(), service: service.clone(), lines: Vec::new(), group });
    cx.spawn(async move |this, cx| {
        use futures::StreamExt;
        while let Some(said) = rx.next().await {
            let gone = this
                .update(cx, |g, cx| {
                    let Some(l) = g.projects.logs.as_mut().filter(|l| l.dir == dir && l.service == service && l.group == group) else { return true };
                    match said {
                        nami::Said::Line(line) => {
                            l.lines.push(line);
                            let over = l.lines.len().saturating_sub(200);
                            l.lines.drain(..over);
                        }
                        nami::Said::Finished(Err(e)) if !e.is_empty() => l.lines.push(e),
                        nami::Said::Finished(_) => {}
                    }
                    cx.notify();
                    false
                })
                .unwrap_or(true);
            if gone {
                break;
            }
        }
    })
    .detach();
    cx.notify();
}

/// The picked project: its tools, services and tasks, and opening it.
fn detail(g: &Shell, i: usize, p: &Project, cx: &mut Context<Shell>) -> Div {
    use crate::ui::{grey, ink, main_button, round_button, sheet};
    let busy = g.run.as_ref().is_some_and(|r| r.finished.is_none());
    let home = std::env::var("HOME").unwrap_or_default();
    let tilde = |d: &str| d.strip_prefix(&home).map(|r| format!("~{r}")).unwrap_or(d.to_string());
    let shown = tilde(&p.dir);
    let file = if p.file.is_empty() { format!("{}/project.nami", p.dir) } else { p.file.clone() };

    // Show project.nami, and Open in Terminal (the main one): its tools there.
    let f = std::path::PathBuf::from(&file);
    let show = grey(ElementId::NamedInteger("show-project-nami".into(), i as u64), "Show project.nami").on_click(cx.listener(move |g, _, _, cx| {
        g.problem = crate::launch::open_text_file(&f, None).err().map(crate::Problem::under("Couldn't open project.nami."));
        cx.notify();
    }));
    let enter = format!("cd {shown} && nami shell");
    let dir = std::path::PathBuf::from(&p.dir);
    let open = if crate::launch::terminal().is_some() {
        main_button(ElementId::NamedInteger("open-terminal".into(), i as u64), "Open in Terminal", true).on_click(cx.listener(move |g, _, _, cx| {
            g.problem = crate::launch::open_terminal(&dir).err().map(crate::Problem::under("Couldn't open the terminal."));
            cx.notify();
        }))
    } else {
        main_button(ElementId::NamedInteger("copy-shell".into(), i as u64), "Copy Shell Command", true).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(enter.clone())))
    };
    // (And in the code editor there is, as before.)
    let editor = crate::launch::editor().map(|(label, _)| {
        let path = std::path::PathBuf::from(&p.dir);
        grey(ElementId::NamedInteger("open-editor".into(), i as u64), format!("Open in {label}")).on_click(cx.listener(move |g, _, _, cx| {
            let path = path.clone();
            g.problem = None;
            cx.spawn(async move |this, cx| {
                let end = cx.background_executor().spawn(async move { crate::launch::open_editor(&path) }).await;
                let _ = this.update(cx, |g, cx| {
                    g.problem = end.err().map(crate::Problem::under("Couldn't open the editor."));
                    cx.notify();
                });
            })
            .detach();
        }))
    });
    // The name keeps a width of its own (cut short with an ellipsis, never a letter a line);
    // where the buttons don't fit beside it, they go under it.
    let head = div().flex().flex_wrap().items_center().gap(px(10.))
        .child(div().flex_1().min_w(px(180.)).overflow_hidden()
            .child(div().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).truncate().child(p.name.clone()))
            .child(div().text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::muted())).truncate().child(tilde(&file))))
        .child(div().flex().flex_wrap().items_center().gap(px(10.)).child(show).children(editor).child(open));

    // Its tools, each with its version; a click on one takes it off.
    let adding = g.projects.adding.as_deref() == Some(p.dir.as_str());
    let tools = div().flex().flex_wrap().gap(px(6.)).children(p.packages.iter().enumerate().map(|(n, listed)| {
        let (name, kept) = tool_name(listed);
        let version = kept.map(String::from).or_else(|| g.catalog.get(name).map(|f| f.version.clone()).filter(|v| !v.is_empty()));
        let (file, value, project, name) = (p.file.clone(), listed.clone(), p.name.clone(), name.to_string());
        div()
            .id(ElementId::NamedInteger(format!("tool-{i}").into(), n as u64))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(11.))
            .h(px(30.))
            .rounded(px(9.))
            .bg(ink(0x0c))
            .font_family("Geist Mono")
            .text_size(px(text::CALLOUT))
            .child(name.clone())
            .children(version.map(|v| div().text_color(rgb(color::muted())).child(v)))
            .child(div().id(ElementId::NamedInteger(format!("tool-off-{i}").into(), n as u64)).text_color(rgb(color::faint())).cursor_pointer().hover(|s| s.text_color(rgb(color::text()))).child("×").on_click(cx.listener(move |g, _, _, cx| {
                change(g, format!("Removing {name} from {project}"), file.clone(), json!({ "remove": { "path": ["packages"], "value": value } }), cx);
            })))
    }))
    .when(!adding, |d| {
        let dir = p.dir.clone();
        d.child(grey(ElementId::NamedInteger("tool-add".into(), i as u64), "Add a Tool").on_click(cx.listener(move |g, _, window, cx| {
            g.projects.adding = Some(dir.clone());
            g.projects.tool_found = None;
            g.projects.tool_search.update(cx, |s, cx| s.set(cx, ""));
            window.focus(&gpui::Focusable::focus_handle(g.projects.tool_search.read(cx), cx), cx);
            cx.notify();
        })))
    });
    let add_box = adding.then(|| {
        let results: Vec<nami::Found> = g.projects.tool_found.as_ref().and_then(|(_, r)| r.as_ref().ok()).map(|r| r.iter().take(6).cloned().collect()).unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(div().flex().gap(px(8.)).items_center().child(div().flex_1().child(g.projects.tool_search.clone())).child(grey(ElementId::NamedInteger("tool-done".into(), i as u64), "Done").on_click(cx.listener(|g, _, _, cx| {
                g.projects.adding = None;
                cx.notify();
            }))))
            .children(results.into_iter().enumerate().map(|(n, f)| {
                let listed = p.packages.iter().any(|a| tool_name(a).0 == f.attr);
                let (file, attr, project, name) = (p.file.clone(), f.attr.clone(), p.name.clone(), f.name());
                let action = if listed {
                    Err("Added".to_string())
                } else {
                    Ok(grey(ElementId::NamedInteger(format!("tool-pick-{i}").into(), n as u64), "Add").on_click(cx.listener(move |g, _, _, cx| {
                        change(g, format!("Adding {name} to {project}"), file.clone(), json!({ "add": { "path": ["packages"], "value": attr } }), cx);
                    })))
                };
                crate::ui::app_row(ElementId::NamedInteger(format!("tool-row-{i}").into(), n as u64), &f, action)
            }))
    });
    let part = |title: &str| div().text_size(px(text::CALLOUT)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::muted())).child(title.to_string());
    let mut out = sheet().flex_1().min_w_0().flex().flex_col().gap(px(18.)).px(px(20.)).py(px(18.)).child(head).child(div().flex().flex_col().gap(px(8.)).child(part("Tools")).child(tools).children(add_box));

    // Its services: running or not, their output, Start or Stop.
    let services = g.services_of(&p.dir);
    if !p.services.is_empty() {
        let mut rows = div().flex().flex_col().rounded(px(11.)).border_1().border_color(ink(0x18)).overflow_hidden();
        for (n, name) in p.services.iter().enumerate() {
            let s = services.iter().find(|s| &s.name == name);
            let running = s.is_some_and(|s| s.running);
            let (dir, svc) = (p.dir.clone(), name.clone());
            let (ldir, lsvc) = (p.dir.clone(), name.clone());
            let open_logs = g.projects.logs.as_ref().is_some_and(|l| l.dir == p.dir && l.service == *name);
            let start_stop = grey(ElementId::NamedInteger(format!("svc-{i}").into(), n as u64), if running { "Stop" } else { "Start" }).when(busy, |b| b.opacity(0.45)).when(!busy, |b| {
                b.on_click(cx.listener(move |g, _, _, cx| {
                    let args: Vec<String> = if running { vec!["down".into(), svc.clone(), "--project".into(), dir.clone()] } else { vec!["up".into(), "-d".into(), svc.clone(), "--project".into(), dir.clone()] };
                    g.start(&format!("{} {}", if running { "Stopping" } else { "Starting" }, service_title(&svc)), vec![(args, None)], cx);
                }))
            });
            let logs = grey(ElementId::NamedInteger(format!("svc-logs-{i}").into(), n as u64), if open_logs { "Hide Logs" } else { "Logs" }).on_click(cx.listener(move |g, _, _, cx| toggle_logs(g, ldir.clone(), lsvc.clone(), cx)));
            // (A Mac's nami doesn't run them yet: only their names, and what to do instead.)
            let mac = crate::look::MAC;
            rows = rows.child(
                div().flex().items_center().gap(px(12.)).px(px(14.)).py(px(10.)).when(n > 0, |d| d.border_t_1().border_color(ink(0x10)))
                    .when(!mac, |d| d.child(crate::ui::dot(if running { color::good() } else { color::faint() })))
                    .child(div().flex_1().min_w_0().flex().items_baseline().gap(px(8.)).child(div().text_size(px(text::CALLOUT)).font_weight(FontWeight::SEMIBOLD).child(service_title(name))).when(!mac, |d| d.child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(service_line(s)))))
                    .when(!mac, |d| d.child(logs).child(start_stop)),
            );
            if let Some(l) = g.projects.logs.as_ref().filter(|_| open_logs) {
                let tail: Vec<String> = l.lines.iter().rev().take(14).rev().cloned().collect();
                let said = if tail.is_empty() { "Waiting for its output…".to_string() } else { tail.join("\n") };
                rows = rows.child(div().px(px(14.)).py(px(10.)).border_t_1().border_color(ink(0x10)).bg(ink(0x08)).font_family("Geist Mono").text_size(px(text::META)).line_height(px(18.)).text_color(rgb(color::muted())).child(said));
            }
        }
        let note = crate::look::MAC.then(|| crate::ui::quiet(MAC_SERVICES).text_size(px(text::META)));
        out = out.child(div().flex().flex_col().gap(px(8.)).child(part("Services")).child(rows).children(note));
    }
    // Services there are starters for, to add. (Not on a Mac, which doesn't run them yet.)
    let starters: Vec<String> = g.examples.as_ref().ok().filter(|_| !crate::look::MAC).map(|e| e.services.iter().map(|x| x.name.clone()).filter(|n| !p.services.contains(n)).collect()).unwrap_or_default();
    let showing = g.projects.adding_service.as_deref() == Some(p.dir.as_str());
    if !starters.is_empty() && !showing {
        let dir = p.dir.clone();
        let add = grey(ElementId::NamedInteger("svc-add".into(), i as u64), "Add a Service…").on_click(cx.listener(move |g, _, _, cx| {
            g.projects.adding_service = Some(dir.clone());
            cx.notify();
        }));
        out = out.child(div().flex().flex_col().gap(px(8.)).when(p.services.is_empty(), |d| d.child(part("Services"))).child(div().flex().child(add)));
    } else if !starters.is_empty() {
        let add = div().flex().flex_wrap().gap(px(6.)).children(starters.into_iter().enumerate().map(|(n, name)| {
            let (dir, project) = (p.dir.clone(), p.name.clone());
            grey(ElementId::NamedInteger(format!("svc-add-{i}").into(), n as u64), format!("Add {}", service_title(&name))).when(!busy, |b| b.on_click(cx.listener(move |g, _, _, cx| {
                g.start(&format!("Adding {} to {project}", service_title(&name)), vec![(vec!["new".into(), dir.clone(), "--from".into(), name.clone()], None)], cx);
            })))
        }));
        out = out.child(div().flex().flex_col().gap(px(8.)).when(p.services.is_empty(), |d| d.child(part("Services"))).child(add));
    }

    // Its tasks, each to run (or stop), and what they said.
    if !p.tasks.is_empty() {
        let cards = p.tasks.iter().enumerate().map(|(t, (task, cmd))| {
            let key = (p.dir.clone(), task.clone());
            let running = g.projects.tasks.get(&key).filter(|r| r.finished.is_none()).map(|r| r.group);
            let first = cmd.lines().next().unwrap_or_default().to_string();
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(10.))
                .min_w_0()
                .rounded(px(11.))
                .border_1()
                .border_color(ink(0x18))
                .child(div().flex_1().min_w_0().child(div().font_family("Geist Mono").text_size(px(text::CALLOUT)).font_weight(FontWeight::SEMIBOLD).truncate().child(task.clone())).child(div().font_family("Geist Mono").text_size(px(text::META)).text_color(rgb(color::muted())).truncate().child(first)))
                .child(round_button(ElementId::NamedInteger(format!("task-{i}").into(), t as u64), if running.is_some() { "stop" } else { "play" }, if running.is_some() { format!("Stop {task}") } else { format!("Run {task}") }).on_click(cx.listener(move |g, _, _, cx| {
                    match running {
                        Some(group) => {
                            if let Some(r) = g.projects.tasks.get_mut(&key) {
                                r.stopped = true;
                            }
                            if let Some(group) = group {
                                nami::stop_task(group);
                            }
                        }
                        None => start_task(g, key.0.clone(), key.1.clone(), cx),
                    }
                    cx.notify();
                })))
        }).collect::<Vec<_>>();
        let logs: Vec<_> = p.tasks.keys().enumerate().filter_map(|(n, task)| {
            let key = (p.dir.clone(), task.clone());
            g.projects.tasks.get(&key).map(|t| task_log(i, n, task, t, key, cx))
        }).collect();
        out = out.child(div().flex().flex_col().gap(px(8.)).child(part("Tasks")).child({
            // (Beside the list of projects where there's room: as many as fit in what's left.)
            let room = if crate::fit::narrower(640.) { crate::fit::page() } else { crate::fit::page() - 306. };
            div().grid().grid_cols(crate::fit::columns_in(room, 170., 8., 3) as u16).gap(px(8.)).children(cards)
        }).children(logs));
    }
    let mono = |t: &str| div().font_family("Geist Mono").child(t.to_string());
    out.child(
        div().flex().flex_wrap().items_center().gap(px(4.)).text_size(px(text::META)).text_color(rgb(color::muted()))
            .child("In a terminal,").child(mono("nami shell")).child("opens the project's tools,").child(mono("nami up")).child("starts its services and").child(mono("nami run test")).child("runs a task."),
    )
}

/// The version of the starter's language: the newest (kept up to date), or
/// a release line it stays on.
fn version_row(g: &Shell, starter: &str, cx: &mut Context<Shell>) -> Option<Div> {
    let attr = pins(starter).first()?;
    let row = div().flex().flex_wrap().gap(px(6.));
    let row = match g.projects.lines.get(*attr) {
        None => row.child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child("Loading versions…")),
        Some(Err(why)) => row.child(super::trouble("versions-said", "Couldn't load versions.", why.clone(), cx)),
        Some(Ok(lines)) => {
            let newest = g.projects.version.is_none();
            row.child(pill("version-newest".into(), "Always Latest", newest).h(px(28.)).text_size(px(text::META)).on_click(cx.listener(|g, _, _, cx| {
                g.projects.version = None;
                cx.notify();
            })))
            .children(lines.iter().enumerate().map(|(n, l)| {
                let on = g.projects.version.as_deref() == Some(l.as_str());
                let pick = l.clone();
                pill(ElementId::NamedInteger("version".into(), n as u64), format!("{} {l}", starter_title(starter)), on).h(px(28.)).text_size(px(text::META)).on_click(cx.listener(move |g, _, _, cx| {
                    g.projects.version = Some(pick.clone());
                    cx.notify();
                }))
            }))
        }
    };
    Some(div().flex().flex_col().gap(px(8.)).child(label("Version")).child(row))
}

pub fn render(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    use crate::ui::{grey, ink, letters, list_row, page_title, quiet};
    let line = div().child("Each project's tools, services and tasks, from its project.nami. They're there only inside the project, so projects never clash.");
    let new = grey("new-project", "New Project…").when(g.projects.new.is_none(), |b| {
        b.on_click(cx.listener(|g, _, _, cx| {
            g.projects.new = Some("node".into());
            g.projects.version = None;
            look_lines(g, "node", cx);
            cx.notify();
        }))
    });
    let mut page = div().flex().flex_col().gap(px(16.)).pt(px(20.)).child(div().flex().items_start().gap(px(12.)).child(page_title("Projects", Some(line.into_any_element()))).child(new));

    // A new one: its starter and its folder.
    if let Some(picked) = g.projects.new.clone() {
        let starters: Vec<String> = g.examples.as_ref().map(|e| e.projects.iter().map(|p| p.name.clone()).collect()).unwrap_or_default();
        page = page.child(
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .p(px(16.))
                .rounded(px(crate::look::shape::MODULE))
                .bg(color::module())
                .border_1()
                .border_color(color::module_line())
                .shadow(crate::look::module_edge())
                .child(label("Start from"))
                .child(div().flex().flex_wrap().gap(px(8.)).children(starters.iter().map(|s| {
                    let on = *s == picked;
                    let name = s.clone();
                    div()
                        .id(ElementId::Name(format!("starter-{s}").into()))
                        .w(px(84.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(6.))
                        .py(px(10.))
                        .rounded(px(crate::look::shape::CONTROL))
                        .border_1()
                        .border_color(if on { rgb(color::key_strong()) } else { color::hairline() })
                        .bg(rgb(color::panel()))
                        .cursor_pointer()
                        .press_row()
                        .child(icon(catalog_icon(starter_icon(s)).as_deref(), starter_icon(s), 36.))
                        .child(div().text_size(px(text::META)).child(starter_title(s)))
                        .on_click(cx.listener(move |g, _, _, cx| {
                            g.projects.new = Some(name.clone());
                            g.projects.version = None;
                            look_lines(g, &name, cx);
                            cx.notify();
                        }))
                })))
                .children(version_row(g, &picked, cx))
                .child(label("Folder"))
                .child(g.projects.folder.clone())
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(button("new-cancel", "Cancel", false, true).on_click(cx.listener(|g, _, _, cx| {
                            g.projects.new = None;
                            cx.notify();
                        })))
                        .child(button("new-make", "Create", true, true).on_click(cx.listener(move |g, _, _, cx| {
                            let folder = g.text(&g.projects.folder.clone(), cx);
                            // ~ is your home (nami::home is ~/nami, where computers go).
                            let home = std::env::var("HOME").unwrap_or_default();
                            let folder = if folder.starts_with('~') { folder.replacen('~', &home, 1) } else { folder };
                            let file = std::path::Path::new(&folder).join("project.nami").display().to_string();
                            let mut cmds = vec![(vec!["new".into(), folder, "--from".into(), picked.clone()], None)];
                            // A version picked: the language's entry pinned to it.
                            if let Some(v) = g.projects.version.clone() {
                                for attr in pins(&picked) {
                                    // (Added before the plain one goes, so the list stays where it is.)
                                    for op in [json!({ "add": { "path": ["packages"], "value": { *attr: v } } }), json!({ "remove": { "path": ["packages"], "value": attr } })] {
                                        cmds.push((vec!["config".into(), "json".into(), op.to_string(), "--file".into(), file.clone()], None));
                                    }
                                }
                            }
                            g.start("Creating the project", cmds, cx);
                            g.projects.new = None;
                        }))),
                ),
        );
    }

    let list = match g.projects.list.clone() {
        None => return page.child(super::looking("projects-looking")),
        Some(Err(why)) => {
            return page.child(super::failed("projects-again", "Couldn't load your projects.", why.clone(), |g, cx| {
                g.projects.list = None;
                look(cx);
            }, cx))
        }
        Some(Ok(list)) => list,
    };
    if list.is_empty() {
        return page.child(quiet("No projects yet. A project is a folder with a project.nami file. Create one with New Project, or run nami shell in a folder."));
    }
    // The projects at the left, the picked one at the right.
    let picked = g.projects.picked.clone().filter(|d| list.iter().any(|p| p.dir == *d)).unwrap_or_else(|| list[0].dir.clone());
    let home = std::env::var("HOME").unwrap_or_default();
    // (The projects at the left, the picked one at the right; in a narrow window the list above it.)
    let narrow = crate::fit::narrower(640.);
    let mut side = div().when(!narrow, |d| d.w(px(290.)).flex_none()).when(narrow, |d| d.w_full()).flex().flex_col().gap(px(4.));
    for (i, p) in list.iter().enumerate() {
        let on = p.dir == picked;
        let shown = p.dir.strip_prefix(&home).map(|r| format!("~{r}")).unwrap_or(p.dir.clone());
        let running = g.services_of(&p.dir).iter().any(|s| s.running);
        let dir = p.dir.clone();
        side = side.child(
            list_row(ElementId::NamedInteger("project".into(), i as u64), on)
                .child(letters(&language_of(&p.packages), 30.).when(on, |d| d.bg(rgb(color::panel()))))
                .child(div().flex_1().min_w_0().child(div().text_size(px(text::CALLOUT)).font_weight(FontWeight::SEMIBOLD).truncate().child(p.name.clone())).child(div().text_size(px(text::META)).font_family("Geist Mono").text_color(rgb(color::muted())).truncate().child(shown)))
                .when(running, |d| d.child(crate::ui::dot(color::good())))
                .on_click(cx.listener(move |g, _, _, cx| {
                    g.projects.picked = Some(dir.clone());
                    cx.notify();
                })),
        );
    }
    side = side.child(div().mt(px(8.)).p(px(12.)).rounded(px(11.)).border_1().border_color(ink(0x18)).text_size(px(text::META)).line_height(px(18.)).text_color(rgb(color::muted())).child("Clone a project with a project.nami, and it shows up here, ready to run."));
    let (i, p) = list.iter().enumerate().find(|(_, p)| p.dir == picked).expect("the picked one is listed");
    page.child(div().flex().when(narrow, |d| d.flex_col()).when(!narrow, |d| d.items_start()).gap(px(16.)).child(side).child(detail(g, i, p, cx)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_projects_language_is_said_by_its_tools() {
        let s = |l: &[&str]| language_of(&l.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        assert_eq!(s(&["nodejs: 22", "pnpm"]), "js");
        assert_eq!(s(&["python3@3.12", "ruff"]), "py");
        assert_eq!(s(&["rustc", "cargo", "gcc", "python3"]), "rs");
        assert_eq!(s(&["terraform"]), "te");
        assert_eq!(s(&[]), "—");
    }

    #[test]
    fn a_tool_and_its_version_are_read() {
        assert_eq!(tool_name("nodejs"), ("nodejs", None));
        assert_eq!(tool_name("nodejs: 22"), ("nodejs", Some("22")));
        assert_eq!(tool_name("python3@3.12"), ("python3", Some("3.12")));
    }

    #[test]
    fn a_service_is_said_with_its_port() {
        let s = |running: bool, state: &str, ports: Vec<u16>| Service { name: "postgres".into(), state: state.into(), running, ports };
        assert_eq!(service_line(Some(&s(true, "running", vec![5432]))), "Running on port 5432");
        assert_eq!(service_line(Some(&s(true, "running", vec![80, 443]))), "Running on ports 80, 443");
        assert_eq!(service_line(Some(&s(false, "stopped", vec![]))), "Stopped");
        assert_eq!(service_line(Some(&s(false, "starting", vec![]))), "Starting");
        assert_eq!(service_line(None), "Stopped");
    }
}

/// Closing the window stops the tasks it started.
impl Drop for Projects {
    fn drop(&mut self) {
        if let Some(group) = self.logs.as_ref().and_then(|l| l.group) {
            nami::stop_task(group);
        }
        for t in self.tasks.values().filter(|t| t.finished.is_none()) {
            if let Some(group) = t.group {
                nami::stop_task(group);
            }
        }
    }
}
