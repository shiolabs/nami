//! The USB drives plugged in, on each system: only removable disks, never
//! the one the computer runs from. NAMI_STICK_TEST_DISK=<file> lists a file
//! as a USB drive too (for tests).

#[derive(Clone, Debug, PartialEq)]
pub struct Disk {
    /// What the writer opens: /dev/sdb, \\.\PhysicalDrive2, /dev/disk4.
    pub path: String,
    /// Its maker's name for it: "SanDisk Ultra".
    pub name: String,
    pub bytes: u64,
}

impl Disk {
    pub fn describe(&self) -> String {
        format!("{} · {:.0} GB", self.model(), self.bytes as f64 / 1e9)
    }

    /// As asking to erase it says it: "SanDisk Ultra (32 GB)".
    pub fn named(&self) -> String {
        format!("{} ({:.0} GB)", self.model(), self.bytes as f64 / 1e9)
    }

    fn model(&self) -> &str {
        if self.name.trim().is_empty() { "USB drive" } else { self.name.trim() }
    }
}

pub fn list() -> Vec<Disk> {
    let mut out = os_list();
    if let Ok(f) = std::env::var("NAMI_STICK_TEST_DISK") {
        let bytes = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
        out.push(Disk { path: f, name: "Test USB drive".into(), bytes });
    }
    out
}

#[cfg(target_os = "linux")]
fn os_list() -> Vec<Disk> {
    let read = |p: std::path::PathBuf| std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default();
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/block") else { return out };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if ["loop", "ram", "zram", "sr", "dm-", "md", "nbd"].iter().any(|p| name.starts_with(p)) {
            continue;
        }
        let dir = e.path();
        let usb = std::fs::canonicalize(&dir).is_ok_and(|p| p.to_string_lossy().contains("/usb"));
        if read(dir.join("removable")) != "1" && !usb {
            continue;
        }
        let bytes = read(dir.join("size")).parse::<u64>().unwrap_or(0) * 512;
        if bytes == 0 {
            continue;
        }
        // Not the one this computer runs from (a live stick's own).
        let dev = format!("/dev/{name}");
        let runs_here = mounts.lines().any(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            f.len() > 1 && f[0].starts_with(&dev) && matches!(f[1], "/" | "/boot" | "/nix/store")
        });
        if runs_here {
            continue;
        }
        let model = format!("{} {}", read(dir.join("device/vendor")), read(dir.join("device/model")));
        out.push(Disk { path: dev, name: model.split_whitespace().collect::<Vec<_>>().join(" "), bytes });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(target_os = "windows")]
fn os_list() -> Vec<Disk> {
    use std::os::windows::process::CommandExt;
    // USB disks, not the one Windows starts from.
    let script = "Get-Disk | Where-Object { $_.BusType -eq 'USB' -and -not $_.IsBoot -and -not $_.IsSystem } | ForEach-Object { \"$($_.Number)`t$($_.Size)`t$($_.FriendlyName)\" }";
    let Ok(out) = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", script]).creation_flags(0x0800_0000).output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.trim().splitn(3, '\t').collect();
            Some(Disk { path: format!(r"\\.\PhysicalDrive{}", f.first()?.parse::<u32>().ok()?), bytes: f.get(1)?.parse().ok()?, name: f.get(2).unwrap_or(&"").to_string() })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn os_list() -> Vec<Disk> {
    let Ok(out) = std::process::Command::new("diskutil").args(["list", "-plist", "external", "physical"]).output() else { return Vec::new() };
    let Ok(list) = plist::Value::from_reader_xml(&out.stdout[..]) else { return Vec::new() };
    let whole: Vec<String> = list
        .as_dictionary()
        .and_then(|d| d.get("WholeDisks"))
        .and_then(|w| w.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_string().map(String::from)).collect())
        .unwrap_or_default();
    whole
        .into_iter()
        .filter_map(|d| {
            let info = std::process::Command::new("diskutil").args(["info", "-plist", &d]).output().ok()?;
            let v = plist::Value::from_reader_xml(&info.stdout[..]).ok()?;
            let dict = v.as_dictionary()?;
            if dict.get("Internal").and_then(|x| x.as_boolean()) == Some(true) {
                return None;
            }
            let bytes = dict.get("TotalSize").or_else(|| dict.get("Size")).and_then(|x| x.as_unsigned_integer()).unwrap_or(0);
            let name = dict.get("MediaName").and_then(|x| x.as_string()).unwrap_or("").to_string();
            Some(Disk { path: format!("/dev/{d}"), name, bytes })
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn os_list() -> Vec<Disk> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::Disk;

    #[test]
    fn named_by_its_model_and_size() {
        let disk = Disk { path: "/dev/sdb".into(), name: " SanDisk Ultra ".into(), bytes: 32_017_047_552 };
        assert_eq!(disk.named(), "SanDisk Ultra (32 GB)");
        assert_eq!(disk.describe(), "SanDisk Ultra · 32 GB");
        assert_eq!(Disk { name: String::new(), ..disk }.named(), "USB drive (32 GB)");
    }
}
