//! What hardware a package is for (`hardware:` in its package.nami), and
//! which packages this computer's own hardware is for:
//!
//! ```yaml
//! hardware:
//!   pci: ["10de:1b80", "10de:*"]   # vendor:device (hex), * for any of the vendor's
//!   usb: ["046d:c52b"]
//! ```
//!
//! `nami doctor` says which of nami's examples (and the repo's own
//! packages/) the computer's devices are for and aren't listed yet; `nami
//! check` notes a listed one this computer has nothing for.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde_yaml::Value;

/// A device's id: vendor and device, as the bus says them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Id {
    pub vendor: u16,
    pub device: u16,
}

/// A device on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub bus: &'static str,
    pub id: Id,
    /// PCI's class (0x03xxxx: a display controller); 0 for USB.
    pub class: u32,
}

/// One `hardware:` entry: a vendor, and a device or any of its (`*`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    vendor: u16,
    device: Option<u16>,
}

/// What a package says it's for.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Wants {
    pci: Vec<Pattern>,
    usb: Vec<Pattern>,
}

fn hex(s: &str) -> Option<u16> {
    (s.len() == 4).then(|| u16::from_str_radix(s, 16).ok()).flatten()
}

/// A package's `hardware:` value; `name` is for what's said when it's wrong.
pub fn parse(v: &Value, name: &str) -> Result<Wants> {
    let usage = "hardware: pci: [\"10de:1b80\", \"10de:*\"] (vendor:device, in hex), usb: [...]";
    let Value::Mapping(m) = v else { bail!("{name}: {usage}") };
    let mut wants = Wants::default();
    for (k, list) in m {
        let into = match k.as_str() {
            Some("pci") => &mut wants.pci,
            Some("usb") => &mut wants.usb,
            _ => bail!("{name}: hardware: {}: pci or usb ({usage})", crate::secrets::key_text(k)),
        };
        let Value::Sequence(items) = list else { bail!("{name}: {usage}") };
        for item in items {
            let s = item.as_str().unwrap_or_default().to_ascii_lowercase();
            let p = s.split_once(':').and_then(|(v, d)| Some(Pattern { vendor: hex(v)?, device: if d == "*" { None } else { Some(hex(d)?) } }));
            into.push(p.ok_or_else(|| anyhow::anyhow!("{name}: hardware: {s:?}: vendor:device, four hex digits each, or vendor:* ({usage})"))?);
        }
    }
    Ok(wants)
}

impl Wants {
    pub fn matches(&self, d: &Device) -> bool {
        let list = if d.bus == "pci" { &self.pci } else { &self.usb };
        list.iter().any(|p| p.vendor == d.id.vendor && p.device.is_none_or(|x| x == d.id.device))
    }

    /// What it's for, as said in a sentence: `10de:* hardware`, or `293
    /// kinds of NVIDIA device (10de)`.
    fn said(&self) -> String {
        let all: Vec<&Pattern> = self.pci.iter().chain(&self.usb).collect();
        match all.as_slice() {
            [p] => format!("{:04x}:{} hardware", p.vendor, p.device.map_or("*".into(), |d| format!("{d:04x}"))),
            [p, rest @ ..] if rest.iter().all(|q| q.vendor == p.vendor) => {
                let name = vendor_name(p.vendor);
                format!("{} kinds of {}device ({:04x})", all.len(), if name.is_empty() { String::new() } else { format!("{name} ") }, p.vendor)
            }
            many => format!("{} kinds of device", many.len()),
        }
    }
}

fn vendor_name(id: u16) -> &'static str {
    match id {
        0x10de => "NVIDIA",
        0x1002 => "AMD",
        0x8086 => "Intel",
        0x14e4 => "Broadcom",
        0x10ec => "Realtek",
        0x168c | 0x17cb => "Qualcomm",
        0x14c3 => "MediaTek",
        _ => "",
    }
}

/// Where the devices are listed: /sys, or `NAMI_SYS` (tests).
pub fn sys_root() -> PathBuf {
    std::env::var_os("NAMI_SYS").map_or_else(|| PathBuf::from("/sys"), PathBuf::from)
}

/// The devices under `sys` (`/sys`, or a test's folder like it).
pub fn devices(sys: &Path) -> Vec<Device> {
    let read = |p: PathBuf| std::fs::read_to_string(p).ok().map(|s| s.trim().trim_start_matches("0x").to_string());
    let mut out = Vec::new();
    for e in std::fs::read_dir(sys.join("bus/pci/devices")).into_iter().flatten().flatten() {
        let p = e.path();
        let (Some(v), Some(d)) = (read(p.join("vendor")).and_then(|s| u16::from_str_radix(&s, 16).ok()), read(p.join("device")).and_then(|s| u16::from_str_radix(&s, 16).ok())) else { continue };
        let class = read(p.join("class")).and_then(|s| u32::from_str_radix(&s, 16).ok()).unwrap_or(0);
        out.push(Device { bus: "pci", id: Id { vendor: v, device: d }, class });
    }
    for e in std::fs::read_dir(sys.join("bus/usb/devices")).into_iter().flatten().flatten() {
        let p = e.path();
        let (Some(v), Some(d)) = (read(p.join("idVendor")).and_then(|s| u16::from_str_radix(&s, 16).ok()), read(p.join("idProduct")).and_then(|s| u16::from_str_radix(&s, 16).ok())) else { continue };
        out.push(Device { bus: "usb", id: Id { vendor: v, device: d }, class: 0 });
    }
    out.sort_by_key(|d| (d.bus, d.id.vendor, d.id.device));
    out.dedup_by_key(|d| (d.bus, d.id));
    out
}

/// A device as a person would say it: `an NVIDIA graphics card (10de:1b80)`.
pub fn describe(d: &Device) -> String {
    let vendor = vendor_name(d.id.vendor);
    let what = match (d.bus, d.class >> 16) {
        ("pci", 0x03) => "graphics card",
        ("pci", 0x02) => "network card",
        ("pci", 0x04) => "sound or video device",
        ("pci", _) => "PCI device",
        _ => "USB device",
    };
    let article = if vendor.is_empty() { if what.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" } } else if "AEIOU".contains(&vendor[..1]) || vendor == "NVIDIA" { "an" } else { "a" };
    let name = [vendor, what].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
    format!("{article} {name} ({:04x}:{:04x})", d.id.vendor, d.id.device)
}

/// A device's own name, from the PCI ID list most Linux systems have
/// (pciutils' or hwdata's pci.ids, next to `sys`'s parent: /usr/share):
/// `NVIDIA GeForce GTX 1070`. None without the list, or for USB.
pub fn model(sys: &Path, d: &Device) -> Option<String> {
    if d.bus != "pci" {
        return None;
    }
    let root = sys.parent().unwrap_or(Path::new("/"));
    let text = ["usr/share/hwdata/pci.ids", "usr/share/misc/pci.ids", "usr/share/pci.ids", "run/current-system/sw/share/hwdata/pci.ids"].iter().find_map(|p| std::fs::read_to_string(root.join(p)).ok())?;
    let (vendor, device) = (format!("{:04x}  ", d.id.vendor), format!("\t{:04x}  ", d.id.device));
    let mut lines = text.lines().skip_while(|l| !l.starts_with(&vendor));
    lines.next()?;
    let name = lines.take_while(|l| l.starts_with('\t') || l.starts_with('#')).find_map(|l| l.strip_prefix(&device))?;
    // `GP104 [GeForce GTX 1070]`: the bracketed name is the one people know.
    let name = name.split_once('[').and_then(|(_, r)| r.split_once(']')).map_or(name, |(n, _)| n).trim();
    let vendor = vendor_name(d.id.vendor);
    Some(if vendor.is_empty() || name.starts_with(vendor) { name.to_string() } else { format!("{vendor} {name}") })
}

/// What the computer is, in a person's words (`nami doctor --about`, the
/// window's This computer): its maker and model, processor, memory,
/// graphics, disks, battery and network, and the system on it. `sys` is
/// /sys (or a test's folder like it); /proc is next to it.
pub fn about(sys: &Path) -> serde_json::Value {
    use serde_json::json;
    let root = sys.parent().unwrap_or(Path::new("/"));
    let read = |p: PathBuf| std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let dmi = |f: &str| read(sys.join("class/dmi/id").join(f)).filter(|v| !PLACEHOLDERS.iter().any(|p| v.eq_ignore_ascii_case(p)));
    // Lenovo's model name is its product_version (product_name is a part number).
    let vendor = dmi("sys_vendor").map(|v| tidy_vendor(&v));
    let model = match vendor.as_deref() {
        Some("Lenovo") => dmi("product_version").or_else(|| dmi("product_name")),
        _ => dmi("product_name"),
    };
    // An ARM board without DMI (a Raspberry Pi) says what it is in its device tree.
    let model = model.or_else(|| read(sys.join("firmware/devicetree/base/model")).map(|m| m.trim_end_matches('\0').to_string()).filter(|m| !m.is_empty()));
    let kind = match dmi("chassis_type").and_then(|t| t.parse::<u32>().ok()) {
        Some(8 | 9 | 10 | 14 | 31 | 32) => "laptop",
        Some(30) => "tablet",
        Some(3..=7 | 13 | 15 | 16 | 35 | 36) => "desktop",
        _ if [&vendor, &model].iter().any(|f| f.as_deref().is_some_and(|m| ["QEMU", "VirtualBox", "VMware", "Virtual Machine", "KVM", "innotek"].iter().any(|v| m.contains(v)))) => "virtual machine",
        _ => "computer",
    };

    let cpuinfo = read(root.join("proc/cpuinfo")).unwrap_or_default();
    let cpu = cpuinfo.lines().find_map(|l| l.strip_prefix("model name").and_then(|r| r.split_once(':')).map(|(_, v)| tidy_cpu(v)));
    let threads = cpuinfo.lines().filter(|l| l.starts_with("processor")).count();
    let kb = |key: &str| read(root.join("proc/meminfo")).and_then(|m| m.lines().find_map(|l| l.strip_prefix(key).map(|r| r.trim().trim_end_matches(" kB").trim().parse::<u64>().ok())).flatten());

    let devices = devices(sys);
    let driver = |addr: &Path| std::fs::read_link(addr.join("driver")).ok().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()));
    let mut graphics = Vec::new();
    for e in std::fs::read_dir(sys.join("bus/pci/devices")).into_iter().flatten().flatten() {
        let p = e.path();
        let class = read(p.join("class")).and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok()).unwrap_or(0);
        if class >> 16 != 0x03 {
            continue;
        }
        let id = |f: &str| read(p.join(f)).and_then(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok());
        let Some(d) = devices.iter().find(|d| d.bus == "pci" && Some(d.id.vendor) == id("vendor") && Some(d.id.device) == id("device")) else { continue };
        graphics.push(json!({ "name": model_or_describe(sys, d), "driver": driver(&p) }));
    }

    let mut disks = Vec::new();
    for e in std::fs::read_dir(sys.join("block")).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if ["loop", "ram", "zram", "dm-", "sr", "md", "nbd"].iter().any(|p| name.starts_with(p)) {
            continue;
        }
        let p = sys.join("block").join(&name);
        let size = read(p.join("size")).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) * 512;
        if size == 0 {
            continue;
        }
        let removable = read(p.join("removable")).as_deref() == Some("1") || std::fs::canonicalize(&p).is_ok_and(|r| r.to_string_lossy().contains("/usb"));
        let virtual_ = read(p.join("device/model")).is_some_and(|m| m.contains("Virtual") || m.contains("QEMU")) || name.starts_with("vd");
        let kind = if removable { "USB" } else if virtual_ { "virtual disk" } else if name.starts_with("nvme") { "NVMe SSD" } else if read(p.join("queue/rotational")).as_deref() == Some("0") { "SSD" } else { "hard disk" };
        disks.push(json!({ "name": name, "model": read(p.join("device/model")), "size": size, "kind": kind, "removable": removable }));
    }
    disks.sort_by_key(|d| d["name"].as_str().unwrap_or("").to_string());

    let mut batteries = Vec::new();
    for e in std::fs::read_dir(sys.join("class/power_supply")).into_iter().flatten().flatten() {
        let p = e.path();
        if read(p.join("type")).as_deref() != Some("Battery") || read(p.join("scope")).as_deref() == Some("Device") {
            continue;
        }
        let n = |f: &str| read(p.join(f)).and_then(|s| s.parse::<u64>().ok());
        let health = match (n("energy_full").or(n("charge_full")), n("energy_full_design").or(n("charge_full_design"))) {
            (Some(full), Some(design)) if design > 0 => Some((full * 100 / design).min(100)),
            _ => None,
        };
        batteries.push(json!({ "charge": n("capacity"), "status": read(p.join("status")), "health": health }));
    }

    let mut network = Vec::new();
    for e in std::fs::read_dir(sys.join("class/net")).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let p = e.path();
        // Real ones only: a device behind it (not lo, bridges, VPNs).
        if !p.join("device").exists() {
            continue;
        }
        let wifi = p.join("wireless").exists() || p.join("phy80211").exists();
        let dev = std::fs::canonicalize(p.join("device")).ok();
        let product = dev.as_ref().and_then(|d| {
            let id = |f: &str| read(d.join(f)).and_then(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok());
            let (v, i) = (id("vendor")?, id("device")?);
            devices.iter().find(|x| x.bus == "pci" && x.id.vendor == v && x.id.device == i).map(|x| model_or_describe(sys, x))
        });
        network.push(json!({ "name": name, "kind": if wifi { "Wi-Fi" } else { "wired" }, "device": product, "driver": dev.as_deref().and_then(|d| driver(d)), "up": read(p.join("operstate")).as_deref() == Some("up") }));
    }
    network.sort_by_key(|n| n["name"].as_str().unwrap_or("").to_string());

    json!({
        "maker": vendor,
        "model": model,
        "kind": kind,
        "processor": cpu,
        "threads": threads,
        "memory": kb("MemTotal:").map(|k| k * 1024),
        "graphics": graphics,
        "disks": disks,
        "batteries": batteries,
        "network": network,
        "kernel": read(root.join("proc/sys/kernel/osrelease")),
        "uptime": read(root.join("proc/uptime")).and_then(|u| u.split_whitespace().next()?.parse::<f64>().ok()).map(|s| s as u64),
        "nami": env!("CARGO_PKG_VERSION"),
        "system": read(PathBuf::from("/run/current-system/version")).filter(|_| root == Path::new("/")),
    })
}

/// [`about`] as lines for the terminal.
pub fn about_text(a: &serde_json::Value) -> String {
    let st = |v: &serde_json::Value| v.as_str().map(String::from);
    let gb = |b: u64| if b >= 1_000_000_000_000 { format!("{:.1} TB", b as f64 / 1e12) } else { format!("{} GB", (b as f64 / 1e9).round()) };
    let mut out = Vec::new();
    let name = [st(&a["maker"]), st(&a["model"])].into_iter().flatten().collect::<Vec<_>>().join(" ");
    out.push(("computer", format!("{}{}", if name.is_empty() { "a computer".into() } else { name }, a["kind"].as_str().filter(|k| *k != "computer").map(|k| format!(" ({k})")).unwrap_or_default())));
    if let Some(p) = st(&a["processor"]) {
        out.push(("processor", format!("{p}, {} threads", a["threads"])));
    }
    if let Some(m) = a["memory"].as_u64() {
        out.push(("memory", format!("{} GB", (m as f64 / (1u64 << 30) as f64).round())));
    }
    for g in a["graphics"].as_array().into_iter().flatten() {
        out.push(("graphics", format!("{}{}", st(&g["name"]).unwrap_or_default(), st(&g["driver"]).map(|d| format!(" ({d})")).unwrap_or_default())));
    }
    for d in a["disks"].as_array().into_iter().flatten() {
        out.push(("disk", format!("{} {}, {}{}", st(&d["name"]).unwrap_or_default(), st(&d["kind"]).unwrap_or_default(), gb(d["size"].as_u64().unwrap_or(0)), st(&d["model"]).map(|m| format!(" ({m})")).unwrap_or_default())));
    }
    for b in a["batteries"].as_array().into_iter().flatten() {
        out.push(("battery", format!("{}%{}{}", b["charge"], st(&b["status"]).map(|s| format!(", {}", s.to_lowercase())).unwrap_or_default(), b["health"].as_u64().map(|h| format!(", holds {h}% of what it did new")).unwrap_or_default())));
    }
    for n in a["network"].as_array().into_iter().flatten() {
        out.push(("network", format!("{} {}{}{}", st(&n["kind"]).unwrap_or_default(), st(&n["name"]).unwrap_or_default(), st(&n["device"]).map(|d| format!(": {d}")).unwrap_or_default(), st(&n["driver"]).map(|d| format!(" ({d})")).unwrap_or_default())));
    }
    out.push(("system", format!("nami {}{}{}", a["nami"].as_str().unwrap_or(""), st(&a["system"]).map(|s| format!(", system {s}")).unwrap_or_default(), st(&a["kernel"]).map(|k| format!(", Linux {k}")).unwrap_or_default())));
    out.iter().map(|(k, v)| format!("{k:<10} {v}\n")).collect()
}

/// What makers leave in the DMI fields when they don't fill them in.
const PLACEHOLDERS: &[&str] = &["To Be Filled By O.E.M.", "System Product Name", "System manufacturer", "Default string", "Not Applicable", "None", "O.E.M.", "Type1ProductConfigId", "System Version", "INVALID"];

/// `LENOVO`, `ASUSTeK COMPUTER INC.`, `Dell Inc.` as people say them.
fn tidy_vendor(v: &str) -> String {
    let l = v.to_lowercase();
    for (k, name) in [("lenovo", "Lenovo"), ("asus", "ASUS"), ("dell", "Dell"), ("hewlett", "HP"), ("hp", "HP"), ("micro-star", "MSI"), ("acer", "Acer"), ("apple", "Apple"), ("microsoft", "Microsoft"), ("framework", "Framework"), ("gigabyte", "Gigabyte"), ("samsung", "Samsung"), ("toshiba", "Toshiba"), ("qemu", "QEMU"), ("system76", "System76"), ("tuxedo", "TUXEDO")] {
        if l.starts_with(k) {
            return name.to_string();
        }
    }
    v.trim_end_matches(['.', ',']).trim_end_matches(" Inc").trim_end_matches(" Corporation").to_string()
}

/// `Intel(R) Core(TM) i7-1165G7 @ 2.80GHz` as `Intel Core i7-1165G7`.
fn tidy_cpu(v: &str) -> String {
    let s = v.replace("(R)", "").replace("(TM)", "").replace("(tm)", "");
    let s = s.split(" @ ").next().unwrap_or(&s).replace(" CPU", "").replace(" Processor", "");
    let s = s.split_once(" with ").or_else(|| s.split_once(" w/ ")).map_or(s.as_str(), |(a, _)| a).to_string();
    let s = s.trim_end_matches(|c: char| c.is_whitespace()).to_string();
    s.split_whitespace().filter(|w| !w.ends_with("-Core")).collect::<Vec<_>>().join(" ")
}

/// A device's own name, else what it is.
fn model_or_describe(sys: &Path, d: &Device) -> String {
    model(sys, d).unwrap_or_else(|| {
        let s = describe(d);
        s.split_once(' ').map_or(s.clone(), |(_, rest)| rest.to_string())
    })
}

/// A package this computer's hardware is for.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Suggestion {
    /// The device, as [`describe`] says it.
    pub hardware: String,
    /// Its own name, when the system has the PCI ID list (`model`).
    pub model: Option<String>,
    pub package: String,
    /// The computer's file lists it already.
    pub listed: bool,
}

/// Package folders by name and their package.nami text: nami's examples,
/// and over them the ones in the packages/ folder nearest `base`.
fn packages(base: Option<&Path>) -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = crate::new::example_packages().into_iter().map(|(n, t)| (n, t.to_string())).collect();
    if let Some(dir) = base.and_then(crate::config::packages_dir) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Ok(text) = std::fs::read_to_string(crate::oldnames::find_in(&e.path(), "package.nami")) {
                all.retain(|(n, _)| *n != name);
                all.push((name, text));
            }
        }
    }
    all.sort();
    all
}

/// Each package's `hardware:`, for the ones that say it.
fn wanting(base: Option<&Path>) -> Vec<(String, Wants)> {
    packages(base)
        .into_iter()
        .filter_map(|(name, text)| {
            let v: Value = serde_yaml::from_str(&text).ok()?;
            Some((name.clone(), parse(v.get("hardware")?, &name).ok()?))
        })
        .collect()
}

/// Whether the computer `file` uses the package folder `name` (its flat
/// view names each folder a line came from).
fn uses(flat: &str, name: &str) -> bool {
    let file = format!("packages/{name}/package.nami");
    flat.contains(&format!("# {file}")) || flat.contains(&format!("/{file}"))
}

/// The packages this computer's devices (under `sys`) are for: of nami's
/// examples and the packages/ folder of the computer's `file`, if any.
pub fn suggestions(sys: &Path, file: Option<&Path>) -> Vec<Suggestion> {
    let devs = devices(sys);
    let flat = file.and_then(|f| crate::config::flat(f).ok()).unwrap_or_default();
    let mut out = Vec::new();
    for (name, wants) in wanting(file.and_then(Path::parent)) {
        for d in devs.iter().filter(|d| wants.matches(d)) {
            out.push(Suggestion { hardware: describe(d), model: model(sys, d), package: name.clone(), listed: uses(&flat, &name) });
        }
    }
    out
}

/// For `nami check` of the running computer's own file: each package it
/// uses whose `hardware:` this computer has nothing for (fine on a USB drive
/// made for other computers, so only noted).
pub fn unmatched(sys: &Path, file: &Path) -> Vec<String> {
    let devs = devices(sys);
    let Ok(flat) = crate::config::flat(file) else { return Vec::new() };
    wanting(file.parent())
        .into_iter()
        .filter(|(name, wants)| uses(&flat, name) && !devs.iter().any(|d| wants.matches(d)))
        .map(|(name, wants)| format!("{name} is for {}, which this computer hasn't (fine if the file is for other computers too)", wants.said()))
        .collect()
}

#[cfg(test)]
mod tests {

    #[test]
    fn what_the_computer_is() {
        let t = std::env::temp_dir().join(format!("nami-about-{}", std::process::id()));
        let sys = t.join("sys");
        let w = |p: &str, v: &str| {
            let f = t.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, v).unwrap();
        };
        w("sys/class/dmi/id/sys_vendor", "LENOVO\n");
        w("sys/class/dmi/id/product_name", "21HM004GMX\n");
        w("sys/class/dmi/id/product_version", "ThinkPad X1 Carbon Gen 11\n");
        w("sys/class/dmi/id/chassis_type", "10\n");
        w("proc/cpuinfo", "processor\t: 0\nmodel name\t: 13th Gen Intel(R) Core(TM) i7-1365U\nprocessor\t: 1\nmodel name\t: 13th Gen Intel(R) Core(TM) i7-1365U\n");
        w("proc/meminfo", "MemTotal:       32463000 kB\nMemFree: 1 kB\n");
        w("proc/sys/kernel/osrelease", "6.18.2\n");
        w("sys/bus/pci/devices/0000:00:02.0/vendor", "0x8086\n");
        w("sys/bus/pci/devices/0000:00:02.0/device", "0xa7a1\n");
        w("sys/bus/pci/devices/0000:00:02.0/class", "0x030000\n");
        w("sys/block/nvme0n1/size", "1000215216\n");
        w("sys/block/nvme0n1/device/model", "Samsung SSD 990 PRO 512GB\n");
        w("sys/block/loop0/size", "100\n");
        w("sys/class/power_supply/BAT0/type", "Battery\n");
        w("sys/class/power_supply/BAT0/capacity", "81\n");
        w("sys/class/power_supply/BAT0/status", "Discharging\n");
        w("sys/class/power_supply/BAT0/energy_full", "45000000\n");
        w("sys/class/power_supply/BAT0/energy_full_design", "57000000\n");
        let a = about(&sys);
        assert_eq!(a["maker"], "Lenovo");
        assert_eq!(a["model"], "ThinkPad X1 Carbon Gen 11", "Lenovo's name is its product_version");
        assert_eq!(a["kind"], "laptop");
        assert_eq!(a["processor"], "13th Gen Intel Core i7-1365U");
        assert_eq!(a["threads"], 2);
        assert_eq!(a["memory"], 32463000u64 * 1024);
        assert_eq!(a["graphics"][0]["name"], "Intel graphics card (8086:a7a1)");
        assert_eq!(a["disks"].as_array().unwrap().len(), 1, "no loop devices");
        assert_eq!(a["disks"][0]["kind"], "NVMe SSD");
        assert_eq!(a["batteries"][0]["health"], 78);
        let text = about_text(&a);
        assert!(text.starts_with("computer   Lenovo ThinkPad X1 Carbon Gen 11 (laptop)\n"), "{text}");
        assert!(text.contains("battery    81%, discharging, holds 78% of what it did new"), "{text}");
        std::fs::remove_dir_all(&t).unwrap();
        assert_eq!(tidy_cpu("AMD Ryzen 7 5800X 8-Core Processor"), "AMD Ryzen 7 5800X");
        assert_eq!(tidy_cpu("AMD Ryzen 7 PRO 7840U w/ Radeon 780M Graphics"), "AMD Ryzen 7 PRO 7840U");
        assert_eq!(tidy_vendor("ASUSTeK COMPUTER INC."), "ASUS");
    }

    use super::*;

    /// A /sys with these PCI devices (vendor, device, class) and USB ones.
    fn fake_sys(pci: &[(&str, &str, &str)], usb: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nami-sys-{}-{}", std::process::id(), pci.len() * 10 + usb.len()));
        let _ = std::fs::remove_dir_all(&dir);
        for (i, (v, d, c)) in pci.iter().enumerate() {
            let p = dir.join(format!("bus/pci/devices/0000:0{i}:00.0"));
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("vendor"), format!("0x{v}\n")).unwrap();
            std::fs::write(p.join("device"), format!("0x{d}\n")).unwrap();
            std::fs::write(p.join("class"), format!("0x{c}\n")).unwrap();
        }
        for (i, (v, d)) in usb.iter().enumerate() {
            let p = dir.join(format!("bus/usb/devices/1-{i}"));
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("idVendor"), format!("{v}\n")).unwrap();
            std::fs::write(p.join("idProduct"), format!("{d}\n")).unwrap();
        }
        dir
    }

    #[test]
    fn a_cards_model_comes_from_pci_ids() {
        let root = std::env::temp_dir().join(format!("nami-pciids-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("usr/share/misc")).unwrap();
        let ids = "# comment\n10de  NVIDIA Corporation\n\t1b81  GP104 [GeForce GTX 1070]\n\t\t1043 8599  ROG STRIX\n\t2684  AD102 [GeForce RTX 4090]\n1af4  Red Hat, Inc.\n\t1050  Virtio 1.0 GPU\n";
        std::fs::write(root.join("usr/share/misc/pci.ids"), ids).unwrap();
        let sys = root.join("sys");
        let pci = |v, d| Device { bus: "pci", id: Id { vendor: v, device: d }, class: 0x030000 };
        assert_eq!(model(&sys, &pci(0x10de, 0x1b81)).as_deref(), Some("NVIDIA GeForce GTX 1070"));
        assert_eq!(model(&sys, &pci(0x10de, 0x2684)).as_deref(), Some("NVIDIA GeForce RTX 4090"));
        assert_eq!(model(&sys, &pci(0x1af4, 0x1050)).as_deref(), Some("Virtio 1.0 GPU"));
        assert_eq!(model(&sys, &pci(0x10de, 0x9999)), None, "not in the list");
        assert_eq!(model(&root.join("elsewhere/sys"), &pci(0x10de, 0x1b81)), None, "no list");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hardware_matches_by_vendor_and_device() {
        let v: Value = serde_yaml::from_str("pci: [\"10de:1B80\", \"8086:*\"]\nusb: [\"046d:c52b\"]").unwrap();
        let w = parse(&v, "x").unwrap();
        let pci = |v, d| Device { bus: "pci", id: Id { vendor: v, device: d }, class: 0x030000 };
        assert!(w.matches(&pci(0x10de, 0x1b80)) && !w.matches(&pci(0x10de, 0x2684)));
        assert!(w.matches(&pci(0x8086, 0x1234)), "any of the vendor's");
        assert!(w.matches(&Device { bus: "usb", id: Id { vendor: 0x046d, device: 0xc52b }, class: 0 }));
        assert!(!w.matches(&Device { bus: "usb", id: Id { vendor: 0x10de, device: 0x1b80 }, class: 0 }), "a bus each");
        for bad in ["pci: [\"10de\"]", "pci: [\"10de:1b8\"]", "pci: \"10de:*\"", "gpu: [\"10de:*\"]", "[\"10de:*\"]"] {
            assert!(parse(&serde_yaml::from_str(bad).unwrap(), "x").is_err(), "{bad}");
        }
    }

    #[test]
    fn devices_from_sys() {
        let sys = fake_sys(&[("10de", "1b80", "030000"), ("8086", "a7a0", "030000"), ("10ec", "8168", "020000")], &[("046d", "c52b")]);
        let devs = devices(&sys);
        assert_eq!(devs.len(), 4);
        assert_eq!(describe(&devs[0]), "an NVIDIA graphics card (10de:1b80)");
        assert_eq!(describe(&devs[2]), "an Intel graphics card (8086:a7a0)");
        assert_eq!(describe(&devs[1]), "a Realtek network card (10ec:8168)");
        assert_eq!(describe(&devs[3]), "a USB device (046d:c52b)");
        std::fs::remove_dir_all(&sys).unwrap();
    }

    #[test]
    fn exactly_one_nvidia_example_per_card() {
        // A GTX 1070 (Pascal) is nvidia-legacy's, an RTX 4090 nvidia's, and
        // an old Kepler card neither's (no driver here has it).
        for (card, want) in [("1b81", Some("nvidia-legacy")), ("2684", Some("nvidia")), ("1180", None)] {
            let sys = fake_sys(&[("10de", card, "030000")], &[]);
            let found: Vec<String> = suggestions(&sys, None).into_iter().map(|s| s.package).collect();
            assert_eq!(found, want.map(|w| vec![w.to_string()]).unwrap_or_default(), "10de:{card}");
            std::fs::remove_dir_all(&sys).unwrap();
        }
    }
}
