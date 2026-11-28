//! `nami bundle --test`: the finished bundle run on the distributions people
//! have, each in a clean container (rootless podman), as the site's
//! showcase (tests/showcase.sh) runs its apps: as someone who isn't root,
//! with an empty cache, its first start timed; an app with a window under
//! X11 (Xvfb), its window looked for and a screenshot's colours counted; a
//! command run with `--version` (or `--test-args`). What it prints is the
//! showcase's table, for this bundle.
//!
//! Each distribution's image is the distro's own plus what a desktop has
//! (Xvfb, Mesa, fonts, D-Bus), built once and kept by podman as
//! `nami-bundle-test-<distro>`; what they take on the disk is said.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::bundle_format::{self as format, Meta};

/// What runs in each container.
const INNER: &str = include_str!("bundle_test.sh");

/// The distributions, as podman names their images, and how each gets
/// what a desktop has.
pub const DISTROS: &[(&str, &str)] = &[
    ("ubuntu:20.04", "docker.io/library/ubuntu:20.04"),
    ("ubuntu:24.04", "docker.io/library/ubuntu:24.04"),
    ("debian:12", "docker.io/library/debian:12"),
    ("fedora", "docker.io/library/fedora:latest"),
    ("arch", "docker.io/library/archlinux:latest"),
    ("opensuse", "docker.io/opensuse/tumbleweed:latest"),
    // Linux Mint's own image: Ubuntu 24.04 with Mint 22.3's packages (zena,
    // the newest released: linuxmintd's mint23 image has no repository yet).
    ("mint", "docker.io/linuxmintd/mint22.3-amd64:latest"),
];

/// Distributions whose images are x86_64 alone (Arch Linux, Linux Mint): an ARM computer
/// tests on the others.
const X86_ONLY: &[&str] = &["arch", "mint"];

/// The distributions this computer tests on: every one, or on ARM those with an ARM image.
pub fn distros_here() -> Vec<(&'static str, &'static str)> {
    DISTROS.iter().copied().filter(|(n, _)| !crate::arch::ARM || !X86_ONLY.contains(n)).collect()
}

/// The packages a desktop has, per family: X11 (Xvfb, xwininfo), a
/// screenshot (ImageMagick), Mesa's OpenGL and Vulkan, fonts, D-Bus.
fn desktop_packages(name: &str) -> &'static str {
    match name {
        "ubuntu:20.04" | "ubuntu:24.04" | "debian:12" | "mint" => {
            "RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends xvfb x11-utils imagemagick libgl1-mesa-dri libegl1 libglx-mesa0 libgl1 mesa-vulkan-drivers fonts-dejavu-core procps dbus ca-certificates && rm -rf /var/lib/apt/lists/*"
        }
        "fedora" => "RUN dnf install -y xorg-x11-server-Xvfb xwininfo ImageMagick mesa-dri-drivers mesa-libEGL mesa-libGL mesa-vulkan-drivers dejavu-sans-fonts procps-ng dbus-daemon && dnf clean all",
        "arch" => "RUN pacman -Syu --noconfirm xorg-server-xvfb xorg-xwininfo imagemagick mesa vulkan-swrast ttf-dejavu procps-ng dbus && pacman -Scc --noconfirm",
        _ => "RUN zypper -n install --no-recommends xorg-x11-server-Xvfb xwininfo ImageMagick Mesa-dri Mesa-libEGL1 Mesa-libGL1 libvulkan_lvp dejavu-fonts procps dbus-1-daemon gzip && zypper clean -a",
    }
}

pub struct Options {
    /// Which distributions (names from DISTROS); empty: all.
    pub on: Vec<String>,
    /// For a command: what to run it with (default `--version`).
    pub args: Option<String>,
    pub json: bool,
}

#[derive(Debug, Default, Serialize, Clone)]
pub struct Outcome {
    /// As said here: ubuntu:24.04.
    pub distribution: String,
    /// As it says itself (/etc/os-release).
    pub pretty_name: String,
    pub glibc: String,
    pub started: bool,
    /// An app with a window: whether one opened, and drew something.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draws: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    /// Its first start, with an empty cache (until its window, for an app).
    pub first_start_ms: Option<u64>,
    /// What went wrong, when it didn't (building the image, or the bundle's own output).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub problem: String,
    /// Its last lines of output.
    pub output: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Report<'a> {
    bundle: String,
    name: &'a str,
    version: &'a str,
    /// gui: a window looked for; cli: run with `args`.
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<&'a str>,
    results: &'a [Outcome],
    images: Vec<Image>,
    passed: bool,
}

#[derive(Debug, Serialize)]
struct Image {
    name: String,
    bytes: u64,
}

/// podman, or how to get it.
fn podman() -> Result<PathBuf> {
    let found = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("podman")).find(|p| p.is_file()));
    found.context(
        "nami bundle --test runs the bundle on each distribution in a container, with podman, which isn't here.\n\
         Get it with `nami install podman`, or add podman to the packages in your project.nami, then run this again.\n\
         It runs rootless: no root and no daemon needed.",
    )
}

/// Does the bundle open a window? Its menu entry says so (Terminal=false).
pub fn has_window(meta: &Meta) -> bool {
    meta.desktop.as_ref().is_some_and(|d| !d.file.lines().any(|l| l.trim() == "Terminal=true"))
}

/// Configuration rootless podman needs and a computer without the
/// distribution's podman package lacks (namiOS's comes from the catalog):
/// a policy that takes images from Docker Hub, kept in nami's own folder.
fn podman_env(cmd: &mut Command) -> Result<()> {
    let have_policy = Path::new("/etc/containers/policy.json").exists()
        || std::env::var_os("HOME").is_some_and(|h| Path::new(&h).join(".config/containers/policy.json").exists());
    if !have_policy {
        let dir = crate::bundle_publish::key_file().parent().unwrap().join("containers");
        fs::create_dir_all(&dir)?;
        let policy = dir.join("policy.json");
        if !policy.exists() {
            fs::write(&policy, "{\"default\":[{\"type\":\"insecureAcceptAnything\"}]}\n")?;
        }
        cmd.arg("--signature-policy").arg(policy);
    }
    Ok(())
}

fn image_name(distro: &str) -> String {
    format!("localhost/nami-bundle-test-{}", distro.replace([':', '/', '.'], "-"))
}

/// The distro's image with a desktop's packages: built once, then kept.
fn ensure_image(podman: &Path, distro: &str, base: &str, log: &mut dyn FnMut(&str)) -> Result<String> {
    let name = image_name(distro);
    let have = Command::new(podman).args(["image", "exists", &name]).status().is_ok_and(|s| s.success());
    if have {
        return Ok(name);
    }
    log(&format!("{distro}: making its image (once: it's kept, as {name})"));
    let mut pull = Command::new(podman);
    pull.arg("pull").arg("--quiet");
    podman_env(&mut pull)?;
    let out = pull.arg(base).stdout(Stdio::null()).stderr(Stdio::piped()).output().context("starting podman")?;
    if !out.status.success() {
        bail!("couldn't get {base}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    // As someone, not root (Electron won't run as root), with a home.
    let file = format!("FROM {base}\n{}\nRUN (userdel -r ubuntu || true) 2>/dev/null; useradd -m -u 1000 tester\n", desktop_packages(distro));
    // An empty folder as its context: the Containerfile needs nothing else.
    let context = std::env::temp_dir().join(format!("nami-bundle-test-{}", std::process::id()));
    fs::create_dir_all(&context)?;
    let mut build = Command::new(podman);
    build.args(["build", "--quiet", "--pull=never", "-t", &name, "-f", "-", "."]).current_dir(&context);
    let mut child = build.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().context("starting podman build")?;
    child.stdin.take().unwrap().write_all(file.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("building its image failed: {}", err.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
    }
    Ok(name)
}

/// The images' sizes, as podman keeps them.
fn image_sizes(podman: &Path, names: &[String]) -> Vec<Image> {
    let mut out = Vec::new();
    for n in names {
        let o = Command::new(podman).args(["image", "inspect", "--format", "{{.Size}}", n]).stderr(Stdio::null()).output();
        if let Some(bytes) = o.ok().filter(|o| o.status.success()).and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok()) {
            out.push(Image { name: n.clone(), bytes });
        }
    }
    out
}

/// `RESULT key=value ...` into an outcome.
fn parse_result(distro: &str, text: &str) -> Outcome {
    let mut o = Outcome { distribution: distro.into(), ..Default::default() };
    let mut output = Vec::new();
    let mut found = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("RESULT ") {
            found = true;
            for kv in rest.split_whitespace() {
                let Some((k, v)) = kv.split_once('=') else { continue };
                let yes = v == "yes";
                match k {
                    "distro" => o.pretty_name = v.replace('_', " "),
                    "glibc" => o.glibc = v.into(),
                    "started" => o.started = yes,
                    "window" => o.window = Some(yes),
                    "draws" if v != "-" => o.draws = Some(yes),
                    "exit" => o.exit = v.parse().ok(),
                    "first_ms" => o.first_start_ms = v.parse().ok(),
                    _ => {}
                }
            }
        } else if let Some(l) = line.strip_prefix("  | ") {
            output.push(l.to_string());
        }
    }
    if !found {
        o.problem = format!("the container didn't finish: {}", text.lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" / "));
    }
    o.output = output;
    o
}

/// One distribution: the image, then the bundle in a fresh container.
fn run_one(podman: &Path, distro: &str, base: &str, bundle: &Path, gui: bool, args: &str, log: &(dyn Fn(&str) + Sync)) -> Outcome {
    let image = match ensure_image(podman, distro, base, &mut |l| log(l)) {
        Ok(i) => i,
        Err(e) => return Outcome { distribution: distro.into(), problem: format!("{e:#}"), ..Default::default() },
    };
    let mut cmd = Command::new(podman);
    cmd.args(["run", "--rm", "--pull=never", "--network=host", "--userns=keep-id:uid=1000,gid=1000", "-u", "1000:1000", "--security-opt", "seccomp=unconfined", "--shm-size=1g"])
        .args(["-e", "HOME=/home/tester", "-e", &format!("NAMI_TEST_KIND={}", if gui { "gui" } else { "cli" }), "-e", &format!("NAMI_TEST_ARGS={args}")])
        .arg("-v")
        .arg(format!("{}:/b/app:ro", bundle.display()))
        .args(["--tmpfs", "/shots", &image, "dbus-run-session", "--", "sh", "-c", INNER]);
    let out = cmd.stdin(Stdio::null()).output();
    match out {
        Ok(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            text += &String::from_utf8_lossy(&o.stderr);
            parse_result(distro, &text)
        }
        Err(e) => Outcome { distribution: distro.into(), problem: format!("starting podman: {e}"), ..Default::default() },
    }
}

/// Did it pass there: it started, and (an app) opened a window.
fn passed(o: &Outcome) -> bool {
    o.problem.is_empty() && o.started && o.window != Some(false)
}

/// `nami bundle --test <bundle>`.
pub fn test(bundle: &Path, opts: &Options) -> Result<bool> {
    let bundle = fs::canonicalize(bundle).with_context(|| format!("{} isn't there", bundle.display()))?;
    let (_, meta) = format::read(&mut fs::File::open(&bundle)?).with_context(|| format!("{} isn't a bundle", bundle.display()))?;
    let podman = podman()?;
    let gui = has_window(&meta);
    let args = opts.args.clone().unwrap_or_else(|| if gui { String::new() } else { "--version".into() });
    let distros: Vec<(&str, &str)> = if opts.on.is_empty() {
        distros_here()
    } else {
        let mut v = Vec::new();
        for want in &opts.on {
            match DISTROS.iter().find(|(n, _)| n == want || n.split(':').next() == Some(want.as_str())) {
                Some(d) if crate::arch::ARM && X86_ONLY.contains(&d.0) => bail!("--test-on {want}: its image is for x86_64 computers alone, and this one is ARM"),
                Some(d) => v.push(*d),
                None => bail!("--test-on {want}: nami tests on {}", DISTROS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")),
            }
        }
        v
    };
    let say = |l: &str| {
        if !opts.json {
            eprintln!("{}", crate::style::err().dim(l));
        }
    };
    let what = if gui { "an app: its window, under X11 (Xvfb)".to_string() } else { format!("a command: run as `{} {args}`", bundle.file_name().unwrap_or_default().to_string_lossy()) };
    if !opts.json {
        eprintln!("{}", crate::style::err().dim(&format!("Testing {} {} on {} distributions, each in a clean container ({what})", meta.name, meta.version, distros.len())));
    }
    // Side by side: each is a container of its own.
    let results: Vec<std::sync::Mutex<Option<Outcome>>> = distros.iter().map(|_| std::sync::Mutex::new(None)).collect();
    let finished = std::sync::atomic::AtomicUsize::new(0);
    let count = distros.len();
    if !opts.json {
        crate::log::status(&format!("Testing on {} distributions", distros.len()));
    }
    std::thread::scope(|s| {
        for (i, (name, base)) in distros.iter().enumerate() {
            let (results, bundle, args, podman, say, finished) = (&results, &bundle, &args, &podman, &say, &finished);
            s.spawn(move || {
                let o = run_one(podman, name, base, bundle, gui, args, say);
                let n = finished.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if !opts.json {
                    crate::log::status(&format!("Testing on {count} distributions · {n} done"));
                }
                *results[i].lock().unwrap() = Some(o);
            });
        }
    });
    crate::log::settle();
    let results: Vec<Outcome> = results.into_iter().map(|m| m.into_inner().unwrap().unwrap_or_default()).collect();
    let names: Vec<String> = distros.iter().map(|(n, _)| image_name(n)).collect();
    let images = image_sizes(&podman, &names);
    let all = results.iter().all(passed);
    if opts.json {
        let r = Report { bundle: bundle.display().to_string(), name: &meta.name, version: &meta.version, kind: if gui { "gui" } else { "cli" }, args: (!gui).then_some(args.as_str()), results: &results, images, passed: all };
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        let p = crate::style::out();
        print!("{}", table(&results, gui, p));
        let total: u64 = images.iter().map(|i| i.bytes).sum();
        let failed: Vec<&str> = results.iter().filter(|o| !passed(o)).map(|o| o.distribution.as_str()).collect();
        let said = if failed.is_empty() {
            format!("✓ {} {} runs on all {} distributions", meta.name, meta.version, results.len())
        } else {
            format!("✗ {} {} didn't pass on {} of {} · {}", meta.name, meta.version, failed.len(), results.len(), failed.join(", "))
        };
        println!("\n{}", p.line(&said));
        println!(
            "{}",
            p.line(&format!(
                "  The {} test images are kept for next time · {} in podman's storage · podman image ls 'nami-bundle-test-*' lists them",
                images.len(),
                crate::tell::size(total)
            ))
        );
    }
    Ok(all)
}

/// The showcase's table, for one bundle.
fn table(results: &[Outcome], gui: bool, p: crate::style::Paint) -> String {
    let mark = |v: bool| if v { (p.added("✓"), "✓") } else { (p.removed("✗"), "✗") };
    let mut rows: Vec<Vec<(String, usize)>> = Vec::new();
    let head: Vec<&str> = if gui { vec!["", "Distribution", "glibc", "Starts", "Window", "Draws", "First start"] } else { vec!["", "Distribution", "glibc", "Starts", "Exit", "First start"] };
    for o in results {
        let name = if o.pretty_name.is_empty() { o.distribution.clone() } else { o.pretty_name.clone() };
        let first = o.first_start_ms.map(|m| format!("{:.1} s", m as f64 / 1000.0)).unwrap_or_else(|| "-".into());
        let cell = |(s, raw): (String, &str)| (s, raw.chars().count());
        let ok = passed(o);
        let mut row = vec![cell(mark(ok)), cell((if ok { name.clone() } else { p.bold(&name) }, &name)), cell((p.dim(&o.glibc), &o.glibc))];
        row.push(cell(mark(o.problem.is_empty() && o.started)));
        if gui {
            let shown = o.problem.is_empty() && o.started;
            row.push(cell(if shown { mark(o.window.unwrap_or(false)) } else { ("-".to_string(), "-") }));
            row.push(cell(match o.draws {
                Some(true) => mark(true),
                Some(false) => (p.changed("~ blank"), "~ blank"),
                None => ("-".to_string(), "-"),
            }));
        } else {
            let e = o.exit.map(|e| e.to_string()).unwrap_or_else(|| "-".into());
            let shown = if o.exit.is_some_and(|e| e != 0) { p.removed(&e) } else { e.clone() };
            row.push(cell((shown, &e)));
        }
        row.push(cell((p.dim(&first), &first)));
        rows.push(row);
    }
    let mut widths: Vec<usize> = head.iter().map(|h| h.len()).collect();
    for r in &rows {
        for (i, (_, w)) in r.iter().enumerate() {
            widths[i] = widths[i].max(*w);
        }
    }
    let mut out = String::new();
    let line = |cells: Vec<(String, usize)>, out: &mut String| {
        for (i, (s, w)) in cells.iter().enumerate() {
            out.push_str(s);
            if i + 1 < cells.len() {
                out.push_str(&" ".repeat(widths[i] - w + 2));
            }
        }
        out.push('\n');
    };
    line(head.iter().map(|h| (p.bold(h), h.len())).collect(), &mut out);
    for r in rows {
        line(r, &mut out);
    }
    // What went wrong, under the table.
    for o in results.iter().filter(|o| !passed(o)) {
        let name = if o.pretty_name.is_empty() { &o.distribution } else { &o.pretty_name };
        out.push_str(&format!("\n{}\n", p.bold(&format!("On {name}:"))));
        if !o.problem.is_empty() {
            out.push_str(&format!("  {}\n", o.problem));
        }
        for l in o.output.iter().rev().take(8).collect::<Vec<_>>().into_iter().rev() {
            out.push_str(&format!("  {}\n", p.dim(l)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_container_says() {
        let text = "  | myapp 1.0\nRESULT distro=Ubuntu_24.04.3_LTS glibc=2.39 started=yes window=yes draws=yes colours=123 first_ms=1840 title=Hello\n";
        let o = parse_result("ubuntu:24.04", text);
        assert_eq!(o.pretty_name, "Ubuntu 24.04.3 LTS");
        assert_eq!(o.glibc, "2.39");
        assert!(o.started && o.window == Some(true) && o.draws == Some(true));
        assert_eq!(o.first_start_ms, Some(1840));
        assert_eq!(o.output, vec!["myapp 1.0"]);
        assert!(passed(&o));
        let cli = parse_result("arch", "RESULT distro=Arch_Linux glibc=2.42 started=no exit=127 first_ms=12\n");
        assert!(!cli.started && cli.window.is_none() && cli.exit == Some(127) && !passed(&cli));
        let broken = parse_result("fedora", "Error: no space left\n");
        assert!(!broken.problem.is_empty() && !passed(&broken));
    }

    #[test]
    fn prints_the_showcase_table() {
        let a = Outcome { distribution: "ubuntu:20.04".into(), pretty_name: "Ubuntu 20.04.6 LTS".into(), glibc: "2.31".into(), started: true, window: Some(true), draws: Some(false), first_start_ms: Some(2100), ..Default::default() };
        let b = Outcome { distribution: "arch".into(), pretty_name: "Arch Linux".into(), glibc: "2.42".into(), started: true, window: Some(true), draws: Some(true), first_start_ms: Some(900), ..Default::default() };
        let t = table(&[a, b], true, crate::style::Paint(false));
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], "   Distribution        glibc  Starts  Window  Draws    First start");
        assert_eq!(lines[1], "✓  Ubuntu 20.04.6 LTS  2.31   ✓       ✓       ~ blank  2.1 s");
        assert_eq!(lines[2], "✓  Arch Linux          2.42   ✓       ✓       ✓        0.9 s");
    }

    #[test]
    fn a_window_or_a_command() {
        let mut m = Meta::default();
        assert!(!has_window(&m));
        m.desktop = Some(format::Desktop { id: "a.desktop".into(), file: "[Desktop Entry]\nExec=a\nTerminal=false\n".into(), icon: None });
        assert!(has_window(&m));
        m.desktop.as_mut().unwrap().file = "[Desktop Entry]\nExec=a\nTerminal=true\n".into();
        assert!(!has_window(&m));
    }

    #[test]
    fn every_distribution_has_its_packages() {
        for (name, base) in DISTROS {
            assert!(desktop_packages(name).contains("Xvf") || desktop_packages(name).contains("xvfb"), "{name}");
            assert!(base.starts_with("docker.io/"), "{base}: named in full, so no registries.conf is needed");
        }
    }
}
