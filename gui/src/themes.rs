//! Themes and accent colours: which colours namiOS's windows are drawn in. Those it comes with,
//! and a person's own files in ~/.local/share/nami/themes (name, then `light:` and `dark:`,
//! each of text, muted, panel, ground, soft, line, fill, on-text; as "#rrggbb").

use std::path::PathBuf;

use crate::look::color;

/// Eight colours of a theme: panel, ground, soft, line, text, muted, fill, on-text.
pub type Eight = [u32; 8];

pub struct Theme {
    pub id: String,
    pub name: String,
    pub light: Eight,
    pub dark: Eight,
    /// A person's own file, not one namiOS comes with.
    pub own: bool,
}

const BUILT_IN: &[(&str, &str, Eight, Eight)] = &[
    ("nami", "namiOS", [0xffffff, 0xfafafa, 0xf4f4f5, 0xe4e4e7, 0x18181b, 0x71717a, 0x33394a, 0xffffff], [0x191d26, 0x13161d, 0x212634, 0x2a3040, 0xf2f3f6, 0x9aa2b2, 0xcdd3de, 0x13161d]),
    ("graphite", "Graphite", [0xffffff, 0xf5f5f5, 0xeeeeee, 0xdddddd, 0x111111, 0x666666, 0x222222, 0xffffff], [0x1c1c1c, 0x111111, 0x2a2a2a, 0x3a3a3a, 0xeeeeee, 0x999999, 0xeeeeee, 0x111111]),
    ("nord", "Nord", [0xeceff4, 0xe5e9f0, 0xd8dee9, 0xd0d6e2, 0x2e3440, 0x4c566a, 0x5e81ac, 0xffffff], [0x3b4252, 0x2e3440, 0x434c5e, 0x4c566a, 0xeceff4, 0xd8dee9, 0x88c0d0, 0x2e3440]),
    ("solarized", "Solarized", [0xfdf6e3, 0xeee8d5, 0xe9e2cb, 0xd9d2bc, 0x073642, 0x657b83, 0x268bd2, 0xfdf6e3], [0x073642, 0x002b36, 0x0f4552, 0x1c5361, 0xeee8d5, 0x93a1a1, 0x2aa198, 0x002b36]),
    ("gruvbox", "Gruvbox", [0xfbf1c7, 0xf2e5bc, 0xebdbb2, 0xd5c4a1, 0x3c3836, 0x7c6f64, 0xaf3a03, 0xfbf1c7], [0x282828, 0x1d2021, 0x3c3836, 0x504945, 0xebdbb2, 0xa89984, 0xfabd2f, 0x282828]),
    ("catppuccin", "Catppuccin", [0xeff1f5, 0xe6e9ef, 0xdce0e8, 0xccd0da, 0x4c4f69, 0x6c6f85, 0x8839ef, 0xeff1f5], [0x1e1e2e, 0x181825, 0x313244, 0x45475a, 0xcdd6f4, 0xa6adc8, 0xcba6f7, 0x1e1e2e]),
    ("rose-pine", "Rosé Pine", [0xfaf4ed, 0xfffaf3, 0xf2e9e1, 0xdfdad9, 0x575279, 0x797593, 0x907aa9, 0xfaf4ed], [0x1f1d2e, 0x191724, 0x26233a, 0x403d52, 0xe0def4, 0x908caa, 0xebbcba, 0x191724]),
    (HIGH_CONTRAST, "High Contrast", [0xffffff, 0xffffff, 0xe8e8e8, 0x595959, 0x000000, 0x2b2b2b, 0x00337a, 0xffffff], [0x000000, 0x000000, 0x262626, 0xa6a6a6, 0xffffff, 0xe0e0e0, 0xffe14d, 0x000000]),
];

/// The theme that's for reading: its colours stay as they are, whatever accent's picked.
pub const HIGH_CONTRAST: &str = "high-contrast";

/// id, name, on light, on dark.
pub const ACCENTS: &[(&str, &str, u32, u32)] = &[
    ("slate", "Slate", 0x33394a, 0xcdd3de),
    ("blue", "Blue", 0x2f6fde, 0x7aa7f0),
    ("teal", "Teal", 0x0f8a8a, 0x5cc8c8),
    ("green", "Green", 0x2e7d4f, 0x7cc79a),
    ("orange", "Orange", 0xc4621a, 0xe8a06a),
    ("red", "Red", 0xc0392b, 0xe88a80),
    ("pink", "Pink", 0xc23a82, 0xe88fc0),
    ("purple", "Purple", 0x7a4fc9, 0xb59af0),
];

pub fn own_folder() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".local/share/nami/themes")
}

fn hex(v: &serde_yaml::Value) -> Option<u32> {
    u32::from_str_radix(v.as_str()?.trim_start_matches('#'), 16).ok()
}

fn eight(v: &serde_yaml::Value) -> Option<Eight> {
    Some([hex(&v["panel"])?, hex(&v["ground"])?, hex(&v["soft"])?, hex(&v["line"])?, hex(&v["text"])?, hex(&v["muted"])?, hex(&v["fill"])?, hex(&v["on-text"])?])
}

/// A theme's file, if that's what it is.
pub fn from_text(id: &str, text: &str) -> Option<Theme> {
    let top: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    Some(Theme { id: id.to_string(), name: top["name"].as_str().unwrap_or(id).to_string(), light: eight(&top["light"])?, dark: eight(&top["dark"])?, own: true })
}

pub fn all() -> Vec<Theme> {
    let mut themes: Vec<Theme> = BUILT_IN.iter().map(|(id, name, light, dark)| Theme { id: id.to_string(), name: name.to_string(), light: *light, dark: *dark, own: false }).collect();
    let mut own: Vec<Theme> = std::fs::read_dir(own_folder())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|f| {
            let path = f.path();
            from_text(path.file_stem()?.to_str()?, &std::fs::read_to_string(&path).ok()?)
        })
        .collect();
    own.sort_by(|a, b| a.name.cmp(&b.name));
    themes.extend(own);
    themes
}

/// The colours of a theme and an accent, on light and on dark. (Nothing asked: None.)
pub fn chosen(theme: Option<&str>, accent: Option<&str>) -> Option<(Eight, Eight)> {
    let accent = ACCENTS.iter().find(|a| Some(a.0) == accent && a.0 != "slate" && theme != Some(HIGH_CONTRAST));
    let theme = theme.filter(|t| *t != "nami");
    if theme.is_none() && accent.is_none() {
        return None;
    }
    let themes = all();
    let found = theme.and_then(|t| themes.iter().find(|x| x.id == t)).or_else(|| themes.first())?;
    let (mut light, mut dark) = (found.light, found.dark);
    if let Some(a) = accent {
        (light[6], light[7]) = (a.2, 0xffffff);
        (dark[6], dark[7]) = (a.3, 0x13161d);
    }
    Some((light, dark))
}

/// Draw in these from now on.
pub fn apply(theme: Option<&str>, accent: Option<&str>) {
    let colours = chosen(theme, accent);
    for (dark, set) in [(false, colours.map(|c| c.0)), (true, colours.map(|c| c.1))] {
        // Text, muted, faint, line, field line, soft, tile, ground, panel, fill, on text, key.
        let slots: [Option<u32>; 12] = match set {
            Some([panel, ground, soft, line, text, muted, fill, on_text]) => [text, muted, muted, line, line, soft, panel, ground, panel, fill, on_text, fill].map(Some),
            None => [None; 12],
        };
        for (i, value) in slots.into_iter().enumerate() {
            color::set_slot(dark, i, value);
        }
    }
    // A Mac: its own accent colour (System Settings, Appearance), as its apps draw their main
    // buttons and what's picked in, not namiOS's slate; unless this person picked nami's own.
    if crate::look::MAC && theme.is_none() && accent.is_none() {
        let (light, dark) = mac_accent(mac_accent_setting().as_deref());
        color::set_slot(false, 11, Some(light));
        color::set_slot(true, 11, Some(dark));
    }
}

/// The Mac's accent colour as `defaults read -g AppleAccentColor` says it (none: blue, the
/// Mac's own; with "Multicolour" there's none either), read once.
fn mac_accent_setting() -> Option<String> {
    static SAID: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    SAID.get_or_init(|| {
        let out = std::process::Command::new("/usr/bin/defaults").args(["read", "-g", "AppleAccentColor"]).stderr(std::process::Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    })
    .clone()
}

/// A Mac's accent colours (controlAccentColor), in light and in dark, by AppleAccentColor's
/// number.
pub fn mac_accent(setting: Option<&str>) -> (u32, u32) {
    match setting {
        Some("-1") => (0x8c8c8c, 0x989898), // graphite
        Some("0") => (0xe0383e, 0xff5257),  // red
        Some("1") => (0xf7821b, 0xf7821b),  // orange
        Some("2") => (0xffc600, 0xffd60a),  // yellow
        Some("3") => (0x62ba46, 0x62ba46),  // green
        Some("5") => (0x953d96, 0xa550a7),  // purple
        Some("6") => (0xf74f9e, 0xf74f9e),  // pink
        _ => (0x007aff, 0x0a84ff),          // blue
    }
}

/// The same for GTK3's apps (adw-gtk3's names). Its CSS has no light and dark: one set, and solid.
pub fn gtk3_css(c: Eight) -> String {
    let [panel, ground, soft, _, text, _, fill, on_text] = c;
    let mut css = String::new();
    for (name, colour) in [
        ("window_bg", ground),
        ("window_fg", text),
        ("view_bg", panel),
        ("view_fg", text),
        ("headerbar_bg", panel),
        ("headerbar_fg", text),
        ("headerbar_backdrop", ground),
        ("sidebar_bg", soft),
        ("sidebar_fg", text),
        ("sidebar_backdrop", ground),
        ("card_bg", panel),
        ("card_fg", text),
        ("dialog_bg", panel),
        ("dialog_fg", text),
        ("popover_bg", panel),
        ("popover_fg", text),
        ("accent_bg", fill),
        ("accent_fg", on_text),
        ("accent", fill),
    ] {
        css += &format!("@define-color {name}_color #{colour:06x};\n");
    }
    css
}

/// The same for GTK's apps, as CSS.
pub fn gtk_css(light: Eight, dark: Eight, glass: crate::look::Glass) -> String {
    // With Frosted glass: see-through around what's read, as on a Mac, as much as its step
    // says (look::Glass::gtk), milkier in light than in dark; what's read stays solid. (Each
    // app takes light's or dark's as the look changes: prefers-color-scheme, below.)
    let see = |colour: u32, percent: u32| if percent < 100 { format!("rgba({}, {}, {}, 0.{percent:02})", colour >> 16, (colour >> 8) & 255, colour & 255) } else { format!("#{colour:06x}") };
    let block = |c: Eight, dark: bool| {
        let [window, bar, bar_away, side, side_away, ..] = glass.gtk(dark);
        let [panel, ground, soft, _, text, _, fill, on_text] = c;
        let (window, bar, bar_away, side, side_away) = (see(ground, window), see(panel, bar), see(ground, bar_away), see(soft, side), see(ground, side_away));
        format!(
            "--window-bg-color: {window}; --window-fg-color: #{text:06x}; --view-bg-color: #{panel:06x}; --view-fg-color: #{text:06x}; --headerbar-bg-color: {bar}; --headerbar-fg-color: #{text:06x}; --headerbar-backdrop-color: {bar_away}; --sidebar-bg-color: {side}; --sidebar-fg-color: #{text:06x}; --sidebar-backdrop-color: {side_away}; --secondary-sidebar-bg-color: #{ground:06x}; --card-bg-color: #{panel:06x}; --dialog-bg-color: #{panel:06x}; --popover-bg-color: #{panel:06x}; --accent-bg-color: #{fill:06x}; --accent-fg-color: #{on_text:06x}; --accent-color: #{fill:06x};"
        )
    };
    // Files draws its whole window as a page: only the page is solid, so the sidebar shows through.
    let panes = |c: Eight, dark: bool| {
        let [.., files_bars, files_side] = glass.gtk(dark);
        format!(
            "window.nautilus-window {{ background-color: transparent; }} window.nautilus-window toolbarview {{ background-color: {bars}; }} window.nautilus-window .sidebar-pane toolbarview {{ background-color: transparent; }} .sidebar-pane {{ background-color: {side}; }} .content-pane, window.nautilus-window gridview, window.nautilus-window columnview {{ background-color: #{page:06x}; }}",
            bars = see(c[1], files_bars),
            side = see(c[2], files_side),
            page = c[0]
        )
    };
    let (extra, extra_dark) = if glass.on() { (panes(light, false), panes(dark, true)) } else { Default::default() };
    format!(":root {{ {} }}\n{extra}\n@media (prefers-color-scheme: dark) {{ :root {{ {} }} {extra_dark} }}\n", block(light, false), block(dark, true))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_macs_accent_is_its_own_blue_unless_another_is_picked() {
        assert_eq!(super::mac_accent(None), (0x007aff, 0x0a84ff));
        assert_eq!(super::mac_accent(Some("4")), (0x007aff, 0x0a84ff));
        assert_eq!(super::mac_accent(Some("-1")).0, 0x8c8c8c);
        assert_eq!(super::mac_accent(Some("5")).0, 0x953d96);
    }

    use super::*;

    /// WCAG's contrast ratio of two colours.
    fn ratio(a: u32, b: u32) -> f64 {
        let lum = |c: u32| {
            let channel = |v: u32| {
                let v = f64::from(v & 255) / 255.;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * channel(c >> 16) + 0.7152 * channel(c >> 8) + 0.0722 * channel(c)
        };
        let (a, b) = (lum(a), lum(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn high_contrast_is_what_it_says() {
        let (_, _, light, dark) = BUILT_IN.iter().find(|t| t.0 == HIGH_CONTRAST).unwrap();
        for [panel, ground, soft, line, text, muted, fill, on_text] in [*light, *dark] {
            // WCAG's AAA for all that's read, on all it's read on; 3:1 for lines and what's picked.
            for page in [panel, ground, soft] {
                assert!(ratio(text, page) >= 7., "text on {page:06x}: {:.1}", ratio(text, page));
                assert!(ratio(muted, page) >= 7., "muted on {page:06x}: {:.1}", ratio(muted, page));
                assert!(ratio(fill, page) >= 3., "fill on {page:06x}: {:.1}", ratio(fill, page));
            }
            assert!(ratio(on_text, fill) >= 7., "on-text on fill: {:.1}", ratio(on_text, fill));
            assert!(ratio(line, panel) >= 3., "line on panel: {:.1}", ratio(line, panel));
            assert!(ratio(line, soft) >= 3., "line on soft: {:.1}", ratio(line, soft));
        }
    }

    #[test]
    fn gtk_is_milkier_in_light_than_in_dark() {
        use crate::look::Glass;
        let (light, dark) = (BUILT_IN[0].2, BUILT_IN[0].3);
        let css = gtk_css(light, dark, Glass::Medium);
        let (day, night) = css.split_once("@media (prefers-color-scheme: dark)").unwrap();
        // (The window's ground: Medium's 80% in light, 66% in dark; Files' sidebar the same way.)
        assert!(day.contains("--window-bg-color: rgba(250, 250, 250, 0.80)"), "{day}");
        assert!(night.contains("--window-bg-color: rgba(19, 22, 29, 0.66)"), "{night}");
        assert!(day.contains(".sidebar-pane { background-color: rgba(244, 244, 245, 0.68)") && night.contains(".sidebar-pane { background-color: rgba(33, 38, 52, 0.58)"));
        // (Off: solid, both.)
        assert!(!gtk_css(light, dark, Glass::Off).contains("rgba("));
    }

    #[test]
    fn an_accent_leaves_high_contrast_alone() {
        let (light, _) = chosen(Some(HIGH_CONTRAST), Some("orange")).unwrap();
        assert_eq!(light[6], 0x00337a);
    }
}
