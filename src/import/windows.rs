//! Windows' disk, read from a nami USB drive (or WSL's /mnt/c): the programs
//! installed (from the registry when it can be read, else the Start menu
//! and Program Files) as their Linux apps, Wi-Fi networks, the language,
//! keyboard, time zone and dark mode, and the folders to move.
//!
//! Windows keeps Wi-Fi passwords encrypted with a key of its own (DPAPI),
//! which can't be opened outside Windows: networks come without them (the
//! new computer asks once), unless they were exported from Windows first
//! (`netsh wlan export profile key=clear folder=C:\wifi`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde_yaml::Mapping;

use super::hive::{self, Data, Hive};
use super::moving::{add_wifi, top, Apps, HomePlan, Match, Moved, Person, Wifi};

const NOT_PEOPLE: &[&str] = &["public", "default", "default user", "all users", "defaultapppool", "wdagutilityaccount"];

/// The folders that move, and where they go in the new home.
const FOLDERS: &[(&str, &str)] = &[
    ("Desktop", "Desktop"),
    ("Documents", "Documents"),
    ("Downloads", "Downloads"),
    ("Music", "Music"),
    ("Pictures", "Pictures"),
    ("Videos", "Videos"),
    ("OneDrive/Desktop", "Desktop"),
    ("OneDrive/Documents", "Documents"),
    ("OneDrive/Pictures", "Pictures"),
];

/// The people with a folder in C:\\Users.
pub fn people(root: &Path) -> Result<Vec<String>> {
    let mut people: Vec<String> = std::fs::read_dir(root.join("Users"))?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !NOT_PEOPLE.contains(&n.to_lowercase().as_str()) && !n.starts_with('.'))
        .collect();
    people.sort();
    Ok(people)
}

pub fn read(root: &Path, user: Option<&str>, apps: &Apps) -> Result<Moved> {
    let users = root.join("Users");
    let people = people(root)?;
    let who = match user {
        Some(u) => people.iter().find(|p| p.eq_ignore_ascii_case(u)).cloned().ok_or_else(|| anyhow::anyhow!("no {u} in {} (there: {})", users.display(), people.join(", ")))?,
        None => match people.as_slice() {
            [one] => one.clone(),
            [] => bail!("no one's folder in {}", users.display()),
            _ => bail!("several people use this Windows: say whose to bring with --user <name> ({})", people.join(", ")),
        },
    };
    let home = users.join(&who);
    let login = login_name(&who);
    let person = Person { login, name: Some(who.clone()).filter(|w| w.chars().any(char::is_uppercase) || w.contains(' ')), home: home.clone(), shell: None, groups: Vec::new(), admin: true };
    let folders = FOLDERS.iter().filter(|(f, _)| home.join(f).is_dir()).map(|(f, t)| (f.to_string(), t.to_string())).collect();
    let plan = HomePlan { from: home.clone(), windows: true, folders, ..Default::default() };
    let mut m = Moved::new("Windows".into(), person, plan);
    m.system.include("namios");
    m.brought.push("the desktop: namiOS's (namios, copied from nami's examples)".into());

    // The registry, when Windows isn't running (from a USB drive it can be read).
    let software = hive::read(&root.join("Windows/System32/config/SOFTWARE"));
    let system = hive::read(&root.join("Windows/System32/config/SYSTEM"));
    let ntuser = hive::read(&home.join("NTUSER.DAT"));
    if let Err(e) = &ntuser {
        m.note(format!("Windows' registry couldn't be read ({e:#}), so programs come from the Start menu and Program Files, and the language, keyboard and time zone aren't known: start the computer from the nami USB drive (with Windows shut down) for those"));
    }
    programs(root, &home, software.as_ref().ok(), ntuser.as_ref().ok(), apps, &mut m);
    if let Ok(s) = &system {
        computer_settings(s, ntuser.as_ref().ok(), &mut m);
    }
    if let Ok(u) = &ntuser {
        look(u, &m.person.login.clone(), &home, &mut m);
    }
    wifi(root, &mut m);
    m.note(format!("your files ({}) move with `nami import {} --home ~` on the new computer (from the USB drive, Windows' disk mounted)", m.home.folders.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>().join(", "), root.display()));
    Ok(m)
}

/// A Windows name as a Linux login: `Ada B` -> `adab`.
fn login_name(who: &str) -> String {
    let s: String = who.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    let s = s.trim_start_matches(|c: char| !c.is_ascii_lowercase()).to_string();
    if s.is_empty() || ["root", "nobody", "nami"].contains(&s.as_str()) {
        "me".into()
    } else {
        s.chars().take(32).collect()
    }
}

/// The installed programs, as Linux apps.
fn programs(root: &Path, home: &Path, software: Option<&Hive>, ntuser: Option<&Hive>, apps: &Apps, m: &mut Moved) {
    // (name, whether an unknown one is worth listing)
    let mut names: Vec<(String, bool)> = Vec::new();
    for (h, path) in [(software, "Microsoft\\Windows\\CurrentVersion\\Uninstall"), (software, "WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall"), (ntuser, "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall")] {
        let Some(h) = h else { continue };
        let Some(k) = h.open(path) else { continue };
        for app in h.subkeys(k) {
            let hidden = h.value(app, "SystemComponent") == Some(Data::Number(1)) || h.text(app, "ParentKeyName").is_some() || h.text(app, "ReleaseType").is_some_and(|r| r.contains("Update"));
            if let Some(n) = h.text(app, "DisplayName").filter(|_| !hidden) {
                names.push((n, true));
            }
        }
    }
    // The Start menu: everyone's and the person's.
    let entries = |dir: PathBuf| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.eq_ignore_ascii_case("desktop.ini")).collect();
        v.sort();
        v
    };
    for dir in [root.join("ProgramData/Microsoft/Windows/Start Menu/Programs"), home.join("AppData/Roaming/Microsoft/Windows/Start Menu/Programs")] {
        names.extend(entries(dir).into_iter().map(|n| (n.trim_end_matches(".lnk").trim_end_matches(".url").to_string(), true)));
    }
    names.extend(entries(home.join("AppData/Local/Programs")).into_iter().map(|n| (n, true)));
    // Program Files' folders are often the maker's name: used only when they match.
    for dir in ["Program Files", "Program Files (x86)"] {
        names.extend(entries(root.join(dir)).into_iter().map(|n| (n, false)));
    }
    let (mut brought, mut seen_pkgs, mut seen_names) = (Vec::new(), BTreeSet::new(), BTreeSet::new());
    for (name, list_unknown) in names {
        // `T3 Code (Alpha)` and `t3code` are one program.
        let key: String = name.split('(').next().unwrap_or(&name).chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        if !seen_names.insert(key) {
            continue;
        }
        match apps.windows(&name) {
            Match::Pkgs(p) => {
                let p: Vec<String> = p.into_iter().filter(|a| apps.catalog.as_ref().is_none_or(|c| c.resolve(a).is_some())).collect();
                if p.is_empty() || !seen_pkgs.insert(p.join(" ")) {
                    continue;
                }
                for a in &p {
                    m.system.package(a, &format!("Windows: {name}"));
                }
                brought.push(format!("{name} -> {}", p.join(" ")));
            }
            Match::Own => {}
            Match::No(why) => {
                if seen_pkgs.insert(why.clone()) {
                    m.system.skip(format!("Windows program {name}"), why);
                }
            }
            Match::Unknown if list_unknown => m.system.skip(format!("Windows program {name}"), "no Linux app known for it: look with `nami search`"),
            Match::Unknown => {}
        }
    }
    m.brought("Windows programs, as Linux apps", &brought);
}

/// The computer's name and time zone (SYSTEM), language and keyboard (the person's).
fn computer_settings(system: &Hive, ntuser: Option<&Hive>, m: &mut Moved) {
    let mut got = Vec::new();
    let set = system.open("Select").and_then(|k| match system.value(k, "Current") {
        Some(Data::Number(n)) => Some(format!("ControlSet{n:03}")),
        _ => None,
    });
    let cs = set.unwrap_or_else(|| "ControlSet001".into());
    if let Some(name) = system.open(&format!("{cs}\\Control\\ComputerName\\ComputerName")).and_then(|k| system.text(k, "ComputerName")) {
        let h: String = name.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        if !h.is_empty() && !h.starts_with('-') {
            top(&mut m.system, &["hostname"], h.clone().into());
            got.push(format!("hostname {h}"));
        }
    }
    let locale = ntuser.and_then(|u| u.open("Control Panel\\International").and_then(|k| u.text(k, "LocaleName")));
    if let Some(l) = &locale {
        let lang = format!("{}.UTF-8", l.replace('-', "_"));
        top(&mut m.system, &["language"], lang.clone().into());
        got.push(format!("language {lang}"));
    }
    let country = locale.as_deref().and_then(|l| l.split('-').nth(1)).unwrap_or("").to_uppercase();
    if let Some(z) = system.open(&format!("{cs}\\Control\\TimeZoneInformation")).and_then(|k| system.text(k, "TimeZoneKeyName")) {
        match timezone(&z, &country) {
            Some(tz) => {
                top(&mut m.system, &["timezone"], tz.into());
                got.push(format!("time zone {tz}"));
            }
            None => m.system.skip(format!("time zone {z}"), "not in nami's short list of Windows' zones: set timezone: (like Europe/Copenhagen)"),
        }
    }
    if let Some(klid) = ntuser.and_then(|u| u.open("Keyboard Layout\\Preload").and_then(|k| u.text(k, "1"))) {
        match keyboard(&klid) {
            Some(kb) => {
                top(&mut m.system, &["keyboard"], kb.into());
                got.push(format!("keyboard {kb}"));
            }
            None => m.system.skip(format!("keyboard layout {klid}"), "not in nami's short list of Windows' layouts: set keyboard: (like us)"),
        }
    }
    got.push(format!("you ({}, an admin)", m.person.login));
    m.brought("the computer", &got);
}

/// Dark mode and the wallpaper (if it's one of the person's pictures).
fn look(u: &Hive, login: &str, home: &Path, m: &mut Moved) {
    let mut got = Vec::new();
    if let Some(Data::Number(light)) = u.open("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").and_then(|k| u.value(k, "AppsUseLightTheme")) {
        let scheme = if light == 0 { "prefer-dark" } else { "default" };
        m.user.settings.entry("org.gnome.desktop.interface".into()).or_default().insert("color-scheme".into(), scheme.into());
        got.push(if light == 0 { "dark mode".to_string() } else { "light mode".to_string() });
    }
    if let Some(wp) = u.open("Control Panel\\Desktop").and_then(|k| u.text(k, "WallPaper")) {
        // C:\Users\<them>\Pictures\x.jpg -> ~/Pictures/x.jpg, which moves with the files.
        let who = home.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let rel = wp.replace('\\', "/");
        let prefix = format!("C:/Users/{who}/");
        if let Some(r) = rel.get(prefix.len()..).filter(|_| rel.to_lowercase().starts_with(&prefix.to_lowercase())) {
            // The longest folder that fits (OneDrive/Pictures before Pictures).
            let mut folders: Vec<&(&str, &str)> = FOLDERS.iter().collect();
            folders.sort_by_key(|(f, _)| std::cmp::Reverse(f.len()));
            if let Some((f, to)) = folders.into_iter().find(|(f, _)| r.starts_with(&format!("{f}/"))) {
                let uri = format!("file:///home/{login}/{to}/{}", &r[f.len() + 1..]);
                let bg = m.user.settings.entry("org.gnome.desktop.background".into()).or_default();
                bg.insert("picture-uri".into(), uri.clone().into());
                bg.insert("picture-uri-dark".into(), uri.into());
                got.push("your wallpaper".into());
            }
        }
    }
    m.brought("settings", &got);
}

/// Wi-Fi profiles: Windows' own (passwords encrypted), and any exported
/// with their passwords (C:\wifi, from `netsh wlan export profile key=clear`).
fn wifi(root: &Path, m: &mut Moved) {
    let mut files: Vec<PathBuf> = Vec::new();
    for e in std::fs::read_dir(root.join("ProgramData/Microsoft/Wlansvc/Profiles/Interfaces")).into_iter().flatten().flatten() {
        files.extend(std::fs::read_dir(e.path()).into_iter().flatten().flatten().map(|f| f.path()));
    }
    let mut exported: Vec<PathBuf> = std::fs::read_dir(root.join("wifi")).into_iter().flatten().flatten().map(|f| f.path()).collect();
    files.sort();
    exported.sort();
    let mut nets: Vec<(String, Mapping, bool)> = Vec::new();
    let mut unreadable = false;
    for f in files.iter().chain(&exported).filter(|f| f.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml"))) {
        let Ok(bytes) = std::fs::read(f) else {
            unreadable = true;
            continue;
        };
        let text = String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string();
        match profile(&text) {
            Ok(Some((id, settings, has_key))) => {
                // An exported copy (with its password) wins over Windows' own.
                if let Some(i) = nets.iter().position(|(n, _, _)| *n == id) {
                    if has_key && !nets[i].2 {
                        nets[i] = (id, settings, has_key);
                    }
                    continue;
                }
                nets.push((id, settings, has_key));
            }
            Ok(None) => {}
            Err((id, why)) => m.system.skip(format!("Wi-Fi network {id}"), why),
        }
    }
    if unreadable {
        m.system.skip("some Wi-Fi networks", "their files couldn't be read (they need root on the USB drive: `sudo nami import ...`)");
    }
    let (mut with, mut without) = (Vec::new(), Vec::new());
    for (id, settings, has_key) in nets {
        add_wifi(&mut m.system, &id, settings);
        if has_key { with.push(id) } else { without.push(id) }
    }
    m.brought("Wi-Fi networks, with their passwords (sealed)", &with);
    m.brought("Wi-Fi networks, without passwords (the new computer asks once)", &without);
    if !without.is_empty() {
        m.note("Windows keeps Wi-Fi passwords encrypted with its own key (DPAPI), which can't be opened outside Windows. To bring them: in Windows, in a terminal, `netsh wlan export profile key=clear folder=C:\\wifi`, then import again (and delete C:\\wifi after)");
    }
}

fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{name}>"))? + start;
    Some(xml[start..end].trim())
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// A network's name, its `wifi:` entry, and whether its password came along.
type Profile = (String, Mapping, bool);

/// A Windows Wi-Fi profile (WLANProfile XML) as a `wifi:` entry.
pub fn profile(xml: &str) -> std::result::Result<Option<Profile>, (String, String)> {
    if !xml.contains("<WLANProfile") {
        return Ok(None);
    }
    let ssid_block = tag(xml, "SSID").unwrap_or("");
    let ssid = tag(ssid_block, "name").map(unescape).or_else(|| tag(xml, "name").map(unescape)).unwrap_or_default();
    if ssid.is_empty() {
        return Ok(None);
    }
    let auth = tag(xml, "authentication").unwrap_or("open").to_string();
    let key_mgmt = match auth.as_str() {
        "open" => None,
        "WPA2PSK" | "WPAPSK" => Some("wpa-psk"),
        "WPA3SAE" => Some("sae"),
        "WPA2" | "WPA" | "WPA3ENT192" | "WPA3ENT" => return Err((ssid, "a work or school network (802.1X): set it up again in Settings".into())),
        other => return Err((ssid, format!("its security ({other}) isn't one nami can set up: set it up again in Settings"))),
    };
    let mut w = Wifi {
        ssid: Some(ssid.clone()),
        security: Some(key_mgmt.unwrap_or("open")),
        hidden: tag(xml, "nonBroadcast") == Some("true"),
        autoconnect: tag(xml, "connectionMode") != Some("manual"),
        ..Default::default()
    };
    let protected = tag(xml, "protected") != Some("false");
    // Without its password (encrypted by Windows), the new computer asks for it.
    w.password = tag(xml, "keyMaterial").map(unescape).filter(|k| key_mgmt.is_some() && !protected && !k.is_empty());
    let has_key = key_mgmt.is_none() || w.password.is_some();
    Ok(Some((ssid.clone(), w.entry(&ssid), has_key)))
}

/// Windows' time zone names (and the country, where one zone covers
/// several) as the tz database's.
fn timezone(win: &str, country: &str) -> Option<&'static str> {
    let by_country: &[(&str, &str, &str)] = &[
        ("Romance Standard Time", "DK", "Europe/Copenhagen"),
        ("Romance Standard Time", "ES", "Europe/Madrid"),
        ("Romance Standard Time", "BE", "Europe/Brussels"),
        ("W. Europe Standard Time", "NL", "Europe/Amsterdam"),
        ("W. Europe Standard Time", "SE", "Europe/Stockholm"),
        ("W. Europe Standard Time", "NO", "Europe/Oslo"),
        ("W. Europe Standard Time", "AT", "Europe/Vienna"),
        ("W. Europe Standard Time", "CH", "Europe/Zurich"),
        ("W. Europe Standard Time", "IT", "Europe/Rome"),
        ("GMT Standard Time", "IE", "Europe/Dublin"),
        ("GMT Standard Time", "PT", "Europe/Lisbon"),
        ("FLE Standard Time", "FI", "Europe/Helsinki"),
    ];
    if let Some((_, _, tz)) = by_country.iter().find(|(w, c, _)| *w == win && *c == country) {
        return Some(tz);
    }
    let zones: &[(&str, &str)] = &[
        ("UTC", "UTC"),
        ("GMT Standard Time", "Europe/London"),
        ("Greenwich Standard Time", "Atlantic/Reykjavik"),
        ("Romance Standard Time", "Europe/Paris"),
        ("W. Europe Standard Time", "Europe/Berlin"),
        ("Central Europe Standard Time", "Europe/Budapest"),
        ("Central European Standard Time", "Europe/Warsaw"),
        ("E. Europe Standard Time", "Europe/Chisinau"),
        ("FLE Standard Time", "Europe/Kiev"),
        ("GTB Standard Time", "Europe/Bucharest"),
        ("Russian Standard Time", "Europe/Moscow"),
        ("Turkey Standard Time", "Europe/Istanbul"),
        ("Israel Standard Time", "Asia/Jerusalem"),
        ("South Africa Standard Time", "Africa/Johannesburg"),
        ("Arabian Standard Time", "Asia/Dubai"),
        ("India Standard Time", "Asia/Kolkata"),
        ("China Standard Time", "Asia/Shanghai"),
        ("Singapore Standard Time", "Asia/Singapore"),
        ("Tokyo Standard Time", "Asia/Tokyo"),
        ("Korea Standard Time", "Asia/Seoul"),
        ("AUS Eastern Standard Time", "Australia/Sydney"),
        ("New Zealand Standard Time", "Pacific/Auckland"),
        ("Eastern Standard Time", "America/New_York"),
        ("Central Standard Time", "America/Chicago"),
        ("Mountain Standard Time", "America/Denver"),
        ("US Mountain Standard Time", "America/Phoenix"),
        ("Pacific Standard Time", "America/Los_Angeles"),
        ("Alaskan Standard Time", "America/Anchorage"),
        ("Hawaiian Standard Time", "Pacific/Honolulu"),
        ("Atlantic Standard Time", "America/Halifax"),
        ("E. South America Standard Time", "America/Sao_Paulo"),
        ("Argentina Standard Time", "America/Buenos_Aires"),
        ("Central Standard Time (Mexico)", "America/Mexico_City"),
    ];
    zones.iter().find(|(w, _)| *w == win).map(|(_, tz)| *tz)
}

/// Windows' keyboard layout IDs as xkb's.
fn keyboard(klid: &str) -> Option<&'static str> {
    let table: &[(&str, &str)] = &[
        ("00000409", "us"),
        ("00020409", "us(intl)"),
        ("00000809", "gb"),
        ("00000406", "dk"),
        ("00000407", "de"),
        ("0000040c", "fr"),
        ("00000410", "it"),
        ("0000040a", "es"),
        ("0000080a", "latam"),
        ("00000414", "no"),
        ("0000041d", "se"),
        ("0000040b", "fi"),
        ("00000413", "nl"),
        ("00000813", "be"),
        ("00000816", "pt"),
        ("00000416", "br"),
        ("00000415", "pl"),
        ("00000405", "cz"),
        ("0000041b", "sk"),
        ("0000040e", "hu"),
        ("00000419", "ru"),
        ("00000422", "ua"),
        ("0000041f", "tr"),
        ("00000408", "gr"),
        ("00000411", "jp"),
        ("00000412", "kr"),
        ("00000807", "ch"),
        ("0000100c", "ch(fr)"),
        ("00000c07", "at"),
        ("0000040f", "is"),
        ("00001009", "ca"),
    ];
    let k = klid.to_lowercase();
    table.iter().find(|(id, _)| *id == k).map(|(_, x)| *x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_profiles() {
        let fixture = |n: &str| std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/import/computer/windows/root/ProgramData/Microsoft/Wlansvc/Profiles/Interfaces/{6B18318B-0000-0000-0000-000000000000}").join(n)).unwrap();
        let (id, s, key) = profile(&fixture("{1}.xml")).unwrap().unwrap();
        assert_eq!(id, "Home & Garden");
        assert!(!key, "Windows' own copy: the password is encrypted");
        let y = serde_yaml::to_string(&s).unwrap();
        assert_eq!(y, "security: sae\n", "Windows' own copy: no password");
        assert!(profile(&fixture("{2}.xml")).is_err(), "a work network");
        let open = profile(&fixture("{3}.xml")).unwrap().unwrap();
        assert!(open.2 && serde_yaml::to_string(&open.1).unwrap() == "security: open\n");
    }

    #[test]
    fn names() {
        assert_eq!(login_name("Ada B"), "adab");
        assert_eq!(login_name("123"), "me");
        assert_eq!(timezone("Romance Standard Time", "DK"), Some("Europe/Copenhagen"));
        assert_eq!(timezone("Romance Standard Time", "FR"), Some("Europe/Paris"));
        assert_eq!(keyboard("00000406"), Some("dk"));
    }

    #[test]
    fn a_windows_disk() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/import/computer/windows/root");
        let names = ["google-chrome", "vscode", "spotify", "steam", "tailscale", "firefox"];
        let apps = Apps::with_names(&names);
        let m = read(&root, None, &apps).unwrap();
        let mut pk: Vec<&str> = m.system.packages.iter().map(|(a, _)| a.as_str()).collect();
        pk.sort();
        assert_eq!(pk, ["google-chrome", "spotify", "steam", "tailscale", "vscode"], "{pk:?}");
        assert_eq!(m.person.login, "alice");
        let skipped: Vec<&str> = m.system.unconverted.iter().map(|u| u.what.as_str()).collect();
        assert!(skipped.iter().any(|s| s.contains("Rhino")), "{skipped:?}");
        assert!(skipped.iter().any(|s| s.contains("Frobnicator")), "{skipped:?}");
        assert!(!skipped.iter().any(|s| s.contains("Google") && !s.contains("Chrome")), "maker folders in Program Files aren't listed: {skipped:?}");
        // The exported copy (C:\wifi) has the password.
        let home = &m.system.top.iter().find(|(p, _)| p.join(".") == "wifi.Home & Garden").unwrap().1;
        assert_eq!(serde_yaml::to_string(home).unwrap(), "security: sae\npassword: !secret garden-pass-123\n");
        assert!(m.home.folders.iter().any(|(f, _)| f == "Documents") && m.home.folders.iter().any(|(f, t)| f == "OneDrive/Pictures" && t == "Pictures"));
    }

    #[test]
    fn registry() {
        use super::hive::build::{hive, K, V};
        let soft = hive(&K {
            name: "ROOT",
            values: vec![],
            keys: vec![K {
                name: "Microsoft",
                values: vec![],
                keys: vec![K {
                    name: "Windows",
                    values: vec![],
                    keys: vec![K {
                        name: "CurrentVersion",
                        values: vec![],
                        keys: vec![K {
                            name: "Uninstall",
                            values: vec![],
                            keys: vec![
                                K { name: "Steam", values: vec![("DisplayName", V::Text("Steam"))], keys: vec![] },
                                K { name: "{x}", values: vec![("DisplayName", V::Text("Microsoft Visual C++ 2022 Redistributable")), ("SystemComponent", V::Number(1))], keys: vec![] },
                                K { name: "Blender", values: vec![("DisplayName", V::Text("blender"))], keys: vec![] },
                            ],
                        }],
                    }],
                }],
            }],
        });
        let soft = Hive::parse(soft).unwrap();
        let names = ["steam", "blender"];
        let apps = Apps::with_names(&names);
        let mut m = Moved::new("Windows".into(), Person { login: "a".into(), name: None, home: "/nonexistent".into(), shell: None, groups: vec![], admin: true }, HomePlan::default());
        programs(Path::new("/nonexistent"), Path::new("/nonexistent"), Some(&soft), None, &apps, &mut m);
        let pk: Vec<&str> = m.system.packages.iter().map(|(a, _)| a.as_str()).collect();
        assert_eq!(pk, ["steam", "blender"]);
    }
}
