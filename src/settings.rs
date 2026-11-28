//! `settings:` in your nami file: desktop settings (GNOME and anything else that
//! uses GSettings/dconf), applied with `gsettings` so the schemas validate
//! them. Keys you remove from the config are reset to their defaults, and
//! they're part of each generation, so rollback restores them too.
//!
//! ```yaml
//! settings:
//!   org.gnome.desktop.interface:
//!     color-scheme: prefer-dark
//!     clock-format: 24h
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};
use serde_yaml::Value;

pub type Settings = BTreeMap<String, BTreeMap<String, Value>>;

/// A YAML value as GVariant text, which `gsettings set` parses against the
/// key's type: 'text', true, 3, 1.5, ['a', 'b']. What YAML can't say
/// (tuples: keyboard layouts are `[('xkb', 'us')]`) is written as GVariant
/// text tagged `!gvariant`, and passed on as it is.
pub fn gvariant(v: &Value) -> Result<String> {
    Ok(match v {
        Value::Tagged(t) if t.tag == "gvariant" => match &t.value {
            Value::String(s) if !s.trim().is_empty() => s.clone(),
            _ => bail!("!gvariant takes GVariant text, like \"[('xkb', 'us')]\""),
        },
        Value::String(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Sequence(items) => format!("[{}]", items.iter().map(gvariant).collect::<Result<Vec<_>>>()?.join(", ")),
        Value::Tagged(t) => gvariant(&t.value)?,
        other => bail!("a setting is text, a number, true/false or a list, not {}", if other.is_mapping() { "a map" } else { "this" }),
    })
}

fn gsettings() -> Option<&'static str> {
    Command::new("gsettings").arg("--version").output().ok().filter(|o| o.status.success()).map(|_| "gsettings")
}

/// Make dconf match `wanted`, resetting what an earlier apply set but
/// `wanted` no longer has. Problems with single keys are reported, not fatal.
pub fn activate(home_dir: &Path, wanted: &Settings) -> Result<()> {
    let state_file = home_dir.join(".local/state/nami/settings.json");
    let previous: Settings = fs::read(&state_file).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
    if wanted.is_empty() && previous.is_empty() {
        return Ok(());
    }
    let Some(gs) = gsettings() else {
        crate::say!("settings: skipped, `gsettings` isn't available on this computer");
        return Ok(());
    };
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        crate::say!("settings: skipped, no desktop session here (run nami apply from a terminal in your session)");
        return Ok(());
    }
    let mut applied: Settings = BTreeMap::new();
    for (schema, keys) in &previous {
        for key in keys.keys() {
            if !wanted.get(schema).is_some_and(|k| k.contains_key(key)) {
                let ok = Command::new(gs).args(["reset", schema, key]).status().map(|s| s.success()).unwrap_or(false);
                if !ok {
                    crate::say!("settings: couldn't reset {schema} {key}");
                }
            }
        }
    }
    for (schema, keys) in wanted {
        for (key, value) in keys {
            if previous.get(schema).and_then(|k| k.get(key)) == Some(value) && current(gs, schema, key).as_deref() == gvariant(value).ok().as_deref() {
                applied.entry(schema.clone()).or_default().insert(key.clone(), value.clone());
                continue;
            }
            let text = match gvariant(value) {
                Ok(t) => t,
                Err(e) => {
                    crate::say!("settings: {schema} {key}: {e}");
                    continue;
                }
            };
            let out = Command::new(gs).args(["set", schema, key, &text]).output();
            match out {
                Ok(o) if o.status.success() => {
                    applied.entry(schema.clone()).or_default().insert(key.clone(), value.clone());
                }
                Ok(o) => crate::say!("settings: {schema} {key}: {}", String::from_utf8_lossy(&o.stderr).trim()),
                Err(e) => crate::say!("settings: {schema} {key}: {e}"),
            }
        }
    }
    fs::create_dir_all(state_file.parent().unwrap())?;
    crate::store::write_atomic(&state_file, &serde_json::to_vec_pretty(&applied)?)?;
    Ok(())
}

fn current(gs: &str, schema: &str, key: &str) -> Option<String> {
    let o = Command::new(gs).args(["get", schema, key]).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// What this machine's schemas say about a key, for the linter.
pub enum KeyCheck {
    /// No gsettings here: nothing to check against.
    Unknown,
    SchemaMissing,
    KeyMissing(Vec<String>),
    /// An enum, and the value isn't one of these.
    NotAllowed(Vec<String>),
    Ok,
}

pub fn check_key(schema: &str, key: &str, value: &Value) -> KeyCheck {
    let Some(gs) = gsettings() else { return KeyCheck::Unknown };
    let Ok(o) = Command::new(gs).args(["list-keys", schema]).output() else { return KeyCheck::Unknown };
    if !o.status.success() {
        return KeyCheck::SchemaMissing;
    }
    let keys: Vec<String> = String::from_utf8_lossy(&o.stdout).lines().map(String::from).collect();
    if !keys.iter().any(|k| k == key) {
        return KeyCheck::KeyMissing(keys);
    }
    // `gsettings range` prints "enum" then the allowed values.
    if let Ok(r) = Command::new(gs).args(["range", schema, key]).output() {
        let text = String::from_utf8_lossy(&r.stdout);
        let mut lines = text.lines();
        if lines.next() == Some("enum") {
            let allowed: Vec<String> = lines.map(|l| l.trim().trim_matches('\'').to_string()).collect();
            if let Value::String(s) = value {
                if !allowed.contains(s) {
                    return KeyCheck::NotAllowed(allowed);
                }
            }
        }
    }
    KeyCheck::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gvariant_text() {
        let v = |s: &str| serde_yaml::from_str::<Value>(s).unwrap();
        assert_eq!(gvariant(&v("prefer-dark")).unwrap(), "'prefer-dark'");
        assert_eq!(gvariant(&v("\"it's\"")).unwrap(), "'it\\'s'");
        assert_eq!(gvariant(&v("true")).unwrap(), "true");
        assert_eq!(gvariant(&v("1.25")).unwrap(), "1.25");
        assert_eq!(gvariant(&v("[firefox.desktop, org.gnome.Nautilus.desktop]")).unwrap(), "['firefox.desktop', 'org.gnome.Nautilus.desktop']");
        assert!(gvariant(&v("{a: 1}")).is_err());
        assert_eq!(gvariant(&v("!gvariant \"[('xkb', 'us')]\"")).unwrap(), "[('xkb', 'us')]");
    }
}
