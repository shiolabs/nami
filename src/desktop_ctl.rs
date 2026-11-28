//! `nami desktop`: everything Control Center does, from the command line, for scripts and
//! agents. It asks the running namiOS desktop over its socket ($XDG_RUNTIME_DIR/nami-desktop.sock,
//! the person's alone), which does it the way its own switches do:
//! what's kept (dark or light, Night Shift, Do Not Disturb, the wallpaper) is written to your
//! user.nami as Control Center and Settings write it.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};

use crate::errors::{Code, coded, coded_hint};

#[derive(Args)]
pub struct DesktopArgs {
    #[command(subcommand)]
    pub command: DesktopCommand,
    /// Print JSON (errors too: {"error": {"code", "message", "hint"}})
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum OnOff {
    On,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Toggle {
    On,
    Off,
    Toggle,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Look {
    Light,
    Dark,
    /// Light by day, dark at night
    Auto,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Night {
    Off,
    /// On all the time (same as always)
    On,
    Always,
    /// From sunset to sunrise
    Sunset,
}

#[derive(Subcommand)]
pub enum DesktopCommand {
    /// Everything at once: sound, screens, network, Bluetooth, Do Not Disturb, look, Night Shift, battery, windows
    Status,
    /// The sound's volume: show it, or set it (40, +5, -5)
    #[command(allow_negative_numbers = true)]
    Volume {
        /// A percentage (0-100), or a step (+5, -10)
        level: Option<String>,
    },
    /// Mute the sound (on), unmute it (off), or switch (toggle; the default)
    Mute { state: Option<Toggle> },
    /// The sound outputs, or use one (by its id or name)
    Output { device: Option<String> },
    /// Each screen's brightness, or set it (1-100)
    Brightness {
        percent: Option<u32>,
        /// Only this screen (its name, as listed)
        #[arg(long)]
        screen: Option<String>,
    },
    /// Wi-Fi: whether it's on and to what; on, off, list, connect
    Wifi {
        #[command(subcommand)]
        command: Option<WifiCommand>,
    },
    /// Bluetooth: whether it's on, and the devices paired; on, off, list
    Bluetooth {
        #[command(subcommand)]
        command: Option<BluetoothCommand>,
    },
    /// Do Not Disturb: show it, turn it on (until turned off, --for a while, or --until a time), or off
    #[command(name = "dnd", alias = "do-not-disturb")]
    Dnd {
        state: Option<OnOff>,
        /// For this long: 30m, 2h (as nami writes durations)
        #[arg(long = "for", value_name = "DURATION", conflicts_with = "until")]
        duration: Option<String>,
        /// Until this evening, tomorrow (morning), or a time: "2026-11-19 18:00"
        #[arg(long)]
        until: Option<String>,
    },
    /// Light or dark (or auto: by the sun), as Control Center's Dark switch sets it; kept in your user.nami
    Look { look: Option<Look> },
    /// Night Shift: off, on (always) or sunset (sunset to sunrise); kept in your user.nami
    Night {
        when: Option<Night>,
        /// How warm, in kelvin (2500 warmest, 5000 least)
        #[arg(long)]
        warmth: Option<u32>,
    },
    /// The wallpaper: show it, or set a picture (kept in your user.nami)
    Wallpaper { picture: Option<PathBuf> },
    /// Open Settings, at a page (appearance, network, bluetooth, sound, displays, notifications, ...; none: its first)
    Settings { page: Option<String> },
    /// A screenshot of the screen (or one window), written to a PNG file, quietly
    Screenshot {
        file: PathBuf,
        /// Only this window (its id, app or title, as `nami desktop windows` lists them)
        #[arg(long)]
        window: Option<String>,
        /// With the pointer in it
        #[arg(long)]
        pointer: bool,
    },
    /// The open windows: id, app, title, workspace, which is in use
    Windows,
    /// Bring a window to the front (its id, app or title)
    Focus { window: String },
    /// Close a window (its id, app or title), as its close button does
    Close { window: String },
    /// Show a notification
    Notify {
        title: String,
        body: Option<String>,
        /// Who it's from (default: Terminal)
        #[arg(long)]
        app: Option<String>,
    },
    /// Lock the screen
    Lock,
}

#[derive(Subcommand)]
pub enum WifiCommand {
    On,
    Off,
    /// The networks in range (looks again first)
    List,
    /// Join a network; a password comes on stdin (--password-stdin), never on the command line
    Connect {
        network: String,
        /// Read the password from stdin (one line)
        #[arg(long)]
        password_stdin: bool,
    },
}

#[derive(Subcommand)]
pub enum BluetoothCommand {
    On,
    Off,
    /// The devices paired, and which are connected
    List,
}

/// Settings' pages, as `nami desktop settings <page>` takes them (`nami-gui`'s names).
pub const PAGES: &[&str] = &[
    "appearance", "network", "bluetooth", "dock", "menubar", "notifications", "sound", "power", "privacy", "displays", "keyboard", "mouse", "shortcuts", "accessibility",
    "startup", "default-apps", "printers", "widgets", "plugins", "datetime", "storage", "sharing", "general", "terminal", "about", "updates", "backups", "people", "history",
    "apps", "installed", "sync", "projects",
];

fn socket() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })));
    dir.join("nami-desktop.sock")
}

/// One request to the desktop, and what it answered (its `ok`).
pub fn ask(request: Value, wait: Duration) -> Result<Value> {
    let path = socket();
    let not_running = || coded_hint(Code::NotRunning, "The namiOS desktop isn't running in this session.", "nami desktop works in a namiOS session, as the person logged in (with XDG_RUNTIME_DIR set).");
    let mut stream = UnixStream::connect(&path).map_err(|_| not_running())?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(wait))?;
    stream.write_all(format!("{request}\n").as_bytes())?;
    let mut line = String::new();
    match BufReader::new(stream).read_line(&mut line) {
        Ok(0) => return Err(coded_hint(Code::NotRunning, "The desktop didn't answer: it may be older than this nami.", "Log out and in again (or restart) after updating, so the desktop is new too.")),
        Ok(_) => {}
        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Err(coded(Code::Failed, "The desktop didn't answer in time.")),
        Err(e) => return Err(e.into()),
    }
    let answer: Value = serde_json::from_str(line.trim()).map_err(|_| coded(Code::Failed, "The desktop's answer wasn't JSON."))?;
    if let Some(e) = answer.get("error") {
        let code = e["code"].as_str().and_then(Code::of_str).unwrap_or(Code::Failed);
        let message = e["message"].as_str().unwrap_or("The desktop couldn't do it.").to_string();
        return Err(match e["hint"].as_str() {
            Some(h) => coded_hint(code, message, h),
            None => coded(code, message),
        });
    }
    Ok(answer.get("ok").cloned().unwrap_or(Value::Null))
}

const QUICK: Duration = Duration::from_secs(20);

/// `+5`, `-5`, `40`, `40%`: a level from `now`, 0 to 100.
pub fn level_of(said: &str, now: u32) -> Result<u32> {
    let t = said.trim().trim_end_matches('%');
    let bad = || coded(Code::InvalidArgs, format!("\"{said}\" isn't a volume: a percentage (40) or a step (+5, -5)"));
    let n = |s: &str| s.parse::<i64>().map_err(|_| bad());
    let v = if let Some(up) = t.strip_prefix('+') { now as i64 + n(up)? } else if t.starts_with('-') { now as i64 + n(t)? } else { n(t)? };
    Ok(v.clamp(0, 100) as u32)
}

/// What `--until` says, as the desktop takes it: evening, tomorrow, or a time.
fn until_of(said: &str) -> Result<String> {
    let t = said.trim().to_lowercase();
    Ok(match t.as_str() {
        "evening" | "this evening" | "tonight" => "evening".into(),
        "tomorrow" | "morning" | "tomorrow morning" => "tomorrow".into(),
        _ if t.len() >= 16 && t.as_bytes().get(4) == Some(&b'-') => said.trim().to_string(),
        _ => return Err(coded(Code::InvalidArgs, format!("\"{said}\" isn't a time: evening, tomorrow, or like \"2026-11-19 18:00\""))),
    })
}

/// The request for a command, and how long it may take.
pub fn request_of(command: &DesktopCommand) -> Result<(Value, Duration)> {
    Ok(match command {
        DesktopCommand::Status => (json!({ "do": "status" }), Duration::from_secs(30)),
        DesktopCommand::Volume { level: None } => (json!({ "do": "volume" }), QUICK),
        DesktopCommand::Volume { level: Some(l) } => {
            let now = if l.starts_with(['+', '-']) { ask(json!({ "do": "volume" }), QUICK)?["volume"].as_u64().unwrap_or(0) as u32 } else { 0 };
            (json!({ "do": "volume", "percent": level_of(l, now)? }), QUICK)
        }
        DesktopCommand::Mute { state } => {
            let muted = match state.unwrap_or(Toggle::Toggle) {
                Toggle::On => json!(true),
                Toggle::Off => json!(false),
                Toggle::Toggle => json!("toggle"),
            };
            (json!({ "do": "volume", "muted": muted }), QUICK)
        }
        DesktopCommand::Output { device } => (json!({ "do": "output", "device": device }), QUICK),
        DesktopCommand::Brightness { percent, screen } => {
            if let Some(p) = percent {
                anyhow::ensure!((1..=100).contains(p), coded(Code::InvalidArgs, format!("{p} isn't a brightness: 1 to 100")));
            }
            (json!({ "do": "brightness", "percent": percent, "screen": screen }), Duration::from_secs(30))
        }
        DesktopCommand::Wifi { command } => match command {
            None => (json!({ "do": "wifi" }), QUICK),
            Some(WifiCommand::On) => (json!({ "do": "wifi", "on": true }), Duration::from_secs(30)),
            Some(WifiCommand::Off) => (json!({ "do": "wifi", "on": false }), Duration::from_secs(30)),
            Some(WifiCommand::List) => (json!({ "do": "wifi", "list": true }), Duration::from_secs(30)),
            Some(WifiCommand::Connect { network, password_stdin }) => {
                let password = if *password_stdin {
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    Some(line.trim_end_matches(['\n', '\r']).to_string())
                } else {
                    None
                };
                (json!({ "do": "wifi-connect", "network": network, "password": password }), Duration::from_secs(150))
            }
        },
        DesktopCommand::Bluetooth { command } => match command {
            None | Some(BluetoothCommand::List) => (json!({ "do": "bluetooth" }), Duration::from_secs(30)),
            Some(BluetoothCommand::On) => (json!({ "do": "bluetooth", "on": true }), Duration::from_secs(30)),
            Some(BluetoothCommand::Off) => (json!({ "do": "bluetooth", "on": false }), Duration::from_secs(30)),
        },
        DesktopCommand::Dnd { state, duration, until } => {
            let mut r = json!({ "do": "dnd" });
            let on = match state {
                Some(OnOff::On) => true,
                Some(OnOff::Off) => false,
                // (--for or --until alone: on.)
                None => duration.is_some() || until.is_some(),
            };
            if state.is_some() || on {
                r["on"] = json!(on);
            }
            if let Some(d) = duration {
                let seconds = crate::duration::seconds(d).map_err(|e| coded(Code::InvalidArgs, e))?;
                r["minutes"] = json!(seconds.div_ceil(60).max(1));
            }
            if let Some(u) = until {
                r["until"] = json!(until_of(u)?);
            }
            (r, QUICK)
        }
        DesktopCommand::Look { look } => (
            json!({ "do": "look", "look": look.map(|l| match l { Look::Light => "light", Look::Dark => "dark", Look::Auto => "auto" }) }),
            QUICK,
        ),
        DesktopCommand::Night { when, warmth } => (
            json!({ "do": "night", "when": when.map(|w| match w { Night::Off => "off", Night::On | Night::Always => "always", Night::Sunset => "sunset" }), "warmth": warmth }),
            QUICK,
        ),
        DesktopCommand::Wallpaper { picture } => {
            let path = picture.as_ref().map(|p| std::path::absolute(p)).transpose()?;
            if let Some(p) = &path {
                anyhow::ensure!(p.is_file(), coded(Code::NotFound, format!("No picture at {}.", p.display())));
            }
            (json!({ "do": "wallpaper", "path": path }), QUICK)
        }
        DesktopCommand::Settings { page } => {
            if let Some(p) = page {
                anyhow::ensure!(PAGES.contains(&p.as_str()), coded_hint(Code::NotFound, format!("Settings has no page called \"{p}\"."), format!("Pages: {}.", PAGES.join(", "))));
            }
            (json!({ "do": "settings", "page": page.clone().unwrap_or_default() }), QUICK)
        }
        DesktopCommand::Screenshot { file, window, pointer } => {
            let path = std::path::absolute(file)?;
            if let Some(dir) = path.parent() {
                anyhow::ensure!(dir.is_dir(), coded(Code::NotFound, format!("No folder {} to write the screenshot in.", dir.display())));
            }
            let window = match window {
                Some(w) => Some(find_window_id(w)?),
                None => None,
            };
            (json!({ "do": "screenshot", "path": path, "window": window, "pointer": pointer }), Duration::from_secs(30))
        }
        DesktopCommand::Windows => (json!({ "do": "windows" }), QUICK),
        DesktopCommand::Focus { window } => (json!({ "do": "window-focus", "window": window }), QUICK),
        DesktopCommand::Close { window } => (json!({ "do": "window-close", "window": window }), QUICK),
        DesktopCommand::Notify { title, body, app } => (json!({ "do": "notify", "title": title, "body": body, "app": app }), QUICK),
        DesktopCommand::Lock => (json!({ "do": "lock" }), QUICK),
    })
}

/// A window's id, from the desktop's list: by id, app or title.
fn find_window_id(wanted: &str) -> Result<u64> {
    let windows = ask(json!({ "do": "windows" }), QUICK)?;
    let list = windows.as_array().cloned().unwrap_or_default();
    let lower = wanted.to_lowercase();
    let text = |w: &Value, k: &str| w[k].as_str().unwrap_or_default().to_lowercase();
    list.iter()
        .find(|w| w["id"].as_u64().is_some_and(|id| id.to_string() == wanted))
        .or_else(|| list.iter().find(|w| text(w, "app_id") == lower))
        .or_else(|| list.iter().find(|w| text(w, "app_id").contains(&lower) || text(w, "title").contains(&lower)))
        .and_then(|w| w["id"].as_u64())
        .ok_or_else(|| coded_hint(Code::NotFound, format!("No window matches \"{wanted}\"."), "`nami desktop windows` lists them."))
}

pub fn cli(args: DesktopArgs) -> Result<()> {
    let (request, wait) = request_of(&args.command)?;
    let answer = ask(request, wait)?;
    if args.json {
        println!("{answer}");
    } else {
        let paint = crate::style::out();
        for (i, line) in words(&args.command, &answer).iter().enumerate() {
            println!("{}", painted(paint, line, i == 0 && matches!(args.command, DesktopCommand::Status)));
        }
    }
    Ok(())
}

fn on_off(on: bool) -> &'static str {
    if on { "On" } else { "Off" }
}

/// What an answer says, in plain lines (coloured by `style::Paint::line`): a
/// change as one ✓ line (`✓ Volume is 40%`), what's asked as `Label  value`
/// lines, `nami desktop status` as a card of them.
pub fn words(command: &DesktopCommand, a: &Value) -> Vec<String> {
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let kept = "kept in user.nami";
    let level = |a: &Value| format!("{}%{}", a["volume"], if a["muted"] == true { " · muted" } else { "" });
    let wifi = |a: &Value| match (a["on"] == true, a["network"].as_str()) {
        (_, _) if a["available"] == false => "none on this computer".to_string(),
        (false, _) => "Off".to_string(),
        (true, Some(n)) => format!("On · {n}"),
        (true, None) => "On · not connected".to_string(),
    };
    let bluetooth = |a: &Value| if a["available"] == false { "none on this computer".to_string() } else { on_off(a["on"] == true).to_string() };
    let devices = |a: &Value| {
        a["devices"].as_array().into_iter().flatten().map(|d| format!("  {}  {}{}", s(&d["name"]), s(&d["id"]), if d["connected"] == true { " · connected" } else { "" })).collect::<Vec<_>>()
    };
    let dnd = |a: &Value| match (a["on"] == true, a["until"].as_str()) {
        (false, _) => "Off".to_string(),
        (true, Some(u)) => format!("On until {u}{}", if a["sleep"] == true { " · Sleep" } else { "" }),
        (true, None) => "On".to_string(),
    };
    let look = |a: &Value| {
        let l = s(&a["look"]);
        let mut c = l.chars();
        let l = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
        if a["look"] == "auto" { format!("{l} · {} now", if a["dark"] == true { "dark" } else { "light" }) } else { l }
    };
    let night = |a: &Value| format!("{} · {}K", match a["when"].as_str() { Some("always") => "On", Some("sunset") => "Sunset to Sunrise", _ => "Off" }, a["warmth"]);
    let screens = |a: &Value| a.as_array().into_iter().flatten().map(|l| (format!("{}%", l["percent"]), format!("{} · {}", s(&l["name"]), s(&l["screen"])))).collect::<Vec<_>>();
    // `Label  value`, the labels in a column.
    let card = |rows: Vec<(String, String)>| {
        let w = rows.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0);
        rows.into_iter().map(|(l, v)| format!("  {l}{}  {v}", " ".repeat(w - l.chars().count()))).collect::<Vec<_>>()
    };
    let one = |label: &str, value: String| vec![format!("{label}  {value}")];
    let windows = |a: &Value| {
        let list = a.as_array().cloned().unwrap_or_default();
        if list.is_empty() {
            return vec!["No windows".to_string()];
        }
        let w = list.iter().map(|w| s(&w["app_id"]).chars().count()).max().unwrap_or(0).min(28);
        list.iter().map(|x| format!("{} {:>4}  {:<w$}  {}", if x["focused"] == true { "●" } else { " " }, x["id"], s(&x["app_id"]), s(&x["title"]))).collect()
    };
    match command {
        DesktopCommand::Status => {
            let mut rows = vec![("Sound".to_string(), level(&a["sound"]) + &a["sound"]["output"]["name"].as_str().map(|n| format!(" · {n}")).unwrap_or_default())];
            rows.extend(screens(&a["brightness"]).into_iter().map(|(p, on)| ("Brightness".to_string(), format!("{p} · {on}"))));
            if let Some(w) = a["network"]["wired"].as_str() {
                rows.push(("Ethernet".into(), w.to_string()));
            }
            rows.push(("Wi-Fi".into(), wifi(&a["wifi"])));
            rows.push(("Bluetooth".into(), bluetooth(&a["bluetooth"])));
            rows.push(("Do Not Disturb".into(), dnd(&a["do_not_disturb"])));
            rows.push(("Look".into(), look(&a["appearance"])));
            rows.push(("Night Shift".into(), night(&a["night_shift"])));
            if let Some(b) = a["battery"].as_object() {
                rows.push(("Battery".into(), format!("{}% · {}", b.get("percent").cloned().unwrap_or_default(), b.get("status").and_then(Value::as_str).unwrap_or_default())));
            }
            if a["locked"] == true {
                rows.push(("Screen".into(), "Locked".into()));
            }
            rows.push(("Windows".into(), a["windows"].as_array().map_or(0, Vec::len).to_string()));
            let mut out = vec!["This desktop".to_string()];
            out.extend(card(rows));
            out
        }
        DesktopCommand::Volume { level: Some(_) } => vec![format!("✓ Volume is {}", level(a))],
        DesktopCommand::Volume { level: None } => one("Volume", level(a)),
        DesktopCommand::Mute { .. } => vec![if a["muted"] == true { "✓ Sound is muted".to_string() } else { format!("✓ Sound is on · {}%", a["volume"]) }],
        DesktopCommand::Output { device } => {
            let outputs: Vec<&Value> = a["outputs"].as_array().into_iter().flatten().collect();
            let now = outputs.iter().find(|d| d["default"] == true).map(|d| s(&d["name"])).unwrap_or_default();
            if device.is_some() {
                return vec![format!("✓ Sound plays on {now}")];
            }
            let mut out = one("Volume", level(a));
            out.extend(outputs.iter().map(|d| format!("{} {}  {}", if d["default"] == true { "●" } else { " " }, s(&d["name"]), s(&d["id"]))));
            out
        }
        DesktopCommand::Brightness { percent: Some(_), .. } => screens(a).into_iter().map(|(p, on)| format!("✓ Brightness is {p} · {on}")).collect(),
        DesktopCommand::Brightness { .. } => screens(a).into_iter().map(|(p, on)| format!("Brightness  {p} · {on}")).collect(),
        DesktopCommand::Wifi { command: Some(WifiCommand::List) } => {
            let mut out = one("Wi-Fi", wifi(a));
            for n in a["networks"].as_array().into_iter().flatten() {
                let mut about = Vec::new();
                if n["secure"] == true {
                    about.push("password");
                }
                if n["known"] == true {
                    about.push("known");
                }
                let about = if about.is_empty() { String::new() } else { format!(" · {}", about.join(" · ")) };
                out.push(format!("{} {:>3}%  {}{about}", if n["connected"] == true { "●" } else { " " }, n["signal"], s(&n["name"])));
            }
            out
        }
        DesktopCommand::Wifi { command: Some(_) } => vec![match (a["on"] == true, a["network"].as_str()) {
            (true, Some(n)) => format!("✓ Wi-Fi is on · connected to {n}"),
            (true, None) => "✓ Wi-Fi is on · not connected".to_string(),
            (false, _) => "✓ Wi-Fi is off".to_string(),
        }],
        DesktopCommand::Wifi { command: None } => one("Wi-Fi", wifi(a)),
        DesktopCommand::Bluetooth { command: Some(BluetoothCommand::On | BluetoothCommand::Off) } => vec![format!("✓ Bluetooth is {}", bluetooth(a).to_lowercase())],
        DesktopCommand::Bluetooth { .. } => {
            let mut out = one("Bluetooth", bluetooth(a));
            out.extend(devices(a));
            out
        }
        DesktopCommand::Dnd { state: None, duration: None, until: None } => one("Do Not Disturb", dnd(a)),
        DesktopCommand::Dnd { .. } => vec![format!("✓ Do Not Disturb is {}", lower_first(&dnd(a)))],
        DesktopCommand::Look { look: Some(_) } => vec![format!("✓ {} · {kept}", look(a))],
        DesktopCommand::Look { look: None } => one("Look", look(a)),
        DesktopCommand::Night { when: None, warmth: None } => one("Night Shift", night(a)),
        DesktopCommand::Night { .. } => vec![format!("✓ Night Shift is {} · {kept}", lower_first(&night(a)))],
        DesktopCommand::Wallpaper { picture: Some(_) } => vec![format!("✓ The wallpaper is {} · {kept}", s(&a["wallpaper"]))],
        DesktopCommand::Wallpaper { picture: None } => one("Wallpaper", s(&a["wallpaper"])),
        DesktopCommand::Settings { .. } => vec![format!("✓ Opened Settings{}", a["page"].as_str().filter(|p| !p.is_empty()).map(|p| format!(" · {p}")).unwrap_or_default())],
        DesktopCommand::Screenshot { .. } => vec![format!("✓ Saved {}", s(&a["path"]))],
        DesktopCommand::Windows => windows(a),
        DesktopCommand::Focus { .. } => vec![format!("✓ {} is in front · window {}", s(&a["app_id"]), a["id"])],
        DesktopCommand::Close { .. } => vec![format!("✓ Closed {} · window {}", s(&a["app_id"]), a["id"])],
        DesktopCommand::Notify { .. } => vec![format!("✓ Shown · {}", s(&a["title"]))],
        DesktopCommand::Lock => vec!["✓ Locked".into()],
    }
}

/// A line of `words`, coloured: a ✓ line as nami says one, the card's title
/// bold, a `Label  value` line's label dim.
fn painted(p: crate::style::Paint, line: &str, title: bool) -> String {
    if title {
        return p.bold(line);
    }
    if line.starts_with('✓') {
        return p.line(line);
    }
    let body = line.trim_start();
    let indent = &line[..line.len() - body.len()];
    match body.find("  ") {
        Some(i) if !body.starts_with('●') && !body.starts_with(|c: char| c.is_ascii_digit()) => {
            let v = i + body[i..].len() - body[i..].trim_start().len();
            format!("{indent}{}{}", p.dim(&body[..v]), &body[v..])
        }
        _ => line.to_string(),
    }
}

/// `On until 18:00` -> `on until 18:00` (after "is").
fn lower_first(t: &str) -> String {
    match t.split_once(' ') {
        Some((w, rest)) if w == "On" || w == "Off" => format!("{} {rest}", w.to_lowercase()),
        _ if t == "On" || t == "Off" => t.to_lowercase(),
        _ => t.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_levels_and_steps() {
        assert_eq!(level_of("40", 10).unwrap(), 40);
        assert_eq!(level_of("40%", 10).unwrap(), 40);
        assert_eq!(level_of("+5", 10).unwrap(), 15);
        assert_eq!(level_of("-15", 10).unwrap(), 0);
        assert_eq!(level_of("250", 10).unwrap(), 100);
        let e = level_of("loud", 10).unwrap_err();
        assert_eq!(crate::errors::classify(&e).0, Code::InvalidArgs);
    }

    #[test]
    fn requests_say_what_the_desktop_does() {
        let (r, _) = request_of(&DesktopCommand::Mute { state: None }).unwrap();
        assert_eq!(r, json!({ "do": "volume", "muted": "toggle" }));
        let (r, _) = request_of(&DesktopCommand::Dnd { state: None, duration: Some("90m".into()), until: None }).unwrap();
        assert_eq!(r, json!({ "do": "dnd", "on": true, "minutes": 90 }));
        let (r, _) = request_of(&DesktopCommand::Dnd { state: Some(OnOff::Off), duration: None, until: None }).unwrap();
        assert_eq!(r, json!({ "do": "dnd", "on": false }));
        let (r, _) = request_of(&DesktopCommand::Dnd { state: None, duration: None, until: None }).unwrap();
        assert_eq!(r, json!({ "do": "dnd" }));
        let (r, _) = request_of(&DesktopCommand::Dnd { state: Some(OnOff::On), duration: None, until: Some("tomorrow".into()) }).unwrap();
        assert_eq!(r, json!({ "do": "dnd", "on": true, "until": "tomorrow" }));
        let (r, _) = request_of(&DesktopCommand::Night { when: Some(Night::On), warmth: None }).unwrap();
        assert_eq!(r, json!({ "do": "night", "when": "always", "warmth": null }));
        let (r, _) = request_of(&DesktopCommand::Look { look: Some(Look::Dark) }).unwrap();
        assert_eq!(r, json!({ "do": "look", "look": "dark" }));
        let (r, _) = request_of(&DesktopCommand::Settings { page: Some("sound".into()) }).unwrap();
        assert_eq!(r, json!({ "do": "settings", "page": "sound" }));
    }

    #[test]
    fn wrong_values_are_invalid_args_or_not_found() {
        let code = |c: DesktopCommand| crate::errors::classify(&request_of(&c).unwrap_err()).0;
        assert_eq!(code(DesktopCommand::Settings { page: Some("nowhere".into()) }), Code::NotFound);
        assert_eq!(code(DesktopCommand::Brightness { percent: Some(0), screen: None }), Code::InvalidArgs);
        assert_eq!(code(DesktopCommand::Dnd { state: None, duration: Some("soon".into()), until: None }), Code::InvalidArgs);
        assert_eq!(code(DesktopCommand::Dnd { state: None, duration: None, until: Some("later".into()) }), Code::InvalidArgs);
        assert_eq!(code(DesktopCommand::Wallpaper { picture: Some("/nowhere/at/all.png".into()) }), Code::NotFound);
    }

    #[test]
    fn the_desktops_answers_and_errors() {
        use std::os::unix::net::UnixListener;
        // (A runtime folder of its own: first with no desktop in it, then a stand-in.)
        let dir = std::env::temp_dir().join(format!("nami-desktop-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: only this test reads it.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &dir) };
        let e = ask(json!({ "do": "status" }), QUICK).unwrap_err();
        assert_eq!(crate::errors::classify(&e).0, Code::NotRunning);
        let listener = UnixListener::bind(dir.join("nami-desktop.sock")).unwrap();
        let desktop = std::thread::spawn(move || {
            for answer in [&b"{\"ok\":{\"volume\":40,\"muted\":false}}\n"[..], b"{\"error\":{\"code\":\"not_found\",\"message\":\"No window matches \\\"x\\\".\"}}\n"] {
                let (mut s, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
                assert!(line.starts_with("{\"do\":"), "{line}");
                s.write_all(answer).unwrap();
            }
        });
        assert_eq!(ask(json!({ "do": "volume" }), QUICK).unwrap(), json!({ "volume": 40, "muted": false }));
        let e = ask(json!({ "do": "window-close", "window": "x" }), QUICK).unwrap_err();
        assert_eq!(crate::errors::classify(&e).0, Code::NotFound);
        assert_eq!(crate::errors::json_of(&e)["error"]["message"], "No window matches \"x\".");
        desktop.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn words_for_answers() {
        assert_eq!(words(&DesktopCommand::Volume { level: None }, &json!({ "volume": 40, "muted": true })), ["Volume  40% · muted"]);
        assert_eq!(words(&DesktopCommand::Volume { level: Some("40".into()) }, &json!({ "volume": 40, "muted": false })), ["✓ Volume is 40%"]);
        assert_eq!(words(&DesktopCommand::Dnd { state: None, duration: None, until: None }, &json!({ "on": true, "until": "2026-11-19 18:00", "sleep": false })), ["Do Not Disturb  On until 2026-11-19 18:00"]);
        assert_eq!(words(&DesktopCommand::Dnd { state: Some(OnOff::On), duration: None, until: Some("18:00".into()) }, &json!({ "on": true, "until": "2026-11-19 18:00" })), ["✓ Do Not Disturb is on until 2026-11-19 18:00"]);
        assert_eq!(words(&DesktopCommand::Wifi { command: None }, &json!({ "available": true, "on": true, "network": "Home" })), ["Wi-Fi  On · Home"]);
        assert_eq!(words(&DesktopCommand::Wifi { command: Some(WifiCommand::On) }, &json!({ "available": true, "on": true, "network": "Home" })), ["✓ Wi-Fi is on · connected to Home"]);
        assert_eq!(words(&DesktopCommand::Look { look: Some(Look::Dark) }, &json!({ "look": "dark", "dark": true })), ["✓ Dark · kept in user.nami"]);
        let status = words(&DesktopCommand::Status, &json!({ "sound": { "volume": 40, "muted": false, "output": { "name": "Speakers" } }, "wifi": { "available": true, "on": false }, "bluetooth": { "available": false }, "do_not_disturb": { "on": false }, "appearance": { "look": "dark" }, "night_shift": { "when": "sunset", "warmth": 4000 }, "windows": [] }));
        assert_eq!(status[0], "This desktop");
        assert_eq!(status[1], "  Sound           40% · Speakers");
        assert!(status.contains(&"  Night Shift     Sunset to Sunrise · 4000K".to_string()), "{status:?}");
    }
}
