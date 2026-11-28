//! `wifi:` in a system file: Wi-Fi networks, by name, on a computer nami
//! made.
//!
//! ```yaml
//! wifi:
//!   home: { password: !secret "YWdl..." }     # WPA2 or WPA3, with a password
//!   cafe: {}                                  # open
//!   office: { security: enterprise, identity: ada, password: !secret "YWdl..." }
//! ```
//!
//! Each is a NetworkManager connection (a keyfile), a `files:` entry in
//! /run/NetworkManager/system-connections: written as the computer starts
//! (its password opened with the computer's own key, for root alone, 0600)
//! and on a live `nami apply`, which tells NetworkManager at once (`reload:
//! nmcli connection reload`). The file's own `files:` win over these.

use anyhow::{anyhow, bail, ensure, Result};
use serde_yaml::{Mapping, Value};

/// What an entry may say.
const TAKES: &[&str] = &["autoconnect", "eap", "hidden", "identity", "password", "phase2", "security", "ssid"];
const SECURITY: &[&str] = &["wpa-psk", "sae", "enterprise", "open"];

/// `value` with its `wifi:` written out as `files:` (underneath the file's own).
pub fn expand(mut value: Value, name: &str) -> Result<Value> {
    let Some(entries) = value.as_mapping_mut().and_then(|m| m.remove("wifi")) else { return Ok(value) };
    let entries = match entries {
        Value::Null => return Ok(value),
        Value::Mapping(e) => e,
        _ => bail!("{name}: wifi: a name for each, then its settings (like `wifi: {{ home: {{ ... }} }}`)"),
    };
    let mut files = Mapping::new();
    for (entry, given) in entries {
        let entry = match entry {
            Value::String(s) => s,
            Value::Number(n) => n.to_string(),
            _ => bail!("{name}: wifi: each one has a name"),
        };
        let file = connection(&entry, given).map_err(|e| anyhow!("{name}: wifi.{entry}: {e}"))?;
        files.insert(format!("/run/NetworkManager/system-connections/{entry}.nmconnection").into(), file);
    }
    let written = Value::Mapping([("files".into(), Value::Mapping(files))].into_iter().collect());
    crate::config::merge(written, value)
}

/// One network's file: `{ content, mode, reload }`.
fn connection(name: &str, given: Value) -> Result<Value> {
    ensure!(!name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\n', '\0']), "a name can't have / in it, or start with a dot");
    let given = match given {
        Value::Null => Mapping::new(),
        Value::Mapping(g) => g,
        _ => bail!("its settings, like {{ password: ... }}"),
    };
    for k in given.keys() {
        let k = k.as_str().ok_or_else(|| anyhow!("settings have names"))?;
        if !TAKES.contains(&k) {
            let hint = crate::config::closest(k, TAKES).map(|c| format!("did you mean {c}? ")).unwrap_or_default();
            bail!("{k}: {hint}wifi takes {}", TAKES.join(", "));
        }
    }
    let get = |k: &str| given.get(k).filter(|v| !v.is_null()).cloned();
    let password = get("password");
    let security = match get("security") {
        None if password.is_some() => "wpa-psk".to_string(),
        None => "open".to_string(),
        Some(Value::String(s)) if SECURITY.contains(&s.as_str()) => s,
        Some(_) => bail!("security: one of {}", SECURITY.join(", ")),
    };
    let section = |pairs: Vec<(&str, Option<Value>)>| Value::Mapping(pairs.into_iter().filter_map(|(k, v)| Some((Value::from(k), v?))).collect());
    let mut content = Mapping::new();
    content.insert("connection".into(), section(vec![("id", Some(name.into())), ("type", Some("wifi".into())), ("autoconnect", get("autoconnect"))]));
    content.insert("wifi".into(), section(vec![("ssid", Some(get("ssid").unwrap_or_else(|| name.into()))), ("hidden", get("hidden"))]));
    match security.as_str() {
        "wpa-psk" | "sae" => {
            content.insert("wifi-security".into(), section(vec![("key-mgmt", Some(security.as_str().into())), ("psk", password)]));
        }
        "enterprise" => {
            content.insert("wifi-security".into(), section(vec![("key-mgmt", Some("wpa-eap".into()))]));
            let eap = get("eap").unwrap_or_else(|| "peap".into());
            let phase2 = get("phase2").unwrap_or_else(|| "mschapv2".into());
            content.insert("802-1x".into(), section(vec![("eap", Some(eap)), ("identity", get("identity")), ("password", password), ("phase2-auth", Some(phase2))]));
        }
        _ => {}
    }
    let mut file = Mapping::new();
    file.insert("content".into(), Value::Mapping(content));
    // NetworkManager reads only files for root alone; told of new ones live.
    file.insert("mode".into(), "0600".into());
    file.insert("reload".into(), "nmcli connection reload".into());
    Ok(Value::Mapping(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn y(s: &str) -> Value {
        serde_yaml::from_str(s).unwrap()
    }

    fn file(of: &Value, name: &str) -> Value {
        of["files"][format!("/run/NetworkManager/system-connections/{name}.nmconnection").as_str()]["content"].clone()
    }

    #[test]
    fn networks() {
        let secret = "!secret YWdlLWVuY3J5cHRpb24ub3JnL3YxCi0+IFgyNTUxOSB0cmF5LWV4YW1wbGUK";
        let mut v = y("wifi:\n  home: { password: PW }\n  cafe: {}\n  office: { security: enterprise, identity: ada, password: PW2, ssid: Corp, hidden: true }\nfiles:\n  /etc/x: hi\n");
        v["wifi"]["home"]["password"] = Value::String(secret.into());
        let out = expand(v, "t").unwrap();
        assert!(out.get("wifi").is_none());
        let home = file(&out, "home");
        assert_eq!(home["connection"], y("{ id: home, type: wifi }"));
        assert_eq!(home["wifi"], y("{ ssid: home }"));
        assert_eq!(home["wifi-security"]["key-mgmt"], y("wpa-psk"));
        // The secret, as it is: still sealed.
        assert_eq!(home["wifi-security"]["psk"].as_str(), Some(secret));
        assert!(home.get("802-1x").is_none());
        // Open: no security at all.
        let cafe = file(&out, "cafe");
        assert!(cafe.get("wifi-security").is_none() && cafe.get("802-1x").is_none());
        let office = file(&out, "office");
        assert_eq!(office["802-1x"], y("{ eap: peap, identity: ada, password: PW2, phase2-auth: mschapv2 }"));
        assert_eq!(office["wifi-security"], y("{ key-mgmt: wpa-eap }"));
        assert_eq!(office["wifi"], y("{ ssid: Corp, hidden: true }"));
        let f = &out["files"]["/run/NetworkManager/system-connections/home.nmconnection"];
        assert_eq!((f["mode"].as_str(), f["reload"].as_str()), (Some("0600"), Some("nmcli connection reload")));
        assert_eq!(out["files"]["/etc/x"], y("hi"));
        assert_eq!(crate::home::render("x.nmconnection", &file(&out, "office")).unwrap(), "[connection]\nid = office\ntype = wifi\n[wifi]\nssid = Corp\nhidden = true\n[wifi-security]\nkey-mgmt = wpa-eap\n[802-1x]\neap = peap\nidentity = ada\npassword = PW2\nphase2-auth = mschapv2\n");
    }

    #[test]
    fn mistakes_are_named() {
        let with = |s: &str| expand(y(s), "t").unwrap_err().to_string();
        let e = with("wifi: { home: { pasword: x } }");
        assert!(e.contains("t: wifi.home: pasword: did you mean password?"), "{e}");
        assert!(with("wifi: { ../x: {} }").contains("can't have /"));
        assert!(with("wifi: [a]").contains("a name for each"));
        assert!(with("wifi: { home: false }").contains("its settings"));
        assert!(with("wifi: { home: { security: wep } }").contains("security: one of"));
    }
}
