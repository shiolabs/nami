//! A nami bundle (`nami bundle`'s one-file apps) as an app of namiOS's:
//! opened in Files or from the browser's downloads it shows in Settings, as
//! a .deb does (src/foreign.rs, gui/src/pages/install_file.rs): its name,
//! icon, version, developer and size, read from its description without
//! starting it; what it is (a program from the internet nobody checked for
//! namiOS), and who signs its updates. Install copies it to
//! ~/.local/share/nami/apps/<id>/, a program, and puts what its desktop
//! file says where the person's apps go: its entry in the launcher (the
//! name the bundle's own `--nami-bundle-install` uses, src/bundle_menu.rs),
//! its icon, the kinds of file and links it opens. A command (jq) goes on
//! PATH instead (~/.local/bin). `nami remove` takes all that off, the
//! file too, and has the bundles' shared store (~/.cache/tray-bundle) let
//! go of the parts no other bundle needs; the app's own data stays.
//!
//! Updates are the bundle's own (src/bundle_update.rs): it looks once a day
//! as it starts, for a version its developer signed. Settings shows what
//! its last look found; this computer's daily-update switch turns that off
//! too (`bundle_update::system_says_off`).
//!
//! A bundle is told by what's in it, not its name: its trailer, and the
//! mark in its first bytes (bundle_format::ELF_MARK) for desktops, whose
//! kind for it is application/x-tray-bundle (namios-files).

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::bundle_format::{self as format, Meta, PathEntry, Trailer, STORE};
use crate::bundle_menu as menu;
use crate::bundle_update as update;
use crate::foreign::{self, Looked, Places, Record, Source, Warning};

/// `nami install <this>`, `nami package-info <this>`: a bundle's file.
pub fn is_bundle_file(arg: &str) -> bool {
    let p = Path::new(arg);
    p.is_file() && format::is_bundle(p)
}

/// What's kept in app.json beside the record every app from a download has.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Kept {
    /// The bundle, where it's installed.
    pub file: String,
    /// Its desktop file's id (org.gnome.TextEditor), if it's an app.
    #[serde(default)]
    pub desktop_id: Option<String>,
    /// Where its developer publishes updates, and their key.
    #[serde(default)]
    pub feed: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    /// Its entry in the person's user.nami (`bundle:github:ada/myapp`),
    /// when it's kept there: taken off when the entry is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AppJson {
    #[serde(flatten)]
    record: Record,
    #[serde(default)]
    bundle: Kept,
}

fn read_app(places: &Places, id: &str) -> Option<AppJson> {
    serde_json::from_str(&fs::read_to_string(places.apps().join(id).join("app.json")).ok()?).ok()
}

struct Opened {
    path: PathBuf,
    trailer: Trailer,
    meta: Meta,
}

fn open_bundle(file: &Path) -> Result<Opened> {
    let path = fs::canonicalize(file).with_context(|| format!("{}", file.display()))?;
    let mut f = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let (trailer, meta) = format::read(&mut f).with_context(|| format!("{}", path.display()))?;
    Ok(Opened { path, trailer, meta })
}

// ---------------------------------------------------------------------------
// What's in a part, without unpacking it: its NAR, read as it's decompressed.

#[derive(Debug, Clone, PartialEq)]
enum Picked {
    File(Vec<u8>),
    Link(String),
}

const MOST_PICKED: u64 = 8 << 20;

fn read_u64(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn skip(r: &mut impl Read, n: u64) -> io::Result<()> {
    let got = io::copy(&mut r.take(n), &mut io::sink())?;
    if got != n {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "a part cut short"));
    }
    Ok(())
}

fn pad(n: u64) -> u64 {
    (8 - n % 8) % 8
}

/// A NAR string's bytes: read if `keep` and small enough, else skipped (None).
fn read_bytes(r: &mut impl Read, keep: bool) -> io::Result<Option<Vec<u8>>> {
    let n = read_u64(r)?;
    let out = if keep && n <= MOST_PICKED {
        let mut b = vec![0u8; n as usize];
        r.read_exact(&mut b)?;
        Some(b)
    } else {
        skip(r, n)?;
        None
    };
    skip(r, pad(n))?;
    Ok(out)
}

fn read_str(r: &mut impl Read) -> io::Result<String> {
    let n = read_u64(r)?;
    if n > 4096 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a name too long for a part"));
    }
    let mut b = vec![0u8; n as usize];
    r.read_exact(&mut b)?;
    skip(r, pad(n))?;
    String::from_utf8(b).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a name that isn't text"))
}

fn expect(r: &mut impl Read, want: &str) -> io::Result<()> {
    let got = read_str(r)?;
    if got != want {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("not a part: {got:?} where {want:?} goes")));
    }
    Ok(())
}

fn nar_node(r: &mut impl Read, at: &str, want: &dyn Fn(&str) -> bool, out: &mut BTreeMap<String, Picked>, depth: usize) -> io::Result<()> {
    if depth > 64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a part nested too deep"));
    }
    expect(r, "(")?;
    expect(r, "type")?;
    match read_str(r)?.as_str() {
        "regular" => {
            let mut tag = read_str(r)?;
            if tag == "executable" {
                expect(r, "")?;
                tag = read_str(r)?;
            }
            if tag != "contents" {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "a file without contents"));
            }
            if let Some(b) = read_bytes(r, want(at))? {
                out.insert(at.to_string(), Picked::File(b));
            }
            expect(r, ")")
        }
        "symlink" => {
            expect(r, "target")?;
            let t = read_str(r)?;
            if want(at) {
                out.insert(at.to_string(), Picked::Link(t));
            }
            expect(r, ")")
        }
        "directory" => loop {
            match read_str(r)?.as_str() {
                ")" => return Ok(()),
                "entry" => {
                    expect(r, "(")?;
                    expect(r, "name")?;
                    let name = read_str(r)?;
                    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "a name a part mustn't have"));
                    }
                    expect(r, "node")?;
                    let child = if at.is_empty() { name } else { format!("{at}/{name}") };
                    nar_node(r, &child, want, out, depth + 1)?;
                    expect(r, ")")?;
                }
                t => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("not a part: {t:?} in a folder"))),
            }
        },
        t => Err(io::Error::new(io::ErrorKind::InvalidData, format!("not a part: a {t:?}"))),
    }
}

/// The files (and links) of a NAR that `want` picks, by their path in it.
fn pick_nar(r: &mut impl Read, want: &dyn Fn(&str) -> bool) -> io::Result<BTreeMap<String, Picked>> {
    expect(r, "nix-archive-1")?;
    let mut out = BTreeMap::new();
    nar_node(r, "", want, &mut out, 0)?;
    Ok(out)
}

/// From the part `base` of the bundle: the files `want` picks.
fn pick(b: &Opened, base: &str, want: &dyn Fn(&str) -> bool) -> Result<BTreeMap<String, Picked>> {
    let p: &PathEntry = b.meta.paths.iter().find(|p| p.name == base).ok_or_else(|| anyhow!("no part {base}"))?;
    ensure!(!p.fetch, "{base} isn't in the file (it's fetched as it first starts)");
    let mut f = File::open(&b.path)?;
    f.seek(SeekFrom::Start(p.offset))?;
    let mut z = zstd::stream::read::Decoder::new(f.take(p.size))?;
    Ok(pick_nar(&mut z, want)?)
}

/// `/nix/store/<base>/<rest>` -> (base, rest).
fn split_store(path: &str) -> Option<(String, String)> {
    let p = path.strip_prefix(STORE)?.trim_start_matches('/');
    let (base, rest) = p.split_once('/').unwrap_or((p, ""));
    Some((base.to_string(), rest.to_string()))
}

/// `rest` with `.` and `..` resolved (None: above its part).
fn normal(rest: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for c in rest.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            c => out.push(c),
        }
    }
    Some(out.join("/"))
}

/// A file of the bundle's, by its store path: links followed (a few).
fn file_of(b: &Opened, path: &str) -> Option<Vec<u8>> {
    let (mut base, mut rest) = split_store(path)?;
    for _ in 0..6 {
        let want = rest.clone();
        match pick(b, &base, &|p| p == want).ok()?.remove(&rest)? {
            Picked::File(d) => return Some(d),
            Picked::Link(t) if t.starts_with('/') => (base, rest) = split_store(&t)?,
            Picked::Link(t) => rest = normal(&format!("{}/{t}", Path::new(&rest).parent().map(|p| p.display().to_string()).unwrap_or_default()))?,
        }
    }
    None
}

/// Its AppStream file's words (`share/metainfo/*.xml`): (developer, summary).
fn metainfo(b: &Opened) -> (Option<String>, Option<String>) {
    let mut bases: Vec<String> = Vec::new();
    for p in [Some(b.meta.entry.as_str()), b.meta.desktop.as_ref().and_then(|d| d.icon.as_deref())].into_iter().flatten() {
        if let Some((base, _)) = split_store(p) {
            if !bases.contains(&base) {
                bases.push(base);
            }
        }
    }
    for base in bases {
        let Ok(files) = pick(b, &base, &|p| (p.starts_with("share/metainfo/") || p.starts_with("share/appdata/")) && p.ends_with(".xml")) else { continue };
        for data in files.values() {
            let Picked::File(d) = data else { continue };
            let t = String::from_utf8_lossy(d);
            // (`<summary xml:lang="de">` is a translation: the untranslated one is `<summary>`.)
            let tag = |name: &str, within: &str| -> Option<String> {
                let at = within.find(&format!("<{name}>")).map(|i| i + name.len() + 2)?;
                let end = within[at..].find('<')?;
                let v = xml_text(within[at..at + end].trim());
                (!v.is_empty()).then_some(v)
            };
            let developer = tag("developer_name", &t).or_else(|| t.find("<developer").and_then(|i| tag("name", &t[i..])));
            let developer = developer.map(|d| d.split(" <").next().unwrap_or(&d).trim().to_string());
            let summary = t.find("<component").and_then(|i| tag("summary", &t[i..]));
            if developer.is_some() || summary.is_some() {
                return (developer, summary);
            }
        }
    }
    (None, None)
}

fn xml_text(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// An app's icon, found.
#[derive(Debug, Clone, PartialEq)]
enum Icon {
    /// A picture the bundle carries, and its path in it (its kind, png or
    /// svg, and whether it's symbolic, by its name).
    Carried(Vec<u8>, String),
    /// None it carries: this computer's icon theme's, by its name.
    System(PathBuf),
}

/// Its icon: the bundle's picture (as `nami bundle` found it), else one by
/// its name (`Icon=applications-games`) in the icon themes it carries, else
/// in this computer's icon theme.
fn icon_of(b: &Opened) -> Option<Icon> {
    let d = b.meta.desktop.as_ref()?;
    let picture = |icon: &str| -> Option<Icon> {
        let ext = Path::new(icon).extension()?.to_string_lossy().to_lowercase();
        ["png", "svg"].contains(&ext.as_str()).then_some(())?;
        Some(Icon::Carried(file_of(b, icon)?, icon.to_string()))
    };
    if let Some(i) = d.icon.as_deref().and_then(picture) {
        return Some(i);
    }
    let name = menu::entry_key(&d.file, "Icon").filter(|n| !n.is_empty() && !n.contains('/'))?;
    carried_icon(b, &name).and_then(|p| picture(&p)).or_else(|| system_icon(&name).map(Icon::System))
}

/// An icon by its name among the bundle's own icons: in its program's part
/// and those that are icon themes (the rest aren't read: they're big).
fn carried_icon(b: &Opened, name: &str) -> Option<String> {
    let own = split_store(&b.meta.entry).map(|(base, _)| base);
    let mut best: Option<(u32, String)> = None;
    for p in b.meta.paths.iter().filter(|p| !p.fetch && (Some(&p.name) == own.as_ref() || p.name.contains("icon"))) {
        let Ok(found) = pick(b, &p.name, &|rel| menu::icon_score(rel, name).is_some()) else { continue };
        for rel in found.keys() {
            let score = menu::icon_score(rel, name).unwrap_or(0);
            if best.as_ref().is_none_or(|b| score > b.0) {
                best = Some((score, format!("{STORE}/{}/{rel}", p.name)));
            }
        }
    }
    best.map(|b| b.1)
}

/// An icon by its name in this computer's icon theme (the person's, as GTK
/// has it, and those it inherits from; then hicolor and Adwaita): a full
/// colour one if any theme has it, else a symbolic one.
fn system_icon(name: &str) -> Option<PathBuf> {
    let mut data = vec![menu::data_home()];
    let dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    data.extend(dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    data.push(PathBuf::from("/run/current-system/sw/share"));
    data.dedup();
    let mut themes: Vec<String> = Vec::new();
    for gtk in ["gtk-4.0", "gtk-3.0"] {
        let ini = fs::read_to_string(menu::config_home().join(gtk).join("settings.ini")).unwrap_or_default();
        if let Some(t) = ini.lines().find_map(|l| l.trim().strip_prefix("gtk-icon-theme-name")).and_then(|v| v.trim().strip_prefix('=')) {
            let t = t.trim().trim_matches('"').to_string();
            if !themes.contains(&t) {
                themes.push(t);
            }
        }
    }
    for t in ["hicolor", "Adwaita"] {
        if !themes.iter().any(|x| x == t) {
            themes.push(t.to_string());
        }
    }
    let mut i = 0;
    while i < themes.len() && themes.len() < 16 {
        // (And what each inherits from.)
        let inherits = data.iter().find_map(|d| fs::read_to_string(d.join("icons").join(&themes[i]).join("index.theme")).ok()).and_then(|t| t.lines().find_map(|l| l.strip_prefix("Inherits=").map(str::to_string)));
        let mut at = i + 1;
        for t in inherits.unwrap_or_default().split(',').map(str::trim).filter(|t| !t.is_empty()) {
            if !themes.iter().any(|x| x == t) {
                themes.insert(at, t.to_string());
                at += 1;
            }
        }
        i += 1;
    }
    let sub = |d: &Path| -> Vec<String> { fs::read_dir(d).into_iter().flatten().flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect() };
    let files = [format!("{name}.png"), format!("{name}.svg"), format!("{name}-symbolic.svg"), format!("{name}-symbolic.png")];
    let mut symbolic: Option<PathBuf> = None;
    for theme in themes.iter().map(Some).chain([None]) {
        let mut best: Option<(u32, PathBuf)> = None;
        for d in &data {
            let mut look = |rel: String| {
                if let Some(score) = menu::icon_score(&rel, name) {
                    let f = d.join(&rel);
                    if best.as_ref().is_none_or(|b| score > b.0) && f.is_file() {
                        best = Some((score, f));
                    }
                }
            };
            match theme {
                Some(t) => {
                    let root = d.join("icons").join(t);
                    for a in sub(&root) {
                        for b in sub(&root.join(&a)) {
                            for f in &files {
                                look(format!("icons/{t}/{a}/{b}/{f}"));
                            }
                        }
                    }
                }
                None => {
                    for f in &files {
                        look(format!("pixmaps/{f}"));
                    }
                }
            }
        }
        match best {
            Some((score, f)) if score >= 4000 => return Some(f),
            Some((_, f)) if symbolic.is_none() => symbolic = Some(f),
            _ => {}
        }
    }
    symbolic
}

/// ~/.cache/nami/packages/bundle-<its hash>: its icon, for Settings to show.
fn cache_for(hash: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".cache"));
    cache.join("nami/packages").join(format!("bundle-{}", &hash[..16]))
}

// ---------------------------------------------------------------------------
// Looking at one.

/// The bundles' shared store the bundle with this hash uses: where its
/// own checks keep what they found. (`~/.cache/tray-bundle`, as its runtime
/// picks it: bundle_format::cache_dirs.)
fn bundle_cache(hash: &str) -> PathBuf {
    let dirs: Vec<PathBuf> = format::cache_dirs().into_iter().chain(format::cache_fallbacks()).collect();
    format::pick_cache(&dirs, Some(hash)).unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Its commands, for a bundle that isn't an app: its program's name, and the other names it answers to.
fn commands(meta: &Meta) -> Vec<String> {
    if meta.desktop.is_some() {
        return Vec::new();
    }
    let mut out: Vec<String> = vec![meta.entry.rsplit('/').next().unwrap_or(&meta.name).to_string()];
    for name in meta.programs.keys() {
        if !out.contains(name) && !name.is_empty() && !name.contains('/') && !name.starts_with('.') {
            out.push(name.clone());
        }
    }
    out.retain(|c| !c.is_empty());
    out
}

/// Where its updates come from, said as a place: `github.com/owner/repo`, a host.
fn feed_place(feed: &str) -> String {
    let rest = feed.trim_start_matches("https://").trim_start_matches("http://");
    if let Some(repo) = rest.strip_prefix("github.com/") {
        let mut parts = repo.split('/');
        if let (Some(o), Some(r)) = (parts.next(), parts.next()) {
            return format!("github.com/{o}/{r}");
        }
    }
    rest.split('/').next().unwrap_or(rest).to_string()
}

/// What Settings shows of a bundle beside what it shows of any download.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Extra {
    /// Its developer's key's name and fingerprint, if it updates itself.
    pub key: Option<String>,
    pub fingerprint: Option<String>,
    pub feed: Option<String>,
    /// "daily" (it updates itself), "off" (this computer's switch, or the
    /// person's for it), or "none" (it was made without updates).
    pub updates: String,
    /// What its own last look found, once it's installed.
    pub last_look: Option<update::LastLook>,
    /// Where it's installed.
    pub installed_at: Option<String>,
    /// Bytes of its parts this computer has already (unpacked by another
    /// bundle, or the same in its own /nix/store): what it'd add is `size` less this.
    pub here: u64,
    /// Who made it, as they call themselves (`bundle: developer:`).
    pub developer: Option<String>,
    /// Its key here: "unsigned", "new" (trusted once it's installed),
    /// "same" (the key trusted before), "changed" (another: refused).
    pub trust: String,
    /// The fingerprint of the key trusted before, when it changed.
    pub trusted_fingerprint: Option<String>,
    /// What its sandbox lets it use, in words ("Runs in a sandbox: it can
    /// use the network…", "Not sandboxed: it can use everything you can.").
    pub sandbox: String,
    /// It runs in a sandbox here (made so, and this computer can make one).
    pub sandboxed: bool,
}

/// What its sandbox is, said for this computer: (words, sandboxed here).
fn sandbox_here(m: &Meta) -> (String, bool) {
    let here = m.sandbox.is_some() && crate::bundle_sandbox::find_bwrap().is_some();
    (crate::bundle_sandbox::words(m.sandbox.as_ref(), here), here)
}

/// What of a bundle's parts this computer has already, in bytes: in the
/// bundles' shared store (as the runtime looks: by name), or in its own
/// /nix/store as the runtime would link to (by name here, with all they
/// refer to; the runtime checks each's hash too). Nothing's read but names.
fn parts_here(m: &Meta, shared: &Path) -> u64 {
    let host = (Path::new("/etc/NIXOS").exists() || Path::new("/run/current-system").exists() && Path::new("/run/opengl-driver").exists()) && Path::new(STORE).is_dir();
    let by_name: BTreeMap<&str, &PathEntry> = m.paths.iter().map(|p| (p.name.as_str(), p)).collect();
    let glibc = m.glibc.rsplit('/').next().unwrap_or("");
    fn on_host(name: &str, by_name: &BTreeMap<&str, &PathEntry>, seen: &mut BTreeMap<String, bool>) -> bool {
        if let Some(s) = seen.get(name) {
            return *s;
        }
        seen.insert(name.to_string(), false);
        let ok = by_name.get(name).is_some_and(|p| !p.nar_hash.is_empty() && Path::new(STORE).join(name).symlink_metadata().is_ok() && p.references.iter().all(|r| r == name || on_host(r, by_name, seen)));
        seen.insert(name.to_string(), ok);
        ok
    }
    let mut seen = BTreeMap::new();
    m.paths
        .iter()
        .filter(|p| shared.join("store").join(&p.name).symlink_metadata().is_ok() || host && p.name != glibc && on_host(&p.name, &by_name, &mut seen))
        .map(|p| p.nar_size)
        .sum()
}

/// Look at a bundle: as `foreign::look` does a download.
pub fn look(places: &Places, file: &Path) -> Result<(Looked, Extra)> {
    let b = open_bundle(file)?;
    let hash = b.trailer.hash();
    let m = &b.meta;
    let desktop = m.desktop.as_ref();
    let name = desktop.and_then(|d| menu::entry_key(&d.file, "Name")).filter(|n| !n.is_empty()).unwrap_or_else(|| m.name.clone());
    let (developer, about) = metainfo(&b);
    // Who made it: what the bundle says (signed with its updates), else its AppStream file.
    let developer = m.developer.clone().filter(|d| !d.trim().is_empty()).or(developer);
    let id = foreign::slug(&m.name);
    let mut l = Looked {
        file: b.path.display().to_string(),
        format: None,
        made_for: "Any Linux".into(),
        id: id.clone(),
        package: m.name.clone(),
        name: name.clone(),
        summary: desktop.and_then(|d| menu::entry_key(&d.file, "Comment")).or(about).filter(|s| *s != name),
        version: m.version.clone(),
        publisher: developer,
        size: m.paths.iter().map(|p| p.nar_size).sum(),
        download: fs::metadata(&b.path).map(|x| x.len()).unwrap_or(0),
        app: desktop.is_some(),
        commands: commands(m),
        ..Default::default()
    };
    match icon_of(&b) {
        Some(Icon::Carried(data, pic)) => {
            let ext = Path::new(&pic).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let to = cache_for(&hash).join(format!("icon{}.{ext}", if menu::is_symbolic(&pic) { "-symbolic" } else { "" }));
            if fs::create_dir_all(to.parent().unwrap()).is_ok() && fs::write(&to, data).is_ok() {
                l.icon = Some(to.display().to_string());
            }
        }
        Some(Icon::System(p)) => l.icon = Some(p.display().to_string()),
        None => {}
    }
    if let Some(d) = desktop {
        for k in menu::kinds(&d.file) {
            if let Some(s) = k.strip_prefix("x-scheme-handler/") {
                if !l.schemes.iter().any(|x| x == s) {
                    l.schemes.push(s.to_string());
                }
            }
        }
    }
    // What it is: a program from the internet, as a .deb is. (Before it's installed: once it
    // is, that's been decided.) And what its sandbox lets it do.
    let (sandbox, sandboxed) = sandbox_here(m);
    l.warnings.push(Warning {
        title: "A program from the internet".into(),
        // (What it can do once it's open is said below: its sandbox, or none.)
        text: format!("Nobody has checked {name} for namiOS: it comes as it is from whoever made it, like a .deb or an AppImage. Install it if you trust where you got it."),
        until_installed: true,
    });
    l.notes.push(sandbox.clone());
    let mut extra = Extra { updates: "none".into(), here: parts_here(m, &bundle_cache(&hash)), developer: m.developer.clone(), sandbox, sandboxed, trust: "unsigned".into(), ..Default::default() };
    // Its key: the one trusted for it before, or another (refused).
    match update::trust_in(&trust_dir(places), m) {
        crate::bundle_trust::Trust::Unsigned => {}
        crate::bundle_trust::Trust::New => extra.trust = "new".into(),
        crate::bundle_trust::Trust::Same => extra.trust = "same".into(),
        crate::bundle_trust::Trust::Changed { was } => {
            extra.trust = "changed".into();
            let (was_fp, now_fp) = (update::fingerprint(&was).unwrap_or_default(), m.update.as_ref().and_then(|u| update::fingerprint(&u.key)).unwrap_or_default());
            l.blocked = Some(format!(
                "This copy of {name} is signed with another key ({}) than the one you trusted before ({}). It may not be from {}, so it can't be installed. If its developer really changed their key, remove {name} first.",
                crate::bundle_trust::short(&now_fp),
                crate::bundle_trust::short(&was_fp),
                crate::bundle_trust::who(m)
            ));
            extra.trusted_fingerprint = Some(was_fp);
        }
    }
    if let Some(u) = &m.update {
        let place = feed_place(&u.feed);
        let fp = update::fingerprint(&u.key);
        extra.key = u.key.split(':').next().map(str::to_string);
        extra.fingerprint = fp.clone();
        extra.feed = Some(u.feed.clone());
        let off = update::system_says_off();
        extra.updates = if off || !update::wanted(m) { "off".into() } else { "daily".into() };
        l.updates = Some(Source { kind: "bundle".into(), url: u.feed.clone(), ..Default::default() });
        l.notes.push(format!(
            "It updates itself: once a day, as it starts, it looks for a newer version at {place}, and uses it from its next start. Each one must be signed by {} (key {}), so nobody else can change it.",
            crate::bundle_trust::who(m),
            fp.as_deref().map(crate::bundle_trust::short).unwrap_or_else(|| "unreadable".into())
        ));
        if off {
            l.notes.push("This computer's daily updates are off, so it doesn't look for updates either.".into());
        }
    } else {
        l.notes.push("It doesn't update itself: to update it, open a newer download of it.".into());
    }
    if m.paths.iter().any(|p| p.fetch) {
        l.notes.push("The first time it starts, it downloads the parts it leaves out from cache.nixos.org, each checked.".into());
    }
    if desktop.is_none() && l.commands.is_empty() {
        l.blocked = Some(format!("{name} has no program in it to install."));
    }
    // Installed already: which version (the installed file's own: it updates itself).
    if let Some(a) = read_app(places, &id) {
        l.installed = Some(a.record.version.clone());
        if a.record.format == "bundle" {
            if let Ok(i) = open_bundle(Path::new(&a.bundle.file)) {
                l.installed = Some(i.meta.version.clone());
                extra.installed_at = Some(i.path.display().to_string());
                if i.meta.update.is_some() {
                    let ihash = i.trailer.hash();
                    let cache = bundle_cache(&ihash);
                    extra.last_look = Some(update::Me { path: &i.path, meta: &i.meta, hash: &ihash, cache: &cache }.last_look());
                }
            }
        }
    }
    Ok((l, extra))
}

/// `nami package-info <bundle> --json`: a download's words, and the bundle's own.
pub fn info_json(places: &Places, file: &Path) -> Result<serde_json::Value> {
    let (l, extra) = look(places, file)?;
    let mut v = serde_json::to_value(&l)?;
    v["format"] = "bundle".into();
    v["bundle"] = serde_json::to_value(&extra)?;
    Ok(v)
}

/// `nami package-info <bundle>`, in words.
pub fn said(places: &Places, file: &Path) -> Result<String> {
    let (l, extra) = look(places, file)?;
    let mut out = foreign::said(&l).replacen(&format!("  Made for: {}", l.made_for), &format!("  Made for: {} (a nami bundle)", l.made_for), 1);
    if let Some(d) = &extra.developer {
        out += &format!("\nDeveloper: {d}\n");
    }
    if let (Some(k), Some(fp)) = (&extra.key, &extra.fingerprint) {
        out += &format!("\nUpdates are signed by {} with the key {fp} ({k})", extra.developer.as_deref().unwrap_or("its developer"));
        out += match extra.trust.as_str() {
            "same" => ": the key you trust for it.\n",
            "changed" => ": not the key you trusted for it before.\n",
            _ => ": trusted once it's installed.\n",
        };
    }
    out += &format!("{}\n", extra.sandbox);
    Ok(out)
}

/// `nami open-package <bundle>`: Settings shows it, to install.
pub fn open(file: &Path) -> Result<i32> {
    let file = fs::canonicalize(file).with_context(|| format!("{}", file.display()))?;
    let gui = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("nami-gui")).find(|f| f.is_file())).unwrap_or_else(|| PathBuf::from("nami-gui"));
    Command::new(gui).arg("--open-file").arg(&file).stdin(Stdio::null()).spawn().context("starting Settings (nami-gui)")?;
    Ok(0)
}

// ---------------------------------------------------------------------------
// Installing and removing.

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The bundle's file name in its folder: the download's, made safe.
fn file_name(path: &Path, id: &str) -> String {
    let n: String = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().chars().map(|c| if c.is_ascii_alphanumeric() || "-_.+".contains(c) { c } else { '-' }).collect();
    let n = n.trim_start_matches('.').to_string();
    if n.is_empty() || n == "app.json" { format!("{id}-x86_64") } else { n }
}

/// Install a bundle for this person: see the top of this file.
pub fn install(places: &Places, file: &Path) -> Result<Record> {
    ensure!(!crate::profile::is_root(), "installing a download is each person's own: run it as yourself, not as root");
    let b = open_bundle(file)?;
    let (looked, _) = look(places, &b.path)?;
    if let Some(why) = &looked.blocked {
        bail!("{why}");
    }
    let (name, id) = (looked.name.clone(), looked.id.clone());
    foreign::say(&format!("Checking {name}…"));
    format::verify(&mut File::open(&b.path)?, &b.trailer, &b.meta).with_context(|| format!("{} is damaged", b.path.display()))?;
    // Its developer's key: trusted from now on (a changed one was refused above, as it was looked at).
    update::settle_in(&trust_dir(places), &b.meta).with_context(|| format!("keeping {name}'s key"))?;
    let dir = places.apps().join(&id);
    let dest = dir.join(file_name(&b.path, &id));
    // What was there before: another version (taken off, its folder too), or the same app from another kind of download.
    if let Some(old) = read_app(places, &id) {
        if old.record.format == "bundle" {
            foreign::say(&format!("Taking off the {name} that was here ({})…", old.record.version));
            foreign::remove_outside(places, &old.record)?;
        } else {
            foreign::remove(places, &old.record)?;
        }
    }
    fs::create_dir_all(&dir)?;
    // (The installed one itself, opened again, stays where it is.)
    if fs::canonicalize(&dest).ok().as_deref() != Some(b.path.as_path()) {
        for e in fs::read_dir(&dir)?.flatten() {
            if e.file_name() != "app.json" && e.path().is_file() {
                let _ = fs::remove_file(e.path());
            }
        }
        foreign::say(&format!("Copying {name}…"));
        let tmp = dir.join(format!(".{}.new", dest.file_name().unwrap().to_string_lossy()));
        fs::copy(&b.path, &tmp).with_context(|| format!("copying {} to {}", b.path.display(), dir.display()))?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
        fs::rename(&tmp, &dest)?;
    } else {
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))?;
    }
    let rec = finish(places, &b, &dest, &looked)?;
    as_a_step(places);
    Ok(rec)
}

fn finish(places: &Places, b: &Opened, dest: &Path, looked: &Looked) -> Result<Record> {
    let (name, id) = (looked.name.clone(), looked.id.clone());
    let m = &b.meta;
    let mut rec = Record {
        id: id.clone(),
        name: name.clone(),
        version: m.version.clone(),
        full_version: b.trailer.hash(),
        format: "bundle".into(),
        package: m.name.clone(),
        publisher: looked.publisher.clone(),
        summary: looked.summary.clone(),
        from: b.path.display().to_string(),
        sha256: b.trailer.hash(),
        installed: now(),
        size: looked.size,
        updates: looked.updates.clone(),
        commands: Vec::new(),
        ..Default::default()
    };
    let mut kept = Kept { file: dest.display().to_string(), feed: m.update.as_ref().map(|u| u.feed.clone()), key: m.update.as_ref().map(|u| u.key.clone()), ..Default::default() };
    let applications = places.data.join("applications");
    if let Some(d) = &m.desktop {
        let did = d.id.trim_end_matches(".desktop").to_string();
        kept.desktop_id = Some(did.clone());
        // Its icon, where the person's icons are.
        // (One of this computer's theme's: its entry names it, as it did.)
        let mut icon_name = None;
        match icon_of(b) {
            Some(Icon::Carried(data, pic)) => {
                if let Some(to) = menu::icon_file_in(&places.data, &did, &pic) {
                    fs::create_dir_all(to.parent().unwrap())?;
                    let _ = fs::remove_file(&to);
                    fs::write(&to, data)?;
                    rec.icon = Some(to.display().to_string());
                    rec.files.push(to.display().to_string());
                    icon_name = Some(to.display().to_string());
                }
            }
            Some(Icon::System(p)) => rec.icon = Some(p.display().to_string()),
            None => {}
        }
        // Its entry in the launcher (the same the bundle's --nami-bundle-install makes: this one in its place).
        let entry = applications.join(format!("{}.desktop", menu::entry_id_in(&places.data, &did)));
        let text = menu::menu_entry(&d.file, &dest.display().to_string(), icon_name.as_deref(), &did, &[("X-Tray-App", &id), ("X-Tray-Bundle", &m.name)]);
        fs::create_dir_all(&applications)?;
        fs::write(&entry, text)?;
        rec.files.push(entry.display().to_string());
        rec.main_entry = Some(entry.display().to_string());
        // The links it opens: its, where nothing else opens them.
        let links = menu::link_kinds(&d.file);
        if !links.is_empty() {
            let mimeapps = places.config.join("mimeapps.list");
            let text = fs::read_to_string(&mimeapps).unwrap_or_default();
            let entry_name = format!("{}.desktop", menu::entry_id_in(&places.data, &did));
            // (Those it's made the default for, for Remove to take back.)
            let taken: Vec<String> = links.iter().filter(|k| menu::defaults(&text, std::slice::from_ref(*k), &entry_name, true) != text).cloned().collect();
            if !taken.is_empty() {
                fs::create_dir_all(&places.config)?;
                fs::write(&mimeapps, menu::defaults(&text, &taken, &entry_name, true))?;
                rec.defaults = taken.into_iter().map(|k| (k, entry_name.clone())).collect();
            }
        }
    } else {
        // A command: on PATH, as the bundle (it runs what it's called).
        fs::create_dir_all(&places.bin)?;
        for c in &looked.commands {
            let link = places.bin.join(c);
            match link.symlink_metadata() {
                Ok(md) if md.file_type().is_symlink() && fs::read_link(&link).is_ok_and(|t| t.starts_with(places.apps().join(&id))) => {
                    fs::remove_file(&link)?;
                }
                Ok(_) => {
                    rec.notes.push(format!("{} is there already: it's left as it is", link.display()));
                    continue;
                }
                Err(_) => {}
            }
            std::os::unix::fs::symlink(dest, &link)?;
            rec.files.push(link.display().to_string());
            rec.commands.push(c.clone());
        }
    }
    // Unpacked now, so its first start is as quick as the rest.
    foreign::say(&format!("Getting {name} ready…"));
    let ready = Command::new(dest).arg(format::flag(dest, "extract")).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output();
    match &ready {
        Ok(o) if o.status.success() => {}
        Ok(o) => eprintln!("{name} isn't unpacked yet (it is as it first starts): {}", String::from_utf8_lossy(&o.stderr).trim()),
        Err(e) => eprintln!("{name} isn't unpacked yet (it is as it first starts): {e}"),
    }
    // Kept in the person's user.nami, so another computer of theirs (or this
    // one again) gets it from where it updates: a bundle that updates itself.
    if let Some(u) = &m.update {
        let entry = entry_for(&u.feed);
        match record_in_file(&user_file(places), &entry, update::fingerprint(&u.key).as_deref()) {
            Ok(()) => kept.listed = Some(entry),
            Err(e) => eprintln!("{name} isn't kept in your user.nami: {e:#}"),
        }
    }
    // Its record last: there, it's installed.
    let app = AppJson { record: rec.clone(), bundle: kept };
    let json = places.apps().join(&id).join("app.json");
    let tmp = json.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&app)?)?;
    fs::rename(&tmp, &json)?;
    foreign::refresh_caches(places);
    foreign::say(&format!("{name} is installed."));
    Ok(rec)
}

/// `nami remove <app>` of a bundle: what it put anywhere, its file, and the
/// parts of the shared store only it used. Its own data stays.
pub fn remove(places: &Places, rec: &Record) -> Result<()> {
    foreign::say(&format!("Removing {}…", rec.name));
    foreign::remove_outside(places, rec)?;
    let dir = places.apps().join(&rec.id);
    let kept = read_app(places, &rec.id).map(|a| a.bundle).unwrap_or_default();
    let file = Some(PathBuf::from(&kept.file)).filter(|f| !kept.file.is_empty() && f.starts_with(&dir)).or_else(|| {
        fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).find(|p| p.is_file() && format::is_bundle(p))
    });
    if let Some(file) = file.filter(|f| f.is_file()) {
        // Its checks' notes, for this copy.
        if let Ok(b) = open_bundle(&file) {
            // Removed on purpose: the key of the next copy installed is trusted anew.
            let _ = fs::remove_file(trust_dir(places).join(crate::bundle_trust::app(&b.meta)));
            if b.meta.update.is_some() {
                let hash = b.trailer.hash();
                let cache = bundle_cache(&hash);
                for f in (update::Me { path: &b.path, meta: &b.meta, hash: &hash, cache: &cache }).state_files() {
                    let _ = fs::remove_file(f);
                }
            }
        }
        // Gone from where it was, it cleans the shared store: what no other bundle uses goes.
        let leaving = dir.join(".removing");
        fs::rename(&file, &leaving)?;
        let _ = fs::set_permissions(&leaving, fs::Permissions::from_mode(0o755));
        let cleaned = Command::new(&leaving).arg(format::flag(&leaving, "clean")).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        if !cleaned.is_ok_and(|s| s.success()) {
            eprintln!("(the bundles' shared store keeps its parts until another bundle cleans it)");
        }
    }
    if dir.exists() {
        crate::store::remove_tree(&dir)?;
    }
    // Off the person's user.nami too.
    if let Some(entry) = &kept.listed {
        if let Err(e) = unrecord_in_file(&user_file(places), entry) {
            eprintln!("{} is still in your user.nami: {e:#}", rec.name);
        }
    }
    foreign::say(&format!("{} is removed.", rec.name));
    as_a_step(places);
    Ok(())
}

// ---------------------------------------------------------------------------
// In the person's user.nami: `- bundle:github:ada/myapp: { key: 3F2A … }`,
// as a Flathub app is `- flathub:<id>`. Applying the file installs what's
// listed and isn't here (from where it updates, checked against the key),
// and takes off what was listed and no longer is.

/// The person's own file (~/user.nami), for the home `places` are in.
fn user_file(places: &Places) -> PathBuf {
    match places.bin.parent().and_then(Path::parent) {
        Some(home) => crate::config::user_file_in(home),
        None => crate::config::user_file(),
    }
}

/// Where the keys trusted for their apps are kept (bundle_trust::dir, for `places`).
fn trust_dir(places: &Places) -> PathBuf {
    places.config.join("nami-bundle/trusted")
}

/// How a bundle is written in `packages:`.
pub const PREFIX: &str = "bundle:";

pub fn is_entry(p: &str) -> bool {
    p.starts_with(PREFIX)
}

/// The entry for a bundle that updates from `feed`: `bundle:github:ada/myapp`
/// for a GitHub repository's latest release, else `bundle:<the feed's URL>`.
pub fn entry_for(feed: &str) -> String {
    let f = feed.trim_end_matches('/');
    let github = f.strip_prefix("https://github.com/").and_then(|r| r.strip_suffix("/releases/latest/download")).filter(|r| r.split('/').count() == 2);
    match github {
        Some(repo) => format!("{PREFIX}github:{repo}"),
        None => format!("{PREFIX}{f}"),
    }
}

/// An entry as nami reads the list (`bundle:github:ada/myapp@3F2A …`, its
/// key where the version goes): (where it updates from, the key's fingerprint).
pub fn parse_entry(e: &str) -> Option<(String, Option<String>)> {
    let rest = e.strip_prefix(PREFIX)?;
    let (source, key) = match rest.rsplit_once('@') {
        Some((s, k)) if !k.contains('/') => (s, Some(k.trim().to_string())),
        _ => (rest, None),
    };
    let ok = source.strip_prefix("github:").map(|r| r.split('/').count() == 2 && !r.contains(' ')).unwrap_or(source.starts_with("https://") || source.starts_with("http://"));
    ok.then(|| (source.to_string(), key.filter(|k| !k.is_empty())))
}

/// A fingerprint as compared: its hex digits alone, in capitals.
fn fp_norm(f: &str) -> String {
    f.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_uppercase()
}

/// `entry` (`bundle:…`) added to the file's packages, with its key, unless it's there.
pub fn record_in_file(file: &Path, entry: &str, fingerprint: Option<&str>) -> Result<()> {
    let mut text = fs::read_to_string(file).unwrap_or_default();
    if let Some((_, written)) = crate::config::listed_packages(&text).into_iter().find(|(have, _)| have == entry) {
        // There: with this key (or none said), as it is; with the key it handed over from, the new one.
        let had = written.get(entry).and_then(|s| s.get("key")).and_then(|k| k.as_str()).map(fp_norm);
        if fingerprint.is_none() || had.is_none() || had == fingerprint.map(fp_norm) {
            return Ok(());
        }
        text = crate::config_edit::apply(&text, &crate::config_edit::Op::Remove { path: vec!["packages".into()], value: written })?;
    }
    let value = match fingerprint {
        Some(fp) => serde_yaml::Value::Mapping(serde_yaml::Mapping::from_iter([(entry.into(), serde_yaml::Value::Mapping(serde_yaml::Mapping::from_iter([("key".into(), fp.into())])))])),
        None => entry.into(),
    };
    let new = crate::config_edit::apply(&text, &crate::config_edit::Op::Add { path: vec!["packages".into()], value })?;
    write_file(file, &new)
}

/// `entry` taken off the file's packages.
pub fn unrecord_in_file(file: &Path, entry: &str) -> Result<()> {
    let Ok(mut text) = fs::read_to_string(file) else { return Ok(()) };
    let ops = crate::config::remove_package(&text, entry);
    if ops.is_empty() {
        return Ok(());
    }
    for op in &ops {
        text = crate::config_edit::apply(&text, op)?;
    }
    write_file(file, &text)
}

fn write_file(file: &Path, text: &str) -> Result<()> {
    if let Some(d) = file.parent() {
        fs::create_dir_all(d)?;
    }
    let tmp = file.with_file_name(format!(".{}.tmp{}", file.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    fs::write(&tmp, text)?;
    // (A link, as ~/user.nami to a config repo's checkout is, stays one.)
    let target = fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    fs::rename(&tmp, &target).with_context(|| format!("writing {}", target.display()))
}

/// The bundles installed here, by the entry they're kept under.
fn listed_here(places: &Places) -> Vec<(Record, Kept)> {
    foreign::records(places).into_iter().filter(|r| r.format == "bundle").filter_map(|r| read_app(places, &r.id).map(|a| (r, a.bundle))).collect()
}

// Bundles are steps of the profile's history, as Flathub's apps are
// (src/flathub.rs): an entry `bundle:github:ada/myapp@<key>` with no store
// paths, its version in its name. Applying the file installs what's listed
// and isn't here; a step that drops one takes it off and going back to a
// step that had it puts it back (`follow`), so `nami undo` and Go Back in
// Time take a bundle's install or removal back too.

/// Set while nami applies a file or follows a step: what's installed or
/// removed then doesn't apply the file again.
static APPLYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Applying(bool);

impl Applying {
    fn start() -> Applying {
        Applying(APPLYING.swap(true, std::sync::atomic::Ordering::SeqCst))
    }
}

impl Drop for Applying {
    fn drop(&mut self) {
        APPLYING.store(self.0, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A bundle installed or removed by itself (Settings, `nami install`,
/// `nami remove`): the person's file applied, so it's a step of their
/// history (undone with `nami undo`). Only for this person's own places,
/// and not while nami is applying already.
fn as_a_step(places: &Places) {
    if APPLYING.load(std::sync::atomic::Ordering::SeqCst) || std::env::var_os("NAMI_BUNDLE_NO_STEP").is_some() || places.data != Places::here().data {
        return;
    }
    let file = user_file(places);
    let Ok(nami) = std::env::current_exe() else { return };
    let ok = Command::new(nami).arg("apply").arg(&file).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output();
    if !ok.as_ref().is_ok_and(|o| o.status.success()) {
        eprintln!("(applying {} for your history didn't work: it's a step of it from your next apply)", file.display());
    }
}

/// The source of an entry, without its key: what's compared between steps.
fn source_of(entry: &str) -> Option<String> {
    parse_entry(entry).map(|(s, _)| s)
}

/// A profile entry: the line as nami reads it, its version in its name.
pub fn entry(listed: &str, version: Option<&str>) -> crate::profile::Entry {
    let base = format!("{PREFIX}{}", source_of(listed).unwrap_or_else(|| listed.trim_start_matches(PREFIX).to_string()));
    let name = match version.filter(|v| !v.is_empty()) {
        Some(v) => format!("{base}-{v}"),
        None => base,
    };
    crate::profile::Entry { attr: listed.to_string(), name, paths: Vec::new() }
}

/// The installed bundle kept under `source`, if it's here.
fn here_from<'a>(here: &'a [(Record, Kept)], source: &str) -> Option<&'a (Record, Kept)> {
    let (entry, feed) = (format!("{PREFIX}{source}"), update::feed_url(source));
    here.iter().find(|(_, k)| k.listed.as_deref() == Some(entry.as_str()) || k.feed.as_deref() == Some(feed.as_str()))
}

/// The entries the listed bundles would have, changing nothing (a dry run).
pub fn entries(places: &Places, listed: &[String]) -> Vec<crate::profile::Entry> {
    let here = listed_here(places);
    listed.iter().map(|l| entry(l, source_of(l).and_then(|s| here_from(&here, &s)).map(|(r, _)| r.version.as_str()))).collect()
}

/// The bundles listed, installed (what's missing comes now, from where it
/// updates, checked against its key): the entries of those that are here,
/// and why the others aren't, so the rest of the file still applies.
pub fn install_listed(places: &Places, listed: &[String]) -> (Vec<crate::profile::Entry>, Vec<(String, String)>) {
    let _applying = Applying::start();
    let (mut out, mut left) = (Vec::new(), Vec::new());
    for l in listed {
        let Some((source, key)) = parse_entry(l) else {
            left.push((l.clone(), "a bundle is written bundle:github:<owner>/<repo>, or bundle:https://<the folder it updates from>".into()));
            continue;
        };
        let here = listed_here(places);
        if let Some((rec, kept)) = here_from(&here, &source) {
            // Here already: kept under its entry from now on.
            let entry_name = format!("{PREFIX}{source}");
            if kept.listed.as_deref() != Some(entry_name.as_str()) {
                if let Some(mut a) = read_app(places, &rec.id) {
                    a.bundle.listed = Some(entry_name);
                    if let Ok(j) = serde_json::to_vec_pretty(&a) {
                        let _ = fs::write(places.apps().join(&rec.id).join("app.json"), j);
                    }
                }
            }
            // It updated itself to a key the listed one handed over to: the file says the new one.
            let installed = installed_bundle(places, rec);
            if let (Some(want), Some(u)) = (&key, installed.as_ref().and_then(|b| b.meta.update.clone())) {
                let fp = update::fingerprint(&u.key).unwrap_or_default();
                if fp_norm(want) != fp_norm(&fp) && update::rotated_from_fingerprint(want, &u.key, &u.rotations) {
                    if let Err(e) = record_in_file(&user_file(places), &format!("{PREFIX}{source}"), Some(&fp)) {
                        eprintln!("{}'s new key isn't in your user.nami: {e:#}", rec.name);
                    }
                }
            }
            let version = installed.map(|b| b.meta.version).unwrap_or_else(|| rec.version.clone());
            out.push(entry(l, Some(&version)));
            continue;
        }
        match install_source(places, &source, key.as_deref(), false) {
            Ok(rec) => out.push(entry(l, Some(&rec.version))),
            Err(e) => left.push((l.clone(), format!("{e:#}"))),
        }
    }
    (out, left)
}

/// The person's profile went from `before` to `after` (an apply, `nami
/// undo`, Go Back in Time): the bundles kept in user.nami that it dropped
/// come off, and those it has that aren't here go on. Said, not failed:
/// the rest of the step stands.
pub fn follow(places: &Places, before: &[crate::profile::Entry], after: &[crate::profile::Entry]) {
    let sources = |es: &[crate::profile::Entry]| -> Vec<(String, String)> { es.iter().filter(|e| is_entry(&e.attr)).filter_map(|e| source_of(&e.attr).map(|s| (s, e.attr.clone()))).collect() };
    let (was, now) = (sources(before), sources(after));
    if was.is_empty() && now.is_empty() {
        return;
    }
    let _applying = Applying::start();
    let here = listed_here(places);
    for (source, _) in was.iter().filter(|(s, _)| !now.iter().any(|(n, _)| n == s)) {
        // Only what was kept in the file: never one installed some other way.
        let entry_name = format!("{PREFIX}{source}");
        if let Some((rec, _)) = here.iter().find(|(_, k)| k.listed.as_deref() == Some(entry_name.as_str())) {
            if let Err(e) = remove(places, rec) {
                crate::say!("warning: taking off {}: {e:#}", rec.name);
            }
        }
    }
    for (source, attr) in &now {
        if here_from(&here, source).is_none() {
            let key = parse_entry(attr).and_then(|(_, k)| k);
            if let Err(e) = install_source(places, source, key.as_deref(), false) {
                crate::say!("warning: putting back {PREFIX}{source}: {e:#}");
            }
        }
    }
}

/// A bundle from where it updates (`github:ada/myapp`, or its feed's URL):
/// its signed feed read, the description it signs checked against the key
/// (`fingerprint`, the one in user.nami) and the key trusted here, the
/// file downloaded and checked part by part, then installed as a download
/// is. `record`: kept in user.nami too (`nami install bundle:…`).
pub fn install_source(places: &Places, source: &str, fingerprint: Option<&str>, record: bool) -> Result<Record> {
    let feed_url = update::feed_url(source);
    ensure!(feed_url.starts_with("https://") || feed_url.starts_with("http://"), "{source}: a bundle comes from github:<owner>/<repo> or an https:// folder");
    let agent = ureq::AgentBuilder::new().timeout_connect(std::time::Duration::from_secs(15)).timeout_read(std::time::Duration::from_secs(120)).user_agent(concat!("nami/", env!("CARGO_PKG_VERSION"))).build();
    let get = |url: &str, most: u64| -> Result<Vec<u8>> {
        let mut data = Vec::new();
        agent.get(url).call().with_context(|| format!("fetching {url}"))?.into_reader().take(most + 1).read_to_end(&mut data)?;
        ensure!(data.len() as u64 <= most, "{url} is bigger than it may be");
        Ok(data)
    };
    foreign::say(&format!("Looking at {source}…"));
    let feed: update::Feed = serde_json::from_slice(&get(&format!("{feed_url}/{}", update::FEED), 1 << 20)?).with_context(|| format!("reading {feed_url}/{}", update::FEED))?;
    let meta_bytes = get(&format!("{feed_url}/{}", update::meta_file(&feed.sha256)), 64 << 20)?;
    ensure!(format::sha256_hex(&meta_bytes) == feed.sha256, "{feed_url}: its description isn't the one its feed signed");
    let meta: Meta = serde_json::from_slice(&meta_bytes).context("reading its description")?;
    let u = meta.update.as_ref().with_context(|| format!("{source} isn't a bundle that updates itself"))?;
    feed.verify(&u.key).with_context(|| format!("{feed_url}: refused"))?;
    ensure!(feed.name == meta.name, "{feed_url}: its feed is for {}, its description for {}", feed.name, meta.name);
    let fp = update::fingerprint(&u.key).unwrap_or_default();
    if let Some(want) = fingerprint {
        ensure!(fp_norm(want) == fp_norm(&fp) || update::rotated_from_fingerprint(want, &u.key, &u.rotations), "{} at {source} is signed with the key {}, not the one your user.nami trusts ({}): it wasn't installed", meta.name, crate::bundle_trust::short(&fp), crate::bundle_trust::short(want));
    }
    if let crate::bundle_trust::Trust::Changed { was } = update::trust_in(&trust_dir(places), &meta) {
        bail!("{}", crate::bundle_trust::refusal(&meta, &update::fingerprint(&was).unwrap_or_default(), &fp, &format!("remove {} first, then install it again", meta.name)));
    }
    // The file, beside where it goes, checked against the signed description.
    let downloads = places.apps().join(".downloads");
    fs::create_dir_all(&downloads)?;
    let file = downloads.join(file_name(Path::new(&feed.file), &foreign::slug(&meta.name)));
    let url = format!("{feed_url}/{}", feed.file);
    foreign::say(&format!("Downloading {} {}…", meta.name, meta.version));
    let result = (|| -> Result<Record> {
        let mut r = agent.get(&url).call().with_context(|| format!("fetching {url}"))?.into_reader().take(feed.size + 1);
        let mut f = File::create(&file)?;
        let n = io::copy(&mut r, &mut f)?;
        ensure!(n == feed.size, "{url} is {n} bytes, not the {} its feed says", feed.size);
        drop(f);
        let b = open_bundle(&file)?;
        ensure!(b.trailer.hash() == feed.sha256, "{url} isn't the version its feed signed");
        let rec = install(places, &file)?;
        if !record {
            // (Applying the file: it's listed there already.)
            if let Some(mut a) = read_app(places, &rec.id) {
                a.bundle.listed = Some(format!("{PREFIX}{source}"));
                fs::write(places.apps().join(&rec.id).join("app.json"), serde_json::to_vec_pretty(&a)?)?;
            }
        }
        Ok(rec)
    })();
    let _ = fs::remove_file(&file);
    result
}

// ---------------------------------------------------------------------------
// Its updates, among the other apps from downloads' (src/foreign_update.rs):
// the bundle's own, with the same switches.

/// The installed bundle, opened.
fn installed_bundle(places: &Places, rec: &Record) -> Option<Opened> {
    let kept = read_app(places, &rec.id)?.bundle;
    open_bundle(Path::new(&kept.file)).ok()
}

/// A newer version its own last look found (`nami file-apps --updates`).
pub fn newer_version(places: &Places, rec: &Record) -> Option<String> {
    let b = installed_bundle(places, rec)?;
    b.meta.update.as_ref()?;
    let hash = b.trailer.hash();
    let cache = bundle_cache(&hash);
    update::Me { path: &b.path, meta: &b.meta, hash: &hash, cache: &cache }.last_look().newer
}

/// What the Updates page needs of an installed bundle (`nami file-apps
/// --json`, its `bundle`): where it updates from and who signs it, whether
/// it updates itself here, and what its own last look found (it looks as
/// it starts, once a day, and a newer version is used from its next start).
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct UpdateInfo {
    /// The version the file is now (it changes as it updates itself).
    pub version: String,
    pub feed: Option<String>,
    /// Its entry in user.nami, when it's kept there.
    pub listed: Option<String>,
    pub developer: Option<String>,
    pub fingerprint: Option<String>,
    /// "unsigned", "same" or "changed" (then it isn't updated).
    pub trust: String,
    /// "daily", "off" (the person's switch for it, or the computer's), "none" (made without updates).
    pub updates: String,
    pub last_look: Option<update::LastLook>,
    /// A newer version is beside the file, used from its next start.
    pub ready: Option<String>,
    pub sandboxed: bool,
}

pub fn update_info(places: &Places, rec: &Record) -> Option<UpdateInfo> {
    let kept = read_app(places, &rec.id)?.bundle;
    let b = open_bundle(Path::new(&kept.file)).ok()?;
    let m = &b.meta;
    let mut out = UpdateInfo {
        version: m.version.clone(),
        feed: m.update.as_ref().map(|u| u.feed.clone()),
        listed: kept.listed,
        developer: m.developer.clone(),
        fingerprint: m.update.as_ref().and_then(|u| update::fingerprint(&u.key)),
        trust: match update::trust_in(&trust_dir(places), m) {
            crate::bundle_trust::Trust::Changed { .. } => "changed".into(),
            crate::bundle_trust::Trust::Unsigned => "unsigned".into(),
            _ => "same".into(),
        },
        updates: "none".into(),
        sandboxed: sandbox_here(m).1,
        ..Default::default()
    };
    if m.update.is_some() {
        out.updates = if update::wanted(m) { "daily".into() } else { "off".into() };
        let hash = b.trailer.hash();
        let cache = bundle_cache(&hash);
        let look = update::Me { path: &b.path, meta: m, hash: &hash, cache: &cache }.last_look();
        out.ready = look.newer.clone().filter(|_| look.why.is_none());
        out.last_look = Some(look);
    }
    Some(out)
}

/// Its own switch, set as the app's is (`--nami-bundle-updates on|off`).
pub fn set_automatic(places: &Places, rec: &Record, on: bool) {
    let Some(b) = installed_bundle(places, rec) else { return };
    if b.meta.update.is_some() {
        let _ = update::set_wanted(&b.meta, on);
    }
}

/// Network trouble, said by the bundle: tried again later, quietly.
fn offline(said: &str) -> bool {
    ["Connection refused", "failed to lookup", "Dns", "dns error", "timed out", "Network is unreachable", "No route to host", "Network Error"].iter().any(|w| said.contains(w))
}

/// The daily run's look (and Update now): the bundle's own update
/// (`--nami-bundle-update`: checked against its developer's key, put
/// together, swapped in), what it found kept as for any app from a
/// download. Its new version, if it updated.
pub fn update_now(places: &Places, rec: &Record, st: &mut foreign::update::AppStatus) -> Option<String> {
    let Some(b) = installed_bundle(places, rec) else {
        st.state = "failed".into();
        st.said = format!("{} isn't where it was installed: install it again.", rec.name);
        return None;
    };
    let now = now();
    if b.meta.update.is_none() {
        st.checked = now;
        st.state = "manual".into();
        st.said = "It doesn't update itself: to update it, open a newer download of it.".into();
        return None;
    }
    foreign::say(&format!("Looking for a newer {}…", rec.name));
    let out = Command::new(&b.path).arg(format::flag(&b.path, "update")).env_remove("NAMI_BUNDLE_UPDATE").env_remove("TRAY_BUNDLE_UPDATE").stdin(Stdio::null()).output();
    let (ok, said) = match &out {
        Ok(o) => (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)).trim().to_string()),
        Err(e) => (false, e.to_string()),
    };
    let before = b.meta.version.clone();
    let after = open_bundle(&b.path).map(|n| n.meta.version).unwrap_or_else(|_| before.clone());
    if !ok && offline(&said) {
        return None;
    }
    st.checked = now;
    st.newest = Some(after.clone());
    if after != before {
        st.before = Some(before.clone());
        st.updated = now;
        st.state = "updated".into();
        st.said = format!("Updated to {after} from {before}.");
        st.told.clear();
        // (Its record says the version it is now.)
        if let Some(mut a) = read_app(places, &rec.id) {
            a.record.version = after.clone();
            if let Ok(j) = serde_json::to_vec_pretty(&a) {
                let _ = fs::write(places.apps().join(&rec.id).join("app.json"), j);
            }
        }
        foreign::say(&st.said);
        return Some(after);
    }
    if ok && said.contains("can't update itself") {
        st.state = "manual".into();
        st.said = said.lines().next().unwrap_or("").trim_start_matches(&format!("{}: ", b.meta.name)).to_string();
    } else if ok {
        if st.state != "updated" || st.updated + 7 * 86400 < now {
            st.state = "up-to-date".into();
            st.said = "Up to date.".into();
        }
    } else {
        st.state = "failed".into();
        st.said = format!("Couldn't look for a newer version: {}", said.lines().last().unwrap_or("").trim_start_matches("nami-bundle: ").trim_start_matches("tray-bundle: ").trim_start_matches("error: "));
    }
    foreign::say(&st.said);
    None
}

/// `nami install` of files, bundles among them (the rest as downloads are).
pub fn install_cli(root: &Path, files: &[String], libraries: bool) -> Result<()> {
    for f in files {
        if is_bundle_file(f) {
            install(&Places::here(), Path::new(f))?;
        } else {
            foreign::install_cli(root, std::slice::from_ref(f), libraries)?;
        }
    }
    Ok(())
}

/// `nami remove` of an app from a download: a bundle as above, the rest as foreign does.
pub fn remove_any(places: &Places, rec: &Record) -> Result<()> {
    if rec.format == "bundle" {
        remove(places, rec)
    } else {
        foreign::remove(places, rec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(out: &mut Vec<u8>, v: &[u8]) {
        out.extend((v.len() as u64).to_le_bytes());
        out.extend(v);
        out.extend(std::iter::repeat_n(0u8, ((8 - v.len() % 8) % 8) as usize));
    }

    /// A NAR: share/icons/a.png (a file), share/icons/b.png (a link to it), bin/x.
    fn nar() -> Vec<u8> {
        let mut o = Vec::new();
        for w in ["nix-archive-1", "(", "type", "directory", "entry", "(", "name", "bin", "node", "(", "type", "directory", "entry", "(", "name", "x", "node", "(", "type", "regular", "executable", "", "contents"] {
            s(&mut o, w.as_bytes());
        }
        s(&mut o, b"#!/bin/sh\n");
        for w in [")", ")", ")", ")", "entry", "(", "name", "share", "node", "(", "type", "directory", "entry", "(", "name", "a.png", "node", "(", "type", "regular", "contents"] {
            s(&mut o, w.as_bytes());
        }
        s(&mut o, b"PNG!");
        for w in [")", ")", "entry", "(", "name", "b.png", "node", "(", "type", "symlink", "target", "a.png", ")", ")", ")", ")", ")"] {
            s(&mut o, w.as_bytes());
        }
        o
    }

    #[test]
    fn entries_in_user_nami() {
        assert_eq!(entry_for("https://github.com/ada/myapp/releases/latest/download"), "bundle:github:ada/myapp");
        assert_eq!(entry_for("https://example.org/apps/myapp/"), "bundle:https://example.org/apps/myapp");
        assert_eq!(parse_entry("bundle:github:ada/myapp"), Some(("github:ada/myapp".into(), None)));
        assert_eq!(parse_entry("bundle:github:ada/myapp@3F2A 9C01"), Some(("github:ada/myapp".into(), Some("3F2A 9C01".into()))));
        assert_eq!(parse_entry("bundle:https://example.org/x@AB12"), Some(("https://example.org/x".into(), Some("AB12".into()))));
        assert_eq!(parse_entry("bundle:myapp"), None);
        assert_eq!(parse_entry("bundle:github:ada"), None);
        assert_eq!(fp_norm("3f2a 9C01"), "3F2A9C01");
    }

    /// A home of its own: (its folder, its Places).
    fn home(tag: &str) -> (PathBuf, Places) {
        let h = std::env::temp_dir().join(format!("nami-bapp-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&h);
        fs::create_dir_all(&h).unwrap();
        let p = Places { data: h.join(".local/share"), config: h.join(".config"), bin: h.join(".local/bin") };
        (h, p)
    }

    #[test]
    fn kept_in_user_nami_and_read_as_nami_reads_it() {
        let (h, places) = home("file");
        let file = user_file(&places);
        assert_eq!(file, h.join("user.nami"));
        fs::write(&file, "# my apps\npackages:\n  - firefox   # the browser\n").unwrap();
        record_in_file(&file, "bundle:github:ada/myapp", Some("3F2A 9C01 5B7E 22D4 8A10 C3F9 0E6B 7D21")).unwrap();
        record_in_file(&file, "bundle:github:ada/myapp", Some("3F2A 9C01 5B7E 22D4 8A10 C3F9 0E6B 7D21")).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("# my apps\npackages:\n  - firefox   # the browser\n"), "{text}");
        assert_eq!(text.matches("bundle:github:ada/myapp").count(), 1, "{text}");
        // nami reads it with its key where a version goes, and checks it.
        let c = crate::config::load(&file).unwrap();
        assert_eq!(c.packages, ["firefox", "bundle:github:ada/myapp@3F2A 9C01 5B7E 22D4 8A10 C3F9 0E6B 7D21"]);
        assert_eq!(parse_entry(&c.packages[1]).unwrap().1.as_deref(), Some("3F2A 9C01 5B7E 22D4 8A10 C3F9 0E6B 7D21"));
        let diags = crate::config::check(&text, "user.nami", &h, None);
        assert!(!diags.iter().any(|d| d.message.contains("bundle:")), "{diags:?}");
        unrecord_in_file(&file, "bundle:github:ada/myapp").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "# my apps\npackages:\n  - firefox   # the browser\n");
        fs::remove_dir_all(&h).unwrap();
    }

    /// A small bundle that updates from `feed`, signed with `key`: its
    /// runtime a shell script (what `install` asks of it, it answers).
    fn make_bundle(out: &Path, key: &ed25519_dalek::SigningKey, key_name: &str, feed: &str, version: &str, rotations: Vec<crate::bundle_format::Rotation>) -> (Vec<u8>, Trailer) {
        let runtime = format!("#!/bin/sh\ncase \"$1\" in --nami-bundle-info) echo \"demo {version}\";; --nami-bundle-update) echo \"demo {version} is the newest\";; esac\nexit 0\n").into_bytes();
        let frame = zstd::encode_all(&nar()[..], 3).unwrap();
        let name = format!("{}-demo-{version}", "a".repeat(32));
        let mut meta = Meta {
            format: 1,
            name: "demo".into(),
            version: version.into(),
            entry: format!("/nix/store/{name}/bin/demo"),
            paths: vec![PathEntry { name: name.clone(), offset: runtime.len() as u64, size: frame.len() as u64, sha256: format::sha256_hex(&frame), nar_size: nar().len() as u64, ..Default::default() }],
            update: Some(crate::bundle_format::Update { feed: feed.into(), key: update::public_key(key_name, key), serial: 1000, rotations }),
            developer: Some("Ada Lovelace".into()),
            ..Default::default()
        };
        meta.programs.insert("demo".into(), meta.entry.clone());
        let json = serde_json::to_vec(&meta).unwrap();
        let trailer = Trailer { payload_offset: runtime.len() as u64, meta_offset: (runtime.len() + frame.len()) as u64, meta_len: json.len() as u64, meta_sha256: sha2::Digest::finalize(<sha2::Sha256 as sha2::Digest>::new_with_prefix(&json)).into() };
        let mut file = runtime;
        file.extend(&frame);
        file.extend(&json);
        file.extend(trailer.encode());
        fs::write(out, &file).unwrap();
        fs::set_permissions(out, fs::Permissions::from_mode(0o755)).unwrap();
        (json, trailer)
    }

    /// A release folder as `--publish-dir` writes it: the bundle, its
    /// description, the signed feed, and the install script.
    fn publish_demo(dir: &Path, key: &ed25519_dalek::SigningKey, key_name: &str, feed: &str, rotations: Vec<crate::bundle_format::Rotation>) {
        fs::create_dir_all(dir).unwrap();
        let file = dir.join("demo-x86_64");
        let (json, trailer) = make_bundle(&file, key, key_name, feed, "1.0", rotations.clone());
        fs::write(dir.join(update::meta_file(&trailer.hash())), &json).unwrap();
        let size = fs::metadata(&file).unwrap().len();
        let mut f = update::Feed { format: 1, name: "demo".into(), version: "1.0".into(), serial: 1000, sha256: trailer.hash(), size, file: "demo-x86_64".into(), signature: String::new(), rotations, ..Default::default() };
        f.sign(key_name, key);
        fs::write(dir.join(update::FEED), serde_json::to_vec(&f).unwrap()).unwrap();
        let meta: Meta = serde_json::from_slice(&json).unwrap();
        let sha = crate::bundle_format::sha256_hex(&fs::read(&file).unwrap());
        fs::write(dir.join(crate::bundle_publish::SCRIPT), crate::bundle_publish::Script::of(&meta, feed, "demo-x86_64", &sha, size).text()).unwrap();
    }

    /// Serves `dir` over HTTP on 127.0.0.1 (until the test ends): its URL.
    fn serve(dir: PathBuf) -> String {
        use std::io::{BufRead, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let mut r = std::io::BufReader::new(&s);
                    let mut first = String::new();
                    let _ = r.read_line(&mut first);
                    loop {
                        let mut l = String::new();
                        if r.read_line(&mut l).unwrap_or(0) == 0 || l == "\r\n" {
                            break;
                        }
                    }
                    let path = first.split(' ').nth(1).unwrap_or("/").trim_start_matches('/').to_string();
                    let mut w = &s;
                    match fs::read(dir.join(&path)).ok().filter(|_| !path.contains("..")) {
                        Some(b) => {
                            let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len());
                            let _ = w.write_all(&b);
                        }
                        None => {
                            let _ = write!(w, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        }
                    }
                });
            }
        });
        url
    }

    #[test]
    fn listed_in_user_nami_and_a_step_of_the_history() {
        let (h, places) = home("apply");
        let key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
        let release = h.join("release");
        let url = serve(release.clone());
        publish_demo(&release, &key, "bundle-test", &url, vec![]);
        let fp = update::fingerprint(&update::public_key("bundle-test", &key)).unwrap();
        let entry = format!("bundle:{url}");
        let listed = format!("{entry}@{fp}");
        // Another key in the file: refused, nothing installed.
        let (got, left) = install_listed(&places, &[format!("{entry}@0000 1111 2222 3333 4444 5555 6666 7777")]);
        assert!(got.is_empty() && left.len() == 1 && left[0].1.contains("not the one your user.nami trusts"), "{left:?}");
        assert!(foreign::records(&places).is_empty());
        // Its key: installed, as a command, kept under its entry; a step's entry, its version in its name.
        let (got, left) = install_listed(&places, std::slice::from_ref(&listed));
        assert!(left.is_empty(), "{left:?}");
        assert_eq!(got, vec![crate::profile::Entry { attr: listed.clone(), name: format!("{entry}-1.0"), paths: vec![] }]);
        let recs = foreign::records(&places);
        assert_eq!(recs.len(), 1);
        assert!(places.bin.join("demo").exists());
        let info = update_info(&places, &recs[0]).unwrap();
        assert_eq!(info.listed.as_deref(), Some(entry.as_str()));
        assert_eq!(info.developer.as_deref(), Some("Ada Lovelace"));
        assert_eq!(info.trust, "same");
        assert!(trust_dir(&places).join("demo").exists(), "its key is trusted from now on");
        assert!(fs::read_to_string(h.join("user.nami")).unwrap().contains(&format!("{entry}:")), "kept in the person's file");
        // Again: nothing to do.
        assert_eq!(install_listed(&places, std::slice::from_ref(&listed)).0, got);
        assert_eq!(foreign::records(&places).len(), 1);
        // A download of the same app signed with another key: refused while the first is trusted.
        let other = ed25519_dalek::SigningKey::from_bytes(&[10u8; 32]);
        let evil = h.join("evil");
        publish_demo(&evil, &other, "bundle-other", &url, vec![]);
        let (l, extra) = look(&places, &evil.join("demo-x86_64")).unwrap();
        assert!(l.blocked.as_deref().is_some_and(|b| b.contains("another key")), "{:?}", l.blocked);
        assert_eq!(extra.trust, "changed");
        assert!(install(&places, &evil.join("demo-x86_64")).is_err());
        // A step without it (undoing its install): taken off, its key forgotten.
        follow(&places, &got, &[]);
        assert!(foreign::records(&places).is_empty());
        assert!(!places.bin.join("demo").exists());
        assert!(!trust_dir(&places).join("demo").exists());
        // Going back to the step with it: put back, from where it updates.
        follow(&places, &[], &got);
        assert_eq!(foreign::records(&places).len(), 1);
        follow(&places, &got, &[]);
        // The file's key still says whose it is: a release signed with another is refused.
        publish_demo(&release, &other, "bundle-other", &url, vec![]);
        let (_, left) = install_listed(&places, std::slice::from_ref(&listed));
        assert!(left.len() == 1 && left[0].1.contains("not the one your user.nami trusts"), "{left:?}");
        assert!(foreign::records(&places).is_empty());
        // Unless the listed key handed over to it (`nami bundle --rotate-key`): taken, and the file says the new key.
        let new = ed25519_dalek::SigningKey::from_bytes(&[12u8; 32]);
        let (from, to) = (update::public_key("bundle-test", &key), update::public_key("bundle-new", &new));
        publish_demo(&release, &new, "bundle-new", &url, vec![update::sign_rotation(&key, &from, &to)]);
        record_in_file(&user_file(&places), &entry, Some(&fp)).unwrap();
        let (got, left) = install_listed(&places, std::slice::from_ref(&listed));
        assert!(left.is_empty() && got.len() == 1, "{left:?}");
        let new_fp = update::fingerprint(&to).unwrap();
        let file = fs::read_to_string(h.join("user.nami")).unwrap();
        assert!(file.contains(&new_fp) && !file.contains(&fp), "{file}");
        assert_eq!(crate::bundle_trust::trusted_in(&trust_dir(&places), &Meta { name: "demo".into(), ..Default::default() }).as_deref(), Some(to.as_str()));
        // A handover signed by the new key instead of the old: refused.
        let forged = update::sign_rotation(&other, &from, &update::public_key("bundle-other", &other));
        follow(&places, &got, &[]);
        publish_demo(&release, &other, "bundle-other", &url, vec![forged]);
        let (_, left) = install_listed(&places, std::slice::from_ref(&listed));
        assert_eq!(left.len(), 1, "{left:?}");
        fs::remove_dir_all(&h).unwrap();
    }

    /// The install line's script, run with sh in a home of its own.
    #[test]
    #[cfg_attr(not(all(target_os = "linux", target_arch = "x86_64")), ignore = "a published bundle's install script is for x86_64 Linux (bundle_publish.rs)")]
    fn the_install_script() {
        let tools = ["sh", "uname", "mkdir", "head", "grep", "wc", "tr", "cut", "chmod", "mv", "rm", "cat", "sed", "sha256sum", "curl", "printf"];
        let which = |t: &str| std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(t)).find(|f| f.is_file()));
        if tools.iter().any(|t| which(t).is_none() && *t != "printf") {
            eprintln!("skipped: no curl or coreutils here");
            return;
        }
        let (h, _) = home("script");
        let bin = h.join("tools");
        fs::create_dir_all(&bin).unwrap();
        for t in tools {
            if let Some(p) = which(t) {
                std::os::unix::fs::symlink(p, bin.join(t)).unwrap();
            }
        }
        let key = ed25519_dalek::SigningKey::from_bytes(&[11u8; 32]);
        let release = h.join("release");
        let url = serve(release.clone());
        publish_demo(&release, &key, "bundle-test", &url, vec![]);
        let script = fs::read_to_string(release.join(crate::bundle_publish::SCRIPT)).unwrap();
        assert!(script.contains("by='demo by Ada Lovelace, its updates signed with the key "), "{script}");
        let run = |path: &str| {
            let o = Command::new(bin.join("sh")).arg(release.join(crate::bundle_publish::SCRIPT)).env_clear().env("HOME", h.join("home")).env("PATH", path).output().unwrap();
            (o.status.success(), String::from_utf8_lossy(&o.stderr).into_owned())
        };
        let path = bin.display().to_string();
        let (ok, said) = run(&path);
        assert!(ok, "{said}");
        assert!(said.contains("Downloading demo 1.0") && said.contains("demo is installed. Run it with: demo"), "{said}");
        let dest = h.join("home/.local/bin/demo");
        assert!(fs::metadata(&dest).unwrap().permissions().mode() & 0o111 != 0, "a program");
        // Again: the copy there updates itself.
        let (ok, said) = run(&path);
        assert!(ok && said.contains("installed already. Looking for a newer version"), "{said}");
        // Someone else's file where it goes: left alone.
        fs::write(&dest, "#!/bin/sh\n").unwrap();
        let (ok, said) = run(&path);
        assert!(!ok && said.contains("is there already and isn't demo"), "{said}");
        fs::remove_file(&dest).unwrap();
        // A download that isn't what the release says: nothing installed.
        let mut f = fs::read(release.join("demo-x86_64")).unwrap();
        let n = f.len();
        f[n - 100] ^= 1;
        fs::write(release.join("demo-x86_64"), f).unwrap();
        let (ok, said) = run(&path);
        assert!(!ok && said.contains("doesn't match what the developer published"), "{said}");
        assert!(!dest.exists());
        // With nami here: nami installs it (and keeps it in user.nami).
        publish_demo(&release, &key, "bundle-test", &url, vec![]);
        fs::write(bin.join("nami"), format!("#!{}\necho \"$@\" > {}\n", bin.join("sh").display(), h.join("nami-args").display())).unwrap();
        fs::set_permissions(bin.join("nami"), fs::Permissions::from_mode(0o755)).unwrap();
        let (ok, said) = run(&path);
        assert!(ok, "{said}");
        assert!(fs::read_to_string(h.join("nami-args")).unwrap().starts_with("install "), "nami installs it");
        assert!(!dest.exists());
        fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn picks_files_out_of_a_part() {
        let got = pick_nar(&mut &nar()[..], &|p| p.starts_with("share/")).unwrap();
        assert_eq!(got.get("share/a.png"), Some(&Picked::File(b"PNG!".to_vec())));
        assert_eq!(got.get("share/b.png"), Some(&Picked::Link("a.png".into())));
        assert!(!got.contains_key("bin/x"));
        assert!(pick_nar(&mut &nar()[..40], &|_| true).is_err());
    }

    #[test]
    fn places_and_names() {
        assert_eq!(feed_place("https://github.com/ada/app/releases/latest/download"), "github.com/ada/app");
        assert_eq!(feed_place("https://example.org/feed"), "example.org");
        assert_eq!(normal("share/icons/../pixmaps/./a.png").as_deref(), Some("share/pixmaps/a.png"));
        assert_eq!(normal("../a"), None);
        assert_eq!(file_name(Path::new("/d/jq x86_64"), "jq"), "jq-x86_64");
        assert_eq!(file_name(Path::new("/d/.hidden"), "jq"), "hidden");
        let mut m = Meta { name: "jq".into(), entry: "/nix/store/x-jq-1.8-bin/bin/jq".into(), ..Default::default() };
        m.programs.insert("jq".into(), m.entry.clone());
        m.programs.insert("jqx".into(), m.entry.clone());
        assert_eq!(commands(&m), vec!["jq", "jqx"]);
    }
}
