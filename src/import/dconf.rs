//! GNOME's settings from `dconf dump /`: the ones a person chose (keyboard
//! layouts, favourite apps, theme, dark mode, night light, touchpad, key
//! bindings...) as nami `settings:`; GNOME Shell's own (its extensions,
//! mutter's) have no place on namiOS. App state (window sizes,
//! recent files, last-used folders) stays behind.

use std::collections::BTreeMap;

use serde_yaml::Value;

/// The settings worth bringing: a dconf path, and its keys (`*`: all).
/// Keys that are state, not choices, are left out everywhere (see `is_state`).
const WANTED: &[(&str, &[&str])] = &[
    ("org/gnome/desktop/input-sources", &["sources", "xkb-options", "per-window"]),
    ("org/gnome/desktop/interface", &["*"]),
    ("org/gnome/desktop/wm/preferences", &["*"]),
    ("org/gnome/desktop/wm/keybindings", &["*"]),
    ("org/gnome/desktop/peripherals/touchpad", &["*"]),
    ("org/gnome/desktop/peripherals/mouse", &["*"]),
    ("org/gnome/desktop/peripherals/keyboard", &["*"]),
    ("org/gnome/desktop/a11y", &["*"]),
    ("org/gnome/desktop/a11y/applications", &["*"]),
    ("org/gnome/desktop/a11y/interface", &["*"]),
    ("org/gnome/desktop/a11y/keyboard", &["*"]),
    ("org/gnome/desktop/a11y/magnifier", &["*"]),
    ("org/gnome/desktop/background", &["picture-uri", "picture-uri-dark", "picture-options", "primary-color", "secondary-color", "color-shading-type"]),
    ("org/gnome/desktop/screensaver", &["lock-enabled", "lock-delay", "picture-uri"]),
    ("org/gnome/desktop/session", &["idle-delay"]),
    ("org/gnome/desktop/calendar", &["*"]),
    ("org/gnome/desktop/privacy", &["remember-recent-files", "recent-files-max-age", "remove-old-trash-files", "remove-old-temp-files", "old-files-age", "report-technical-problems"]),
    ("org/gnome/desktop/sound", &["event-sounds", "allow-volume-above-100-percent", "theme-name"]),
    ("org/gnome/desktop/notifications", &["show-banners", "show-in-lock-screen"]),
    ("org/gnome/desktop/datetime", &["automatic-timezone"]),
    ("org/gnome/desktop/media-handling", &["*"]),
    ("org/gnome/shell", &["favorite-apps"]),
    ("org/gnome/settings-daemon/plugins/media-keys", &["*"]),
    ("org/gnome/settings-daemon/plugins/color", &["night-light-enabled", "night-light-temperature", "night-light-schedule-automatic", "night-light-schedule-from", "night-light-schedule-to"]),
    ("org/gnome/settings-daemon/plugins/power", &["sleep-inactive-ac-type", "sleep-inactive-ac-timeout", "sleep-inactive-battery-type", "sleep-inactive-battery-timeout", "power-button-action", "idle-dim", "power-saver-profile-on-low-battery"]),
    ("org/gnome/nautilus/preferences", &["default-folder-viewer", "show-hidden-files", "show-delete-permanently", "show-create-link", "click-policy", "recursive-search", "show-image-thumbnails", "date-time-format"]),
    ("org/gnome/nautilus/list-view", &["default-zoom-level", "use-tree-view", "default-visible-columns"]),
    ("org/gnome/nautilus/icon-view", &["default-zoom-level"]),
    ("org/gtk/settings/file-chooser", &["show-hidden", "sort-directories-first", "clock-format"]),
    ("org/gtk/gtk4/settings/file-chooser", &["show-hidden", "sort-directories-first"]),
    ("org/gnome/TextEditor", &["*"]),
    ("org/gnome/Ptyxis", &["*"]),
    ("org/gnome/system/location", &["enabled"]),
];

/// Keys that are where things were, not what someone chose.
fn is_state(key: &str) -> bool {
    ["window", "last-", "-state", "maximized", "recent", "history", "timestamp", "geometry", "welcome-dialog", "migrated"]
        .iter()
        .any(|w| key.contains(w))
        && !key.starts_with("switch-") && !key.starts_with("move-to-") && !key.starts_with("toggle-") && !key.contains("keybinding")
}

const CUSTOM_KEYS: &str = "org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/";
const CUSTOM_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";

/// What came of a dump.
#[derive(Debug, Default)]
pub struct Picked {
    /// schema (or `schema:/path/` for a relocatable one) -> key -> value.
    pub settings: BTreeMap<String, BTreeMap<String, Value>>,
    /// Keys left out, by dconf path (app state and the like), for counting.
    pub left_out: usize,
}

/// The useful settings in a `dconf dump /`.
pub fn pick(dump: &str) -> Picked {
    let mut out = Picked::default();
    let mut path = String::new();
    for line in dump.lines() {
        let line = line.trim_end();
        if let Some(p) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            path = p.trim_matches('/').to_string();
            continue;
        }
        let Some((key, text)) = line.split_once('=') else { continue };
        let (key, text) = (key.trim(), text.trim());
        if key.is_empty() || path.is_empty() {
            continue;
        }
        let schema = if let Some(name) = format!("{path}/").strip_prefix(CUSTOM_KEYS).map(|r| r.trim_end_matches('/').to_string()).filter(|n| !n.is_empty() && !n.contains('/')) {
            Some(format!("{CUSTOM_SCHEMA}:/{CUSTOM_KEYS}{name}/"))
        } else {
            WANTED.iter().find(|(p, _)| *p == path).filter(|(_, keys)| (keys == &["*"] && !is_state(key)) || keys.contains(&key)).map(|_| path.replace('/', "."))
        };
        let Some(schema) = schema else {
            out.left_out += 1;
            continue;
        };
        let value = to_yaml(text);
        out.settings.entry(schema).or_default().insert(key.to_string(), value);
    }
    out
}

/// A GVariant value as YAML: text, numbers, true/false and lists of them
/// as they are; anything else (tuples, dictionaries: keyboard layouts are
/// `[('xkb', 'us')]`) as its GVariant text, tagged `!gvariant`, which nami
/// hands to gsettings as it is.
pub fn to_yaml(text: &str) -> Value {
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    match p.value() {
        Some(v) if p.at_end() => v,
        _ => gvariant_tag(text),
    }
}

pub fn gvariant_tag(text: &str) -> Value {
    Value::Tagged(Box::new(serde_yaml::value::TaggedValue { tag: serde_yaml::value::Tag::new("gvariant"), value: Value::String(text.to_string()) }))
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn at_end(&mut self) -> bool {
        self.ws();
        self.i == self.s.len()
    }
    fn peek(&self) -> u8 {
        self.s.get(self.i).copied().unwrap_or(0)
    }
    fn word(&mut self) -> &str {
        let start = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || b"_-.+".contains(&self.s[self.i])) {
            self.i += 1;
        }
        std::str::from_utf8(&self.s[start..self.i]).unwrap_or("")
    }
    /// Only the simple values; None for what YAML can't say plainly.
    fn value(&mut self) -> Option<Value> {
        self.ws();
        // A type in front: `uint32 5`, `@as []`.
        if self.peek() == b'@' {
            while self.i < self.s.len() && !self.s[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
            return self.value();
        }
        match self.peek() {
            b'\'' | b'"' => self.string().map(Value::String),
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.ws();
                    if self.peek() == b']' {
                        self.i += 1;
                        break;
                    }
                    let v = self.value()?;
                    if matches!(v, Value::Sequence(_)) {
                        return None;
                    }
                    items.push(v);
                    self.ws();
                    match self.peek() {
                        b',' => self.i += 1,
                        b']' => {}
                        _ => return None,
                    }
                }
                Some(Value::Sequence(items))
            }
            c if c.is_ascii_alphanumeric() || c == b'-' || c == b'+' || c == b'.' => {
                let save = self.i;
                let w = self.word().to_string();
                match w.as_str() {
                    "true" => Some(Value::Bool(true)),
                    "false" => Some(Value::Bool(false)),
                    "uint32" | "int32" | "int64" | "uint64" | "int16" | "uint16" | "byte" | "double" | "string" | "boolean" => self.value(),
                    _ => {
                        if let Ok(n) = w.parse::<i64>() {
                            Some(Value::Number(n.into()))
                        } else if let Some(h) = w.strip_prefix("0x") {
                            i64::from_str_radix(h, 16).ok().map(|n| Value::Number(n.into()))
                        } else if w.contains(|c: char| c.is_ascii_digit()) && w.parse::<f64>().is_ok_and(f64::is_finite) {
                            Some(Value::Number(w.parse::<f64>().unwrap().into()))
                        } else {
                            self.i = save;
                            None
                        }
                    }
                }
            }
            _ => None,
        }
    }
    fn string(&mut self) -> Option<String> {
        let q = self.peek();
        self.i += 1;
        let mut out = Vec::new();
        while self.i < self.s.len() {
            let c = self.s[self.i];
            self.i += 1;
            if c == q {
                return String::from_utf8(out).ok();
            }
            if c != b'\\' {
                out.push(c);
                continue;
            }
            let e = *self.s.get(self.i)?;
            self.i += 1;
            match e {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'u' | b'U' => {
                    let n = if e == b'u' { 4 } else { 8 };
                    let hex = std::str::from_utf8(self.s.get(self.i..self.i + n)?).ok()?;
                    self.i += n;
                    let ch = char::from_u32(u32::from_str_radix(hex, 16).ok()?)?;
                    out.extend(ch.to_string().as_bytes());
                }
                other => out.push(other),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        assert_eq!(to_yaml("'prefer-dark'"), Value::String("prefer-dark".into()));
        assert_eq!(to_yaml("\"it's\""), Value::String("it's".into()));
        assert_eq!(to_yaml("'K\\u00f8benhavn'"), Value::String("København".into()));
        assert_eq!(to_yaml("true"), Value::Bool(true));
        assert_eq!(to_yaml("uint32 3700"), Value::Number(3700.into()));
        assert_eq!(to_yaml("1.25"), Value::Number(1.25.into()));
        assert_eq!(to_yaml("@as []"), Value::Sequence(vec![]));
        assert_eq!(to_yaml("['<Super>1', '<Alt>F1']"), serde_yaml::from_str::<Value>("['<Super>1', '<Alt>F1']").unwrap());
        // Tuples aren't YAML: kept as GVariant text.
        assert_eq!(to_yaml("[('xkb', 'us'), ('xkb', 'dk')]"), gvariant_tag("[('xkb', 'us'), ('xkb', 'dk')]"));
        assert_eq!(to_yaml("(6.0, 21.0)"), gvariant_tag("(6.0, 21.0)"));
    }

    #[test]
    fn picks_choices_not_state() {
        let dump = include_str!("../../tests/fixtures/import/computer/ubuntu/dconf.txt");
        let p = pick(dump);
        let s = |schema: &str, key: &str| p.settings.get(schema).and_then(|k| k.get(key)).cloned();
        assert_eq!(s("org.gnome.desktop.interface", "color-scheme"), Some(Value::String("prefer-dark".into())));
        assert_eq!(s("org.gnome.desktop.input-sources", "sources"), Some(gvariant_tag("[('xkb', 'dk'), ('xkb', 'us')]")));
        assert_eq!(s("org.gnome.settings-daemon.plugins.color", "night-light-enabled"), Some(Value::Bool(true)));
        assert_eq!(s("org.gnome.desktop.peripherals.touchpad", "tap-to-click"), Some(Value::Bool(true)));
        let custom = format!("{CUSTOM_SCHEMA}:/{CUSTOM_KEYS}custom0/");
        assert_eq!(s(&custom, "binding"), Some(Value::String("<Super>Return".into())));
        // GNOME Shell's extensions stay behind.
        assert_eq!(s("org.gnome.shell.extensions.dash-to-dock", "dock-position"), None);
        assert!(s("org.gnome.shell", "enabled-extensions").is_none());
        // App state stays behind.
        assert!(s("org.gnome.nautilus.window-state", "initial-size").is_none());
        assert!(!p.settings.keys().any(|k| k.contains("evolution") || k.contains("control-center")));
        assert!(p.left_out >= 4);
    }
}
