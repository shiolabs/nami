//! What nixpkgs GUI apps expect from the system, provided on any distro.
//!
//! Nixpkgs binaries never look at the host's /usr/lib, so on a non-NixOS host
//! they find no GPU drivers, no locales and maybe no fonts. NixOS solves this
//! with fixed paths that nami recreates:
//!
//! - `/run/opengl-driver`: libglvnd, the Vulkan loader, GBM and VA-API look
//!   for drivers here (nixpkgs patches them to). We point it at a "runtime"
//!   tree holding nixpkgs mesa, plus the host's NVIDIA userspace libraries if
//!   the proprietary kernel module is loaded (they must match its version, so
//!   they have to come from the host).
//! - `LOCALE_ARCHIVE`: nixpkgs glibc reads locales from here.
//! - `FONTCONFIG_FILE`: host fonts, nami-installed fonts, and a fallback config
//!   for hosts without fontconfig.
//!
//! The runtime is only pulled in once a package needs it: when something in
//! the installed closure links against libglvnd, mesa or the Vulkan loader.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::index::Index;
use crate::profile::Entry;
use crate::store::Store;

/// The runtime profile (drivers, fonts, locales), as this computer names it: /nami/profiles/runtime.
pub fn runtime_profile() -> PathBuf {
    crate::migrate::here().join("profiles/runtime")
}
pub const OPENGL_DRIVER: &str = "/run/opengl-driver";

/// Closure members that mean "this program renders with the GPU".
const GRAPHICS_MARKERS: &[&str] = &["-libglvnd-", "-mesa-", "-vulkan-loader-", "-libGL-"];

/// Does anything in these (realised) paths' closures render with the GPU?
pub fn needs_graphics(store: &Store, roots: &[String]) -> Result<bool> {
    // A Mac's apps use macOS's own graphics, fonts and locales: no runtime to add.
    if crate::index::MAC {
        return Ok(false);
    }
    Ok(store.closure(roots)?.iter().any(|p| GRAPHICS_MARKERS.iter().any(|m| p.contains(m))))
}

/// Decide which runtime packages the given (realised) closure needs.
/// `force`: a user's profile needs them even if the system's doesn't.
pub fn wanted(store: &Store, index: &Index, roots: &[String], host: &Path, force: bool) -> Result<Vec<Entry>> {
    if crate::index::MAC || !force && !needs_graphics(store, roots)? {
        return Ok(Vec::new());
    }
    let locales = if needs_all_locales(host) { "glibcLocales" } else { "glibcLocalesUtf8" };
    let mut attrs = vec!["mesa", "fontconfig", locales];
    if !host.join("etc/fonts/fonts.conf").exists() {
        attrs.push("dejavu_fonts"); // no host fonts at all: give sans, serif and mono
    }
    let mut entries = Vec::new();
    for (attr, output) in attrs.into_iter().map(|a| (a, "out")) {
        let pkg = index.get(attr)?;
        if let Some(Some(path)) = pkg.outputs.get(output) {
            entries.push(Entry { attr: attr.to_string(), name: pkg.name.clone(), paths: vec![path.clone()] });
        }
    }
    Ok(entries)
}

/// Fill a runtime generation tree with the parts that aren't store paths.
pub fn populate(tree: &Path, host: &Path, runtime: &[Entry]) -> Result<()> {
    fs::create_dir_all(tree.join("nami"))?;
    fs::write(tree.join("nami/fonts.conf"), fonts_conf(host, runtime))?;
    let linked = link_host_nvidia(tree, host)?;
    if linked > 0 {
        crate::say!("linked {linked} NVIDIA driver files from the host");
    }
    Ok(())
}

/// Make `/run/opengl-driver` point at the runtime profile. It lives on a tmpfs,
/// so install.sh also registers it with systemd-tmpfiles for every boot.
pub fn ensure_driver_link(root: &Path) -> Result<()> {
    if crate::index::MAC {
        return Ok(());
    }
    let link = root.join(OPENGL_DRIVER.trim_start_matches('/'));
    let ours = |t: &Path| [runtime_profile(), Path::new("/").join(crate::migrate::VAR).join("profiles/runtime")].iter().any(|p| crate::oldnames::same_place(t, p));
    if link.symlink_metadata().is_err() {
        fs::create_dir_all(link.parent().unwrap())?;
        symlink(runtime_profile(), &link)?;
    } else if !fs::read_link(&link).is_ok_and(|t| ours(&t)) {
        crate::say!("warning: {OPENGL_DRIVER} exists and isn't nami's (NixOS host?); leaving it alone");
    }
    Ok(())
}

fn fonts_conf(host: &Path, runtime: &[Entry]) -> String {
    let here = crate::migrate::seen(host).display().to_string();
    // Prefer the host's config (its fonts, its rendering settings). On hosts
    // without fontconfig, use nixpkgs' defaults. Its fonts.conf only includes
    // the *host's* /etc/fonts/conf.d, so add the package's own conf.d too:
    // that's where the sans-serif/serif/monospace aliases live.
    let includes = match runtime.iter().find(|e| e.attr == "fontconfig") {
        Some(fc) if !host.join("etc/fonts/fonts.conf").exists() => {
            let etc = format!("{}/etc/fonts", fc.paths[0]);
            format!("  <include ignore_missing=\"yes\">{etc}/fonts.conf</include>\n  <include ignore_missing=\"yes\">{etc}/conf.d</include>")
        }
        _ => "  <include ignore_missing=\"yes\">/etc/fonts/fonts.conf</include>".to_string(),
    };
    format!(
        r#"<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<!-- generated by nami -->
<fontconfig>
{includes}
  <dir>{here}/profiles/default/share/fonts</dir>
  <dir>{here}/profiles/runtime/share/fonts</dir>
  <dir>~/.local/state/nami/profile/share/fonts</dir>
  <dir>/usr/share/fonts</dir>
  <dir>/usr/local/share/fonts</dir>
  <dir prefix="xdg">fonts</dir>
  <cachedir prefix="xdg">fontconfig</cachedir>
</fontconfig>
"#
    )
}

/// The system locale, from the environment or the distro's config file.
fn needs_all_locales(host: &Path) -> bool {
    let mut langs: Vec<String> = ["LANG", "LC_ALL", "LC_CTYPE"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .collect();
    for f in ["etc/locale.conf", "etc/default/locale"] {
        if let Ok(text) = fs::read_to_string(host.join(f)) {
            langs.extend(text.lines().filter_map(|l| l.split_once('=')).map(|(_, v)| v.trim_matches('"').to_string()));
        }
    }
    langs.iter().any(|l| {
        let l = l.to_lowercase();
        !(l.is_empty() || l == "c" || l == "posix" || l.starts_with("c.") || l.starts_with("en_us"))
    })
}

/// Where distributions keep NVIDIA's libraries (Debian's folder is named for the computer's
/// kind: x86_64-linux-gnu, aarch64-linux-gnu).
fn nvidia_lib_dirs() -> [String; 6] {
    let m = crate::arch::MULTIARCH;
    [format!("usr/lib/{m}"), "usr/lib64".into(), "usr/lib".into(), "usr/lib/nvidia".into(), "usr/lib64/nvidia".into(), format!("usr/lib/{m}/nvidia/current")]
}
const NVIDIA_LIB_PREFIXES: &[&str] = &[
    "libGLX_nvidia.", "libEGL_nvidia.", "libGLESv1_CM_nvidia.", "libGLESv2_nvidia.", "libnvidia-",
    "libcuda.", "libnvcuvid.", "libnvoptix.", "libnvidia-egl-",
];
const NVIDIA_DATA: &[(&str, &str)] = &[
    ("usr/share/vulkan/icd.d", "share/vulkan/icd.d"),
    ("etc/vulkan/icd.d", "share/vulkan/icd.d"),
    ("usr/share/vulkan/implicit_layer.d", "share/vulkan/implicit_layer.d"),
    ("usr/share/glvnd/egl_vendor.d", "share/glvnd/egl_vendor.d"),
    ("usr/share/egl/egl_external_platform.d", "share/egl/egl_external_platform.d"),
];

/// If the proprietary NVIDIA kernel module is loaded, symlink the host's
/// matching userspace libraries into the runtime tree. They only depend on
/// libc/libm/libdl, so they load fine into nixpkgs processes.
fn link_host_nvidia(tree: &Path, host: &Path) -> Result<usize> {
    let Ok(version) = fs::read_to_string(host.join("sys/module/nvidia/version")) else { return Ok(0) };
    let version = version.trim();
    let mut linked = 0;
    for dir in nvidia_lib_dirs() {
        let dir = host.join(dir);
        for (sub, dest) in [("", "lib"), ("vdpau", "lib/vdpau"), ("gbm", "lib/gbm")] {
            let Ok(entries) = fs::read_dir(dir.join(sub)) else { continue };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let ours = NVIDIA_LIB_PREFIXES.iter().any(|p| name.starts_with(p))
                    || name.starts_with("libvdpau_nvidia")
                    || name.starts_with("nvidia-drm_gbm");
                if ours && (name.contains(version) || e.path().is_symlink() || sub == "gbm") {
                    linked += link_into(tree, dest, &name, &e.path(), host)?;
                }
            }
        }
    }
    if linked == 0 {
        crate::say!("warning: NVIDIA module {version} is loaded but its userspace libraries weren't found");
        return Ok(0);
    }
    for (src, dest) in NVIDIA_DATA {
        let Ok(entries) = fs::read_dir(host.join(src)) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.contains("nvidia") {
                linked += link_into(tree, dest, &name, &e.path(), host)?;
            }
        }
    }
    Ok(linked)
}

/// Symlink `src` (an on-disk path under `host`) to `tree/dest/name`, using
/// its real (host-absolute) path. Existing entries win: mesa comes first.
fn link_into(tree: &Path, dest: &str, name: &str, src: &Path, host: &Path) -> Result<usize> {
    let dir = tree.join(dest);
    fs::create_dir_all(&dir)?;
    let to = dir.join(name);
    if to.symlink_metadata().is_ok() {
        return Ok(0);
    }
    let real: PathBuf = Path::new("/").join(src.strip_prefix(host).unwrap_or(src));
    symlink(real, to)?;
    Ok(1)
}

/// Debug helper for `nami doctor`: which runtime pieces are active.
pub fn describe(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let link = root.join(OPENGL_DRIVER.trim_start_matches('/'));
    out.push((
        OPENGL_DRIVER.to_string(),
        match fs::read_link(&link) {
            Ok(t) => format!("-> {}", t.display()),
            Err(_) => "missing".into(),
        },
    ));
    let tree = crate::migrate::var_dir(root).join("profiles/runtime");
    let drivers: Vec<String> = fs::read_dir(tree.join("lib/dri"))
        .map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix("_dri.so").map(String::from)).collect())
        .unwrap_or_default();
    out.push(("mesa drivers".into(), if drivers.is_empty() { "none".into() } else { drivers.join(" ") }));
    let icds: Vec<String> = fs::read_dir(tree.join("share/vulkan/icd.d"))
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    out.push(("vulkan ICDs".into(), if icds.is_empty() { "none".into() } else { icds.join(" ") }));
    let nvidia = fs::read_to_string(root.join("sys/module/nvidia/version")).map(|v| v.trim().to_string());
    out.push(("nvidia module".into(), nvidia.unwrap_or_else(|_| "not loaded".into())));
    out
}

/// The distro's certificate bundle, for programs from the store (they look
/// where NixOS keeps it).
pub fn ca_bundle() -> Option<&'static str> {
    ["/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/ca-bundle.pem", "/etc/ssl/cert.pem"].into_iter().find(|f| std::path::Path::new(f).is_file())
}
