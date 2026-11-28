//! The window's pieces: the shared look (look.rs), and nami-gui's own:
//! add-ons, app rows, icons.

use gpui::prelude::*;
use crate::look::Press;
use gpui::{Div, ElementId, FontWeight, InteractiveElement, ParentElement, Stateful, Styled, div, rgb};
use crate::look::px;
use crate::look::text;

pub use crate::look::*;

/// Settings' buttons, as the desktop's panels have them (`look::push_button`): in place of the
/// bordered one the installer and the welcome keep (`look::button`).
pub fn button(id: &'static str, label: impl Into<gpui::SharedString>, primary: bool, enabled: bool) -> Stateful<Div> {
    push_button(id, label, primary, enabled)
}

/// A box that's ticked or not: ticked, the key's strong slate with a white tick, as a Mac's
/// checkbox is in its accent.
pub fn tick(id: ElementId, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(18.))
        .flex_none()
        .rounded(px(shape::SMALL))
        .border_1()
        .border_color(if on { rgb(color::key_strong()) } else { color::switch_off() })
        .when(on, |d| d.bg(rgb(color::key_strong())))
        .when(!on, |d| d.bg(rgb(color::panel())))
        .text_color(rgb(color::on_key_strong()))
        .text_size(px(text::META))
        .font_weight(FontWeight::BOLD)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .press()
        .child(if on { "✓" } else { "" })
}

/// One of a few to pick from, each a module of its own: a title, a line about it, and whether
/// it's picked (its round in the key's strong slate, and its edge).
pub fn choice(id: impl Into<ElementId>, title: impl Into<gpui::SharedString>, about: impl Into<gpui::SharedString>, picked: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .gap(px(12.))
        .items_start()
        .p(px(shape::MODULE_INSET))
        .rounded(px(shape::MODULE))
        .border_1()
        .border_color(if picked { rgb(color::key_strong()) } else { color::module_line() })
        .bg(color::module())
        .shadow(module_edge())
        .cursor_pointer()
        .when(!picked, |d| d.hover(|s| s.border_color(color::switch_off())))
        .press_row()
        .child(
            div()
                .mt(px(2.))
                .size(px(16.))
                .flex_none()
                .rounded_full()
                .border_1()
                .border_color(if picked { rgb(color::key_strong()) } else { color::switch_off() })
                .when(picked, |d| d.bg(rgb(color::key_strong())))
                .flex()
                .items_center()
                .justify_center()
                .when(picked, |d| d.child(div().size(px(6.)).rounded_full().bg(rgb(color::on_key_strong())))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(title.into()))
                .child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(about.into())),
        )
}

/// A small button in a group's line ("Connect", "Forget", "Move up"): a grey push button, a
/// step deeper while it's `on` (what's picked, or what's waiting on it). Never the slate: that's
/// for a screen's one main button (`main_button`).
pub fn pill(id: ElementId, label: impl Into<gpui::SharedString>, on: bool) -> Stateful<Div> {
    let b = push_button(id, label, false, true);
    if on { b.bg(color::round_off()).font_weight(FontWeight::SEMIBOLD) } else { b }
}

/// A switch, to look at (what it's in is what's pressed), as the desktop's panels have it: on,
/// the key's strong slate comes in under its white knob as the knob slides across (`SNAP`);
/// off, the text's colour, see-through.
pub fn switch_look(id: impl Into<gpui::SharedString>, on: bool) -> Div {
    use gpui::AnimationExt;
    let id = id.into();
    let t = if on { 1f32 } else { 0f32 };
    div()
        .relative()
        .w(px(34.))
        .h(px(20.))
        .flex_none()
        .rounded_full()
        .bg(color::switch_off())
        .child(div().absolute().top_0().left_0().size_full().rounded_full().bg(rgb(color::key_strong())).with_spring(ElementId::NamedInteger(id.clone(), 1), crate::motion::spring(crate::motion::SNAP).to(t), |el, t| el.opacity(t.clamp(0., 1.))))
        .child(div().absolute().top(px(2.)).size(px(16.)).rounded_full().bg(rgb(color::level_knob())).shadow(knob_shadow()).with_spring(ElementId::NamedInteger(id, 2), crate::motion::spring(crate::motion::SNAP).to(t), |el, t| el.left(px(2. + 14. * t))))
}

thread_local! {
    /// Where each pop-up was last drawn, by its id: what its menu opens under.
    static POPUPS: std::cell::RefCell<std::collections::HashMap<String, gpui::Bounds<gpui::Pixels>>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// A pop-up button, for one of more than a few (or of long ones) to pick from: what's picked,
/// and the up and down chevrons; a press opens the menu of them under it (`popup_at`).
pub fn popup(id: impl Into<gpui::SharedString>, label: impl Into<gpui::SharedString>) -> Stateful<Div> {
    let id = id.into();
    let key = id.to_string();
    div()
        .id(ElementId::Name(id))
        .relative()
        .flex_none()
        .h(px(shape::PUSH))
        .pl(px(10.))
        .pr(px(6.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(px(shape::ROW))
        .bg(color::push())
        .text_size(px(text::CALLOUT))
        .cursor_pointer()
        .hover(|s| s.bg(color::round_off()))
        .press_row()
        .child(label.into())
        .child(crate::icons::glyph("chevrons-up-down", 12., color::muted()))
        .child(gpui::canvas(move |bounds, _, _| POPUPS.with(|p| p.borrow_mut().insert(key.clone(), bounds)), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
}

/// Where the pop-up by `id` is, for its menu; where it was pressed, if it's not been drawn.
pub fn popup_at(id: &str, pressed: gpui::Point<gpui::Pixels>) -> gpui::Bounds<gpui::Pixels> {
    POPUPS.with(|p| p.borrow().get(id).copied()).unwrap_or_else(|| gpui::Bounds::new(pressed, gpui::size(gpui::px(1.), gpui::px(1.))))
}

/// A line in a group that goes somewhere (a page, what's behind it): the slate wash under the
/// pointer, as the desktop's lists have it, and a chevron at its right after what's said there.
pub fn link_row(id: impl Into<ElementId>, last: bool) -> Stateful<Div> {
    group_row(last).id(id).cursor_pointer().hover(|s| s.bg(color::hover_pill())).press_row()
}

/// What's at a link's right: how it is now (if anything), quiet, and the chevron.
pub fn chevron(said: impl Into<gpui::SharedString>) -> Div {
    let said = said.into();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .text_size(px(text::CALLOUT))
        .text_color(rgb(color::muted()))
        .when(!said.is_empty(), |d| d.child(said))
        .child(crate::icons::glyph("chevron", 13., color::muted()))
}

/// What's said at the top of a page, under its title in the toolbar: what the page is for.
pub fn intro(said: impl Into<gpui::SharedString>) -> Div {
    div().px(px(shape::MODULE_INSET)).text_size(px(text::CALLOUT)).line_height(gpui::relative(text::leading(text::CALLOUT))).text_color(rgb(color::grouped_muted())).child(said.into())
}

/// nami's add-on examples the window offers, set up on a computer: each
/// one's folder, what it's called, and what it's for, in a line.
/// The kernels to pick from, by their package folders (none: the usual one).
pub const KERNELS: [(&str, &str, &str); 4] = [
    ("", "Linux", "The standard long-term kernel, version 6.18"),
    ("kernel-zen", "Zen", "More responsive under load, for desktops and games. Version 7.2"),
    ("kernel-xanmod", "XanMod", "Tuned for desktops and games. Long-term version 6.18"),
    ("kernel-xanmod-latest", "XanMod Latest", "The newest XanMod release, version 7.2"),
];


/// What a computer is for: an example package folder each (`gaming`),
/// listing `apps`; `more` are other good ones, offered unticked. An app
/// taken out goes in `without:`, one added in `packages:`.
pub struct Need {
    pub name: &'static str,
    pub title: &'static str,
    pub line: &'static str,
    pub apps: &'static [(&'static str, &'static str)],
    pub more: &'static [(&'static str, &'static str)],
}

pub const NEEDS: [Need; 8] = [
    Need { name: "gaming", title: "Games", line: "Steam, other game stores and GameMode",
        apps: &[("steam", "Steam"), ("heroic", "Heroic: Epic, GOG, Amazon"), ("protonup-qt", "ProtonUp-Qt"), ("gamemode", "GameMode"), ("mangohud", "MangoHud")],
        more: &[("lutris", "Lutris"), ("bottles", "Bottles"), ("prismlauncher", "Prism: Minecraft"), ("retroarch", "RetroArch"), ("discord", "Discord")] },
    Need { name: "coding", title: "Coding", line: "A code editor, containers, Git and an SSH agent",
        apps: &[("vscode", "Visual Studio Code"), ("docker", "Docker"), ("git", "git"), ("gh", "GitHub CLI"), ("meld", "Meld")],
        more: &[("zed-editor", "Zed"), ("podman", "Podman"), ("gnome-builder", "Builder"), ("dbeaver-bin", "DBeaver"), ("bruno", "Bruno")] },
    Need { name: "office", title: "Office and Email", line: "Documents, spreadsheets, slides, PDFs and mail",
        apps: &[("libreoffice", "LibreOffice"), ("thunderbird", "Thunderbird"), ("papers", "Papers")],
        more: &[("onlyoffice-desktopeditors", "ONLYOFFICE"), ("obsidian", "Obsidian"), ("joplin-desktop", "Joplin"), ("xournalpp", "Xournal++")] },
    Need { name: "photos", title: "Photos and Drawing", line: "Edit photos, paint and create vector graphics",
        apps: &[("gimp", "GIMP"), ("krita", "Krita"), ("inkscape", "Inkscape"), ("darktable", "darktable")],
        more: &[("digikam", "digiKam"), ("rawtherapee", "RawTherapee"), ("scribus", "Scribus"), ("pinta", "Pinta")] },
    Need { name: "video", title: "Video and Audio", line: "Edit video, record, stream and make music",
        apps: &[("kdePackages.kdenlive", "Kdenlive"), ("obs-studio", "OBS Studio"), ("audacity", "Audacity"), ("handbrake", "HandBrake")],
        more: &[("shotcut", "Shotcut"), ("ardour", "Ardour"), ("lmms", "LMMS"), ("musescore", "MuseScore"), ("davinci-resolve", "DaVinci Resolve")] },
    Need { name: "making", title: "3D and Making", line: "3D modelling and printing, CAD and circuit design",
        apps: &[("blender", "Blender"), ("freecad", "FreeCAD"), ("prusa-slicer", "PrusaSlicer"), ("openscad", "OpenSCAD")],
        more: &[("orca-slicer", "OrcaSlicer"), ("kicad", "KiCad"), ("librecad", "LibreCAD")] },
    Need { name: "talking", title: "Communication", line: "Messages and calls",
        apps: &[("signal-desktop", "Signal"), ("element-desktop", "Element"), ("telegram-desktop", "Telegram")],
        more: &[("discord", "Discord"), ("slack", "Slack"), ("zoom-us", "Zoom"), ("teams-for-linux", "Teams")] },
    Need { name: "learning", title: "Learning", line: "Flash cards, maths, astronomy and games for children",
        apps: &[("anki", "Anki"), ("geogebra6", "GeoGebra"), ("stellarium", "Stellarium"), ("gcompris", "GCompris")],
        more: &[("tuxpaint", "Tux Paint")] },
];

/// An app's catalog picture, where nami keeps them (or none yet).
pub fn catalog_icon(attr: &str) -> Option<std::path::PathBuf> {
    let root = std::env::var_os("NAMI_ROOT").map(std::path::PathBuf::from).unwrap_or_else(|| "/".into());
    // (nami's folder: /nami, or where it was, on a computer whose `/` can't hold it.)
    ["nami", "nix/var/nami"].iter().flat_map(|d| ["png", "svg"].map(|e| root.join(format!("{d}/icons/{attr}.{e}")))).find(|p| p.is_file())
}

/// A row of What it's for: bordered, filled when `on`.
pub fn pick_frame(id: ElementId, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(12.))
        .rounded(px(shape::MODULE))
        .border_1()
        .border_color(if on { rgb(color::key_strong()) } else { color::module_line() })
        .bg(color::module())
        .shadow(module_edge())
        .cursor_pointer()
        .when(!on, |d| d.hover(|s| s.border_color(color::switch_off())))
        .press_row()
}

/// A row's top line: its tick, title and what it's for.
pub fn pick_head(tick_id: ElementId, on: bool, title: &str, line: &str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(tick(tick_id, on))
        .child(div().flex().flex_col().flex_1().min_w_0().child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(title.to_string())).child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(line.to_string())))
}

/// A need's row: its tick, what it's for, and its apps' pictures; once
/// ticked, its apps to take out (`chips`) and others to add (`more`).
pub fn need_row(id: ElementId, need: &Need, on: bool, chips: Vec<Stateful<Div>>, more: Vec<Stateful<Div>>) -> Stateful<Div> {
    // The apps' own pictures (tools like git have none).
    let faces = div().flex().gap(px(4.)).flex_none().children(need.apps.iter().chain(need.more).filter_map(|(a, _)| catalog_icon(a)).take(4).map(|p| icon(Some(&p), "", 24.)));
    let head = pick_head(ElementId::Name(format!("{}-tick", need.name).into()), on, need.title, need.line).child(faces);
    pick_frame(id, on)
        .child(head)
        .when(on, |d| {
            d.child(div().flex().flex_wrap().gap(px(6.)).pl(px(30.)).children(chips))
                .when(!more.is_empty(), |d| d.child(div().flex().flex_wrap().items_center().gap(px(6.)).pl(px(30.)).child(div().text_size(px(text::META)).text_color(rgb(color::muted())).mr(px(2.)).child("More:")).children(more)))
        })
}

/// An app in a need: its picture and name, on (ticked) or off.
pub fn app_chip(id: ElementId, attr: &str, label: &str, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.))
        .pl(px(4.))
        .pr(px(10.))
        .h(px(28.))
        .rounded_full()
        .border_1()
        .border_color(if on { rgb(color::key_strong()) } else { color::hairline() })
        .bg(color::module())
        .text_size(px(text::META))
        .text_color(rgb(if on { color::text() } else { color::muted() }))
        .cursor_pointer()
        .press()
        .child(icon(catalog_icon(attr).as_deref(), label, 18.))
        .child(label.to_string())
        .child(div().text_color(rgb(if on { color::text() } else { color::muted() })).child(if on { "✓" } else { "+" }))
}

pub const ADDONS: [(&str, &str, &str); 8] = [
    ("tailscale", "Tailscale", "A private network for your devices"),
    ("docker", "Docker", "Run containers with docker and docker compose"),
    ("podman", "Podman", "Run containers without a background service"),
    ("vscode", "Visual Studio Code", "Microsoft's code editor"),
    ("nvidia", "NVIDIA driver", "For NVIDIA graphics cards, with CUDA"),
    ("printing-extra", "Extra printer drivers", "For older printers. Newer printers don't need them."),
    ("input-cjk", "Chinese, Japanese, Korean", "Input methods for typing these languages"),
    ("speech", "Screen reader voices", "Lets the screen reader speak. Uses 0.7 GB."),
];

/// The NVIDIA drivers, and which cards each is for.
pub const NVIDIA: [(&str, &str, &str); 2] = [
    ("nvidia", "NVIDIA driver", "GTX 16xx, RTX 20xx and newer, with CUDA"),
    ("nvidia-legacy", "NVIDIA driver for older cards", "GTX 750 to 1080 Ti, Quadro M and P, Titan V"),
];

/// The add-ons to offer, of the ones nami has (`has`): (name, title, line).
/// On this computer (`here`), the NVIDIA driver is the one for its card
/// (`gpu`); for a USB drive, both, with this computer's card said at its own.
pub fn addon_list(has: impl Fn(&str) -> bool, gpu: Option<&crate::nami::Hardware>, here: bool) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (name, title, line) in ADDONS {
        if name != "nvidia" {
            if has(name) {
                out.push((name.to_string(), title.to_string(), line.to_string()));
            }
            continue;
        }
        match (here, gpu) {
            (true, Some(g)) if has(&g.package) => out.push((g.package.clone(), "NVIDIA driver".into(), format!("For this computer's {}", g.name()))),
            (true, _) if has("nvidia") => out.push(("nvidia".into(), "NVIDIA driver".into(), "For NVIDIA graphics cards. This computer doesn't have one.".into())),
            (true, _) => {}
            (false, _) => {
                for (n, t, l) in NVIDIA.iter().filter(|(n, _, _)| has(n)) {
                    let line = match gpu {
                        Some(g) if g.package == *n => format!("{l}. This computer has a {}.", g.name()),
                        _ => l.to_string(),
                    };
                    out.push((n.to_string(), t.to_string(), line));
                }
            }
        }
    }
    out
}

/// An add-on to tick: what it is, and what it's for.
pub fn addon(id: ElementId, title: &str, line: &str, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_start()
        .gap(px(10.))
        .p(px(12.))
        .rounded(px(shape::MODULE))
        .border_1()
        .border_color(if on { rgb(color::key_strong()) } else { color::module_line() })
        .bg(color::module())
        .shadow(module_edge())
        .cursor_pointer()
        .when(!on, |d| d.hover(|s| s.border_color(color::switch_off())))
        .press_row()
        .child(tick(ElementId::Name(format!("{title}-tick").into()), on))
        .child(div().flex().flex_col().min_w_0().child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(title.to_string())).child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(line.to_string())))
}

/// Add-ons, two to a row (one in a narrow window).
pub fn addons(cards: Vec<Stateful<Div>>) -> Div {
    crate::fit::grid(260., 8., 2).children(cards)
}

/// The icon of an app, every one alike: a rounded square with a hairline,
/// its picture from the catalog inside (or its first letter).
pub fn icon(path: Option<&std::path::Path>, name: &str, size: f32) -> gpui::AnyElement {
    icon_named(path, name, None, size)
}

/// A found app's icon (search, Discover, its page): one of Homebrew's casks or Flathub's without
/// a picture has its app's name's first letter (Google Chrome's G).
pub fn app_icon(f: &crate::nami::Found, size: f32) -> gpui::AnyElement {
    let title = if f.cask() || f.flathub() { f.title.as_deref() } else { None };
    icon_named(f.icon.as_deref(), &f.attr, title, size)
}

/// Where an app is from when it isn't the catalog: `cask:google-chrome`, `flathub:org.gimp.GIMP`.
fn from_elsewhere(name: &str) -> bool {
    name.starts_with("cask:") || name.starts_with("flathub:")
}

/// The letter an app without a picture is shown with: its name's first, never its source's
/// (`cask:google-chrome` is G; `flathub:org.gimp.GIMP`, the id's last part, G).
pub fn letter(name: &str) -> String {
    let name = match name.split_once(':') {
        Some(("flathub", id)) => id.rsplit('.').next().unwrap_or(id),
        Some(("cask", token)) => token,
        _ => name,
    };
    name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_default()
}

#[cfg(test)]
#[test]
fn an_apps_letter_is_its_names() {
    assert_eq!(letter("cask:google-chrome"), "G");
    assert_eq!(letter("Google Chrome"), "G");
    assert_eq!(letter("cask:1password"), "1");
    assert_eq!(letter("flathub:org.gimp.GIMP"), "G");
    assert_eq!(letter("flathub:com.visualstudio.code"), "C");
    assert_eq!(letter("gimp"), "G");
    assert_eq!(letter("_private-tool"), "P");
    assert_eq!(letter("iTerm2"), "I");
    assert_eq!(letter(""), "");
    // (A title with a colon in it is a title.)
    assert_eq!(letter("Mail: Inbox"), "M");
    // A cask's name isn't a language's: no developer's mark for RubyMine or Python's apps.
    assert!(from_elsewhere("cask:rubymine") && from_elsewhere("flathub:org.python.Idle") && !from_elsewhere("ruby"));
}

fn icon_named(path: Option<&std::path::Path>, name: &str, title: Option<&str>, size: f32) -> gpui::AnyElement {
    let tile = div()
        .size(px(size))
        .flex_none()
        .rounded(px(size * 0.24))
        .border_1()
        .border_color(color::hairline())
        .bg(rgb(color::tile()))
        .flex()
        .items_center()
        .justify_center();
    // Small ones (in a chip) leave less room around the picture.
    let inner = size * if size < 28. { 0.78 } else { 0.66 };
    match path.filter(|p| p.is_file()) {
        // A symbolic one (a theme's, one colour): drawn in the text's, as GTK does, in either look.
        Some(p) if p.file_name().is_some_and(|n| n.to_string_lossy().ends_with("-symbolic.svg")) => tile
            .child(gpui::svg().external_path(p.display().to_string()).size(px(size * 0.5)).text_color(rgb(color::text())))
            .into_any_element(),
        Some(p) => tile.child(gpui::img(crate::pictures::sharp(p, inner)).size(px(inner))).into_any_element(),
        // A developer tool: its language's mark, in the text's colour.
        None if !from_elsewhere(name) && crate::icons::dev_mark(name).is_some() => tile
            .child(gpui::svg().path(format!("dev/{}.svg", crate::icons::dev_mark(name).unwrap_or_default())).size(px(size * 0.5)).text_color(rgb(color::text())))
            .into_any_element(),
        None => tile
            .text_size(px(size * 0.42))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(color::muted()))
            .child(letter(title.unwrap_or(name)))
            .into_any_element(),
    }
}

/// A search result to open: its icon, the app's name and line, and its
/// add button (`action`; Err: why there's none).
pub fn app_row(id: ElementId, f: &crate::nami::Found, action: Result<Stateful<Div>, String>) -> Stateful<Div> {
    let action = match action {
        Ok(a) => a.into_any_element(),
        Err(why) => div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(why).into_any_element(),
    };
    app_row_with(id, f, None, action)
}

/// The same, with its own line under the name (how installing it goes),
/// and any button.
/// The small label on an app from Flathub.
pub fn flathub_label() -> Div {
    div().flex_none().text_size(px(text::CAPTION)).px(px(6.)).rounded(px(4.)).border_1().border_color(color::hairline()).text_color(rgb(color::muted())).child("Flathub")
}

pub fn app_row_with(id: ElementId, f: &crate::nami::Found, line: Option<gpui::AnyElement>, action: gpui::AnyElement) -> Stateful<Div> {
    let line = line.unwrap_or_else(|| div().truncate().child(f.line()).into_any_element());
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(8.))
        .py(px(8.))
        .rounded(px(crate::look::shape::ROW))
        .cursor_pointer()
        .hover(|s| s.bg(color::hover_pill()))
        .press_row()
        .child(app_icon(f, 36.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div().flex().items_center().gap(px(8.)).min_w_0()
                        .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).min_w_0().child(crate::tip::cut(f.name())))
                        .when(!f.flathub() && f.title.as_ref().is_some_and(|t| !t.eq_ignore_ascii_case(&f.attr)), |d| d.child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(f.attr.clone())))
                        .when(f.flathub(), |d| d.child(flathub_label()))
                        .when(!f.app, |d| d.child(div().text_size(px(text::CAPTION)).px(px(6.)).rounded(px(4.)).bg(color::push()).text_color(rgb(color::muted())).child("command"))),
                )
                .child(div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).min_w_0().child(line)),
        )
        .child(action)
}

/// A level's track, as Control Center's sliders have it: this tall and long, round; its glyph
/// at the left in this much room, always on the fill; its knob as tall as the track.
pub const TRACK: f32 = 22.;
pub const TRACK_LONG: f32 = 220.;
pub const GLYPH_ROOM: f32 = 26.;

/// How far along the track a level's knob's middle is at `percent`: from just past the glyph
/// to the right end.
pub fn knob_at(percent: f32) -> f32 {
    GLYPH_ROOM + TRACK / 2. + percent.clamp(0., 100.) / 100. * (TRACK_LONG - GLYPH_ROOM - TRACK)
}

/// The other way: the level a press at `x` from the track's left sets (in the look's units),
/// so a press puts the knob where it is.
pub fn percent_at(x: f32) -> u32 {
    (((x - GLYPH_ROOM - TRACK / 2.) / (TRACK_LONG - GLYPH_ROOM - TRACK)).clamp(0., 1.) * 100.).round() as u32
}

/// The same, from a press at `x` over the track drawn at `bounds`.
pub fn percent_in(bounds: gpui::Bounds<gpui::Pixels>, x: gpui::Pixels) -> u32 {
    percent_at(f32::from(x - bounds.left()) / scale())
}

/// A level drawn at `percent`, as Control Center's: the bright fill to a white knob with its
/// shadow on a see-through track, round. What's pressed, its glyph and its keys are the
/// caller's (in it, `TRACK_LONG` by `TRACK`, it's `.relative()`).
pub fn level(id: impl Into<ElementId>, percent: f32) -> Stateful<Div> {
    let knob = knob_at(percent);
    div()
        .id(id)
        .relative()
        .flex_none()
        .w(px(TRACK_LONG))
        .h(px(TRACK))
        .rounded_full()
        .cursor_pointer()
        .child(div().absolute().top_0().left_0().size_full().rounded_full().overflow_hidden().bg(color::level_track()).child(div().h_full().w(px(knob + TRACK / 2.)).rounded_full().bg(rgb(color::level_fill()))))
        // (Beside the track, not in it: its shadow isn't cut off with it.)
        .child(div().absolute().top_0().left(px(knob - TRACK / 2.)).size(px(TRACK)).rounded_full().bg(rgb(color::level_knob())).shadow(knob_shadow()))
}

/// A level's glyph, at its left, on the fill.
pub fn level_glyph(icon: &str) -> Div {
    div().absolute().top_0().left_0().w(px(GLYPH_ROOM)).h(px(TRACK)).flex().items_center().justify_center().child(crate::icons::glyph(icon, 14., color::level_ink()))
}

// The app's quiet look (nami as a package manager, and namiOS's Settings with it), as a Mac's
// store and settings have it: one slate button on a screen for what it's for, every other
// button grey; what's picked a soft grey with the text's colour, not the slate; counts and tags
// grey. Colour where it means something: links, the main button, a running service's dot, what
// changed, what went wrong. The apps' icons bring the rest.

fn dark() -> bool {
    color::DARK.load(std::sync::atomic::Ordering::Relaxed)
}

/// The text's colour, `alpha` of it (0-255), see-through: the greys of buttons, fills and tags,
/// the same in light and dark.
pub fn ink(alpha: u32) -> gpui::Rgba {
    gpui::rgba((color::text() << 8) | alpha)
}

/// A sidebar's line: picked, a soft grey fill under the text's colour (in place of look's slate
/// one); else its hover. Its icon and label are the caller's.
pub fn side_row(id: impl Into<ElementId>, picked: bool) -> Stateful<Div> {
    let row = div()
        .id(id)
        .flex_none()
        .h(px(30.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(10.))
        .rounded(px(shape::ROW))
        .text_size(px(text::BODY))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(color::text()));
    if picked {
        row.bg(ink(if dark() { 0x1f } else { 0x16 })).press_row()
    } else {
        row.hover(|s| s.bg(color::hover_pill())).press_row()
    }
}

/// A sidebar line's icon: the text's colour on the picked one, quiet otherwise, faint where it
/// can't be used (`on`).
pub fn side_icon(picked: bool, on: bool) -> u32 {
    if !on { color::faint() } else if picked { color::text() } else { color::side_muted() }
}

/// A count at a line's end (updates waiting, files an app changed): grey, never the slate.
pub fn badge(n: usize) -> Div {
    div()
        .flex_none()
        .ml_auto()
        .min_w(px(20.))
        .h(px(20.))
        .px(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(ink(0x17))
        .text_color(rgb(color::side_muted()))
        .text_size(px(text::META))
        .font_weight(FontWeight::SEMIBOLD)
        .child(n.to_string())
}

/// A small outlined tag beside a name ("Just you", "System"): quiet, its edge a hairline.
pub fn tag(said: impl Into<gpui::SharedString>) -> Div {
    div()
        .flex_none()
        .px(px(8.))
        .h(px(20.))
        .flex()
        .items_center()
        .rounded_full()
        .border_1()
        .border_color(ink(0x26))
        .text_size(px(text::META))
        .text_color(rgb(color::muted()))
        .child(said.into())
}

/// A grey button: what isn't a screen's main one.
pub fn grey(id: impl Into<ElementId>, label: impl Into<gpui::SharedString>) -> Stateful<Div> {
    push_button(id, label, false, true).font_weight(FontWeight::SEMIBOLD)
}

/// The screen's one slate button, for what it's for (Install, Update All, Keep).
pub fn main_button(id: impl Into<ElementId>, label: impl Into<gpui::SharedString>, enabled: bool) -> Stateful<Div> {
    push_button(id, label, true, enabled).font_weight(FontWeight::SEMIBOLD)
}

/// A round grey button with a glyph in it (a task's Run, More).
pub fn round_button(id: impl Into<ElementId>, glyph: &str, label: impl Into<gpui::SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(color::push())
        .cursor_pointer()
        .hover(|s| s.bg(color::round_off()))
        .press_row()
        .role(gpui::Role::Button)
        .aria_label(label.into())
        .child(crate::icons::glyph(glyph, 13., color::text()))
}

/// A link: the slate, as a Mac's are its accent.
pub fn link(id: impl Into<ElementId>, label: impl Into<gpui::SharedString>) -> Stateful<Div> {
    div().id(id).flex_none().text_color(rgb(color::key())).cursor_pointer().press().hover(|s| s.underline()).child(label.into())
}

/// A row of filters to pick one of: a grey track, the picked one a raised white pill on it (a
/// lit grey in dark), as a Mac's segmented controls; round like every button (`round` is
/// kept for the callers, and no longer changes the shape).
pub fn filters(_round: bool) -> Div {
    div().flex().flex_none().items_center().p(px(3.)).gap(px(2.)).rounded_full().bg(color::segment_track())
}

/// One of `filters`.
pub fn filter(id: impl Into<ElementId>, label: impl Into<gpui::SharedString>, picked: bool, _round: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(28.))
        .px(px(13.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_size(px(text::CALLOUT))
        .cursor_pointer()
        .role(gpui::Role::Button)
        .when(picked, |d| d.bg(rgb(color::segment_on())).shadow(knob_shadow()).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::text())))
        .when(!picked, |d| d.font_weight(FontWeight::MEDIUM).text_color(rgb(color::muted())).hover(|s| s.bg(color::hover_pill())))
        .press_row()
        .child(label.into())
}

/// A white card on the page's grey, its edge a hairline, as the store's are.
pub fn sheet() -> Div {
    div().rounded(px(shape::CARD)).bg(rgb(color::panel())).border_1().border_color(ink(0x14)).overflow_hidden()
}

/// A line in a `sheet`, the hairline above it but for the first.
pub fn sheet_row(first: bool) -> Div {
    div().flex().items_center().gap(px(14.)).px(px(18.)).py(px(10.)).when(!first, |d| d.border_t_1().border_color(ink(0x10)))
}

/// A page's own title, big, as the store's pages have theirs, and the line under it, if any.
pub fn page_title(title: impl Into<gpui::SharedString>, line: Option<gpui::AnyElement>) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(title.into()))
        .children(line.map(|l| div().text_size(px(text::BODY)).text_color(rgb(color::muted())).child(l)))
}

/// A tile of a few letters, for what has no icon of its own (a command, a file, a project's
/// language): grey, in mono.
pub fn letters(said: &str, size: f32) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(size * 0.26))
        .bg(ink(0x10))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px((size * 0.37).max(10.)))
        .font_family("Geist Mono")
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(color::muted()))
        .child(said.to_string())
}

/// Lines of a file, dark, in mono: what's in your file.
pub fn code_block(text: impl Into<gpui::SharedString>) -> Div {
    div()
        .px(px(14.))
        .py(px(10.))
        .rounded(px(shape::CONTROL))
        .bg(rgb(if dark() { 0x0e1117 } else { 0x15181f }))
        .text_color(rgb(0xe4e6eb))
        .font_family("Geist Mono")
        .text_size(px(text::CALLOUT))
        .line_height(px(21.))
        .child(text.into())
}

/// A small state dot: green while running, the slate for what changed.
pub fn dot(colour: u32) -> Div {
    div().size(px(8.)).flex_none().rounded_full().bg(rgb(colour))
}

/// A small heading over a list in a page's column ("WITH THEIR APPS").
pub fn column_heading(title: &str) -> Div {
    div().px(px(6.)).pt(px(10.)).pb(px(2.)).text_size(px(text::META)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color::grouped_muted())).child(title.to_uppercase())
}

/// A line of a master list (a file, a project): picked, a soft grey fill.
pub fn list_row(id: impl Into<ElementId>, picked: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(12.))
        .py(px(9.))
        .rounded(px(shape::CONTROL + 1.))
        .cursor_pointer()
        .when(picked, |d| d.bg(ink(0x12)))
        .when(!picked, |d| d.hover(|s| s.bg(color::hover_pill())))
        .press_row()
}

/// A quiet sentence on a page.
pub fn quiet(said: impl Into<gpui::SharedString>) -> Div {
    div().text_size(px(text::CALLOUT)).text_color(rgb(color::muted())).child(said.into())
}

// Settings' rows, as a Mac's System Settings has them: Settings' pages' and nami's own
// (Updates, the apps from downloads).

/// A line in a group: what it is on the left, how it's changed on the right, as a Mac's
/// System Settings has them (`look::group_row`).
pub fn setting_row(label: &str, hint: &str, control: gpui::AnyElement, last: bool) -> Div {
    crate::look::group_row(last)
        .flex_wrap()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(150.))
                .child(div().text_size(px(text::BODY)).truncate().child(label.to_string()))
                .when(!hint.is_empty(), |d| d.child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(hint.to_string()))),
        )
        // (No wider than the row: a long set of choices goes onto a second line, at its right.)
        .child(div().flex_none().max_w_full().flex().justify_end().child(control))
}

/// A group of lines, under its heading (if it has one).
pub fn setting_card(title: &str, rows: Vec<Div>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .when(!title.is_empty(), |d| d.child(crate::look::group_heading(title)))
        .child(crate::look::group().children(rows))
}

/// A switch, pressed as a whole: the desktop's (`ui::switch_look`).
pub fn switch(id: ElementId, on: bool) -> Stateful<Div> {
    div().id(id).flex_none().rounded_full().cursor_pointer().press().child(crate::ui::switch_look("switch", on))
}

/// A switch, as a screen reader's told of it.
pub fn said_switch(el: Stateful<Div>, label: &str, on: bool) -> Stateful<Div> {
    el.role(gpui::Role::Switch).aria_label(label.to_string()).aria_toggled(if on { gpui::Toggled::True } else { gpui::Toggled::False })
}
