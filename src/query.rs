//! Read-only questions about packages and profiles, answered for people
//! (tables) and for programs (`--json`).

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::index::{Index, Package};
use crate::profile::{self, Entry, Profiles};

#[derive(Debug, Serialize)]
pub struct Item {
    pub attr: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    /// Ships a .desktop file: an app rather than a command or library.
    pub app: bool,
    pub installed: bool,
    /// Where its main program is, when installed.
    pub program: Option<String>,
    /// Its app icon (PNG or SVG): the one it ships when installed, else the
    /// catalog's.
    pub icon: Option<String>,
    /// Not free software.
    pub unfree: bool,
    /// Built on this computer when installed (the binary cache lacks it).
    pub builds_here: bool,
    /// Who makes it, for apps that aren't from nixpkgs (extra apps the
    /// catalog's publisher builds and serves).
    pub publisher: Option<String>,
    /// The app's own name, its line, a few sentences, its kinds (the
    /// window's groups: `CATEGORIES`), screenshots and popularity (0-1000).
    pub title: Option<String>,
    pub summary: Option<String>,
    pub about: Option<String>,
    pub categories: Vec<String>,
    pub screenshots: Vec<String>,
    pub popularity: Option<u16>,
    /// Its menu entries (.desktop files), to start it by.
    #[serde(default)]
    pub desktop: Vec<String>,
    /// Where it comes from when it isn't the catalog: `flathub`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Why nami can't install it here (one of Homebrew's casks that needs a .pkg, another
    /// macOS...): said, and its Install not offered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

/// The kinds of app people look through, and the freedesktop categories
/// each takes in: (key, name, categories).
pub const CATEGORIES: &[(&str, &str, &[&str])] = &[
    ("office", "Office", &["Office"]),
    ("graphics", "Graphics and photos", &["Graphics", "Photography"]),
    ("audio-video", "Music and video", &["AudioVideo", "Audio", "Video"]),
    ("games", "Games", &["Game"]),
    ("internet", "Internet", &["Network"]),
    ("coding", "Coding", &["Development"]),
    ("learning", "Learning and science", &["Education", "Science"]),
    ("tools", "Tools", &["Utility", "System", "Settings"]),
];

/// The groups (`CATEGORIES` keys) a package's categories put it in.
fn groups(pkg: &Package) -> Vec<String> {
    CATEGORIES.iter().filter(|(_, _, cats)| pkg.categories.iter().any(|c| cats.contains(&c.as_str()))).map(|(k, _, _)| k.to_string()).collect()
}

/// `firefox-157.0` -> `157.0` (minus packaging suffixes like `-wrapped`).
pub fn version_of(name: &str) -> String {
    let v = name
        .match_indices('-')
        .find(|(i, _)| name[i + 1..].starts_with(|c: char| c.is_ascii_digit()))
        .map(|(i, _)| name[i + 1..].to_string())
        .unwrap_or_default();
    v.trim_end_matches("-wrapped").trim_end_matches("-unwrapped").to_string()
}

/// The `bin` directory of the active profile, as users see it.
pub fn profile_bin(profiles: &Profiles) -> PathBuf {
    if profiles.is_system() {
        crate::migrate::here().join("profiles/default/bin")
    } else {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state/nami/profile/bin")
    }
}

/// The icon an installed app ships, found the way a desktop would: the
/// `Icon=` of its .desktop file, looked up in its own share/icons.
pub fn own_icon(paths: &[String]) -> Option<PathBuf> {
    for p in paths {
        let Ok(dir) = std::fs::read_dir(format!("{p}/share/applications")) else { continue };
        for f in dir.flatten() {
            let Ok(text) = std::fs::read_to_string(f.path()) else { continue };
            let Some(name) = text.lines().find_map(|l| l.strip_prefix("Icon=")).map(str::trim) else { continue };
            if name.starts_with('/') {
                if Path::new(name).exists() {
                    return Some(name.into());
                }
                continue;
            }
            let share = Path::new(p).join("share");
            let mut candidates = vec![share.join(format!("icons/hicolor/scalable/apps/{name}.svg"))];
            for size in ["512x512", "256x256", "128x128", "96x96", "64x64", "48x48"] {
                candidates.push(share.join(format!("icons/hicolor/{size}/apps/{name}.png")));
            }
            candidates.push(share.join(format!("pixmaps/{name}.png")));
            candidates.push(share.join(format!("pixmaps/{name}.svg")));
            if let Some(c) = candidates.into_iter().find(|c| c.exists()) {
                return Some(c);
            }
        }
    }
    None
}

pub fn item(attr: &str, pkg: Option<&Package>, name: &str, installed: &[Entry], bin: &PathBuf, index: Option<&Index>) -> Item {
    let entry = installed.iter().find(|e| e.attr == attr);
    let is_installed = entry.is_some();
    let icon = entry.and_then(|e| own_icon(&e.paths)).or_else(|| index.and_then(|i| i.icon(attr))).map(|p| p.display().to_string());
    let program = is_installed.then(|| {
        let p = pkg.and_then(|p| p.main_program.clone()).unwrap_or_else(|| attr.to_string());
        bin.join(p)
    }).filter(|p| p.exists()).map(|p| p.display().to_string());
    Item {
        attr: attr.to_string(),
        name: name.to_string(),
        version: version_of(name),
        description: pkg.and_then(|p| p.description.clone()),
        homepage: pkg.and_then(|p| p.homepage.clone()),
        license: pkg.and_then(|p| p.license.clone()),
        app: pkg.is_some_and(|p| p.apps.is_some()),
        installed: is_installed,
        program,
        icon,
        unfree: pkg.is_some_and(|p| p.unfree),
        builds_here: pkg.is_some_and(|p| p.closure.is_some()),
        publisher: pkg.and_then(|p| p.publisher.clone()),
        title: pkg.and_then(|p| p.title.clone()),
        summary: pkg.and_then(|p| p.summary.clone()),
        about: pkg.and_then(|p| p.about.clone()),
        categories: pkg.map(groups).unwrap_or_default(),
        screenshots: pkg.map(|p| p.screenshots.clone()).unwrap_or_default(),
        popularity: pkg.and_then(|p| p.popularity),
        desktop: pkg.and_then(|p| p.apps.clone()).unwrap_or_default(),
        source: None,
        unavailable: None,
    }
}

/// Ranked search: how well it matches (its name, the app's own name, its
/// search words, its description), then how many people use it; apps
/// first. Without `all`, only apps. With `category` (a `CATEGORIES` key),
/// only that kind of app. An empty `q` lists them (or all apps), most used first.
pub fn search(index: &Index, q: &str, all: bool, category: Option<&str>, profiles: &Profiles) -> Result<Vec<Item>> {
    Ok(search_counting(index, q, all, category, profiles, false)?.0)
}

/// `search`, and (with `count`, without `all`) how many tools and
/// libraries it left out that --all would find.
pub fn search_counting(index: &Index, q: &str, all: bool, category: Option<&str>, profiles: &Profiles, count: bool) -> Result<(Vec<Item>, usize)> {
    let installed = profiles.current_packages()?;
    let bin = profile_bin(profiles);
    let q = q.trim().to_lowercase();
    // A category is a bit in each package's search record (index_file.rs).
    let group: Option<u8> = match category {
        Some(c) => Some(CATEGORIES.iter().position(|(k, _, _)| *k == c).map(|i| 1u8 << i).ok_or_else(|| {
            anyhow::anyhow!("no category {c:?}; there are: {}", CATEGORIES.iter().map(|(k, _, _)| *k).collect::<Vec<_>>().join(", "))
        })?),
        None => None,
    };
    // Scored from what the catalog's compact form has of each package (in
    // lowercase already); only the ones shown are read whole.
    let mut hits: Vec<(i32, &str, usize)> = Vec::new();
    let mut hidden = 0;
    for (i, attr, rec) in index.records() {
        // (A Mac's catalog names the icons of Homebrew's casks in an entry that's no package.)
        if attr == crate::casks::ICONS_ENTRY {
            continue;
        }
        let is_app = rec.flags & crate::index_file::APP != 0;
        // Asked for by its very name, a tool or library is shown with the apps:
        // `nami search ripgrep` finds ripgrep, not only apps that mention it.
        let named = !q.is_empty() && attr.eq_ignore_ascii_case(&q);
        let left_out = !all && !is_app && !named;
        if left_out && (!count || q.is_empty()) {
            continue;
        }
        if group.is_some_and(|g| rec.groups & g == 0) {
            continue;
        }
        let a = attr.to_lowercase();
        let title = rec.title;
        // A word starting with what's typed: "word" finds "Word processor", not "password".
        let starts = |text: &str| text.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(&q));
        // Its name in its set: ggplot2 of rPackages.ggplot2.
        let leaf = a.rsplit('.').next().unwrap_or(&a);
        let mut score = if q.is_empty() {
            0
        } else if a == q || title == q {
            100
        } else if leaf == q {
            90
        } else if a.starts_with(&q) || starts(title) {
            70
        } else if rec.keywords().any(starts) || (q.len() >= 4 && a.contains(&q)) || leaf.starts_with(&q) {
            60
        } else if q.len() >= 3 && [rec.summary, rec.description].iter().any(|d| starts(d)) {
            40
        } else {
            continue;
        };
        if left_out {
            hidden += 1;
            continue;
        }
        score += i32::from(rec.popularity) / 25;
        if is_app {
            score += 15;
        }
        if attr.contains('.') {
            score -= 20; // inside package sets: rarely what people mean
        }
        hits.push((score, attr, i));
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(b.1)));
    // One of each app: its variants (gimp-with-plugins, bottles-unwrapped)
    // say the same about themselves; asked for by name, they're there.
    let mut seen = std::collections::BTreeSet::new();
    let limit = if q.is_empty() { 200 } else { 60 };
    let mut out = Vec::new();
    for (_, a, i) in hits {
        if out.len() == limit {
            break;
        }
        let Some(p) = index.package_at(i) else { continue };
        let keep = match (&p.title, &p.summary) {
            (Some(t), Some(s)) => seen.insert((t.clone(), s.clone())) || a.to_lowercase() == q,
            _ => true,
        };
        if keep {
            out.push(item(a, Some(p), &p.name, &installed, &bin, Some(index)));
        }
    }
    Ok((out, hidden))
}

/// Screenshots (URLs) as files, downloaded into ~/.cache/nami/screenshots
/// once; any that can't be had are left out.
pub fn fetch_screenshots(http: &ureq::Agent, urls: &[String]) -> Vec<String> {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")));
    let Some(dir) = base.map(|b| b.join("nami/screenshots")) else { return Vec::new() };
    let _ = std::fs::create_dir_all(&dir);
    let threads: Vec<_> = urls.iter().map(|url| {
        let (http, url) = (http.clone(), url.clone());
        // One of the catalog's objects (`.../objects/<sha256>`: a Mac's App Store screenshots of
        // a cask, casks::screenshots): kept only if it's those bytes, as the signed catalog says.
        let object = url.rsplit_once("/objects/").map(|(_, s)| s.to_string()).filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        let ext = if object.is_some() || url.ends_with(".jpg") || url.ends_with(".jpeg") { "jpg" } else { "png" };
        let file = match &object {
            Some(sha) => dir.join(format!("{sha}.{ext}")),
            None => dir.join(format!("{}.{ext}", &crate::catalog::sha256_hex(url.as_bytes())[..32])),
        };
        std::thread::spawn(move || {
            if file.exists() {
                return Some(file);
            }
            let mut data = Vec::new();
            use std::io::Read;
            http.get(&url).call().ok()?.into_reader().take(16 << 20).read_to_end(&mut data).ok()?;
            if object.as_deref().is_some_and(|sha| crate::catalog::sha256_hex(&data) != sha) {
                return None;
            }
            let part = file.with_extension("part");
            std::fs::write(&part, &data).ok()?;
            std::fs::rename(&part, &file).ok()?;
            Some(file)
        })
    }).collect();
    threads.into_iter().filter_map(|t| t.join().ok().flatten()).map(|f| f.display().to_string()).collect()
}

/// Installed packages, with catalog details where known.
pub fn list(index: Option<&Index>, profiles: &Profiles) -> Result<Vec<Item>> {
    let installed = profiles.current_packages()?;
    let bin = profile_bin(profiles);
    Ok(installed
        .iter()
        .map(|e| {
            if let Some(it) = crate::flathub::list_item(e).or_else(|| crate::casks::list_item(e)) {
                return it;
            }
            let pkg = index.and_then(|i| i.get(&e.attr).ok());
            let mut it = item(&e.attr, pkg, &e.name, &installed, &bin, index);
            // What's actually installed says whether it's an app (recipes
            // aren't in the catalog).
            if !it.app {
                it.app = e.paths.iter().any(|p| {
                    std::fs::read_dir(format!("{p}/share/applications"))
                        .is_ok_and(|d| d.flatten().any(|f| f.file_name().to_string_lossy().ends_with(".desktop")))
                });
            }
            if let Some(spec) = crate::python_env::parse(&e.attr) {
                it.description = Some(spec.about(&it.version));
                it.program = Some(bin.join("python3")).filter(|p| p.exists()).map(|p| p.display().to_string());
            }
            if e.attr.starts_with('/') {
                it.description = Some(format!("Built from your recipe at {}", e.attr));
                let pname = e.name.strip_suffix(&format!("-{}", it.version)).unwrap_or(&e.name);
                it.program = Some(bin.join(pname)).filter(|p| p.exists()).map(|p| p.display().to_string());
            }
            it
        })
        .collect())
}

#[derive(Debug, Serialize)]
pub struct Change {
    pub attr: String,
    pub name: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
pub struct Step {
    pub generation: u64,
    pub date: String,
    pub created: u64,
    pub current: bool,
    pub packages: usize,
    pub added: Vec<Entry>,
    pub removed: Vec<Entry>,
    pub changed: Vec<Change>,
    /// Dotfiles added, removed or changed.
    pub files: usize,
}

/// Generations, newest first, each with what changed from the one before.
pub fn history(profiles: &Profiles) -> Result<Vec<Step>> {
    let current = profiles.current_generation();
    let gens = profiles.generations()?;
    let mut out = Vec::new();
    for (i, &g) in gens.iter().enumerate().rev() {
        let m = profiles.manifest(g)?;
        let before = if i > 0 { profiles.manifest(gens[i - 1])?.packages } else { Vec::new() };
        let added = m.packages.iter().filter(|e| !before.iter().any(|b| b.attr == e.attr)).cloned().collect();
        let removed = before.iter().filter(|b| !m.packages.iter().any(|e| e.attr == b.attr)).cloned().collect();
        let changed = m
            .packages
            .iter()
            .filter_map(|e| {
                let b = before.iter().find(|b| b.attr == e.attr && (b.paths != e.paths || e.paths.is_empty() && b.name != e.name))?;
                Some(Change { attr: e.attr.clone(), name: e.name.clone(), from: version_of(&b.name), to: version_of(&e.name) })
            })
            .collect();
        let files = if i > 0 {
            let d = profiles.diff(gens[i - 1], g)?;
            d.home_added.len() + d.home_removed.len() + d.home_changed.len()
        } else {
            m.home.as_ref().map(|h| h.files.len()).unwrap_or(0)
        };
        out.push(Step {
            generation: g,
            date: profile::format_time(m.created),
            created: m.created,
            current: Some(g) == current,
            packages: m.packages.len(),
            added,
            removed,
            changed,
            files,
        });
    }
    Ok(out)
}

/// A package a system's flat file lists (`packages:`), and the file it's from
/// (`  - firefox@150  # system.nami`).
pub fn flat_packages(flat: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = flat.lines().skip_while(|l| l.trim_end() != "packages:");
    lines.next();
    for line in lines.take_while(|l| l.starts_with(' ') || l.is_empty()) {
        let Some(item) = line.trim_start().strip_prefix("- ") else { continue };
        let (name, from) = item.split_once("  # ").unwrap_or((item, ""));
        let name = name.trim().trim_matches(|c| c == '\'' || c == '"');
        if !name.is_empty() {
            out.push((name.to_string(), from.trim().to_string()));
        }
    }
    out
}

#[derive(Debug, Serialize)]
pub struct Update {
    pub attr: String,
    pub from: String,
    pub to: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_files_packages_and_where_each_is_from() {
        let flat = "# Generated by nami\nhostname: t  # system.nami\npackages:\n  - systemd  # ../packages/base/package.nami\n  - firefox@150  # system.nami\n  - 'gimp'  # system.nami\nservices:\n  - name: x\n";
        assert_eq!(
            flat_packages(flat),
            vec![("systemd".into(), "../packages/base/package.nami".into()), ("firefox@150".into(), "system.nami".into()), ("gimp".into(), "system.nami".into())]
        );
        assert!(flat_packages("hostname: t\n").is_empty());
    }

    #[test]
    fn a_package_before_its_namesakes_in_sets_and_a_set_member_by_its_own_name() {
        let root = std::env::temp_dir().join(format!("nami-search-sets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("nix/var/nami")).unwrap();
        let p = |name: &str, extra: serde_json::Value| {
            let mut v = serde_json::json!({"name": name, "outputs": {"out": format!("/nix/store/{}-{name}", "0".repeat(32))}});
            v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            v
        };
        let none = serde_json::json!({});
        let index = serde_json::json!({
            "pandoc": p("pandoc-cli-3.7", serde_json::json!({"mainProgram": "pandoc", "description": "Conversion between documentation formats"})),
            "haskellPackages.pandoc": p("pandoc-3.7", serde_json::json!({"description": "Conversion between markup formats"})),
            "haskellPackages.pandoc-types": p("pandoc-types-1.23", none.clone()),
            "python3Packages.pandocfilters": p("python3.13-pandocfilters-1.5", none.clone()),
            "rPackages.ggplot2": p("r-ggplot2-3.5", serde_json::json!({"description": "Create Elegant Data Visualisations"})),
            "rPackages.ggplot2movies": p("r-ggplot2movies-0.0.1", none.clone()),
            "rPackages.GGally": p("r-GGally-2.2", serde_json::json!({"description": "Extension to ggplot2"})),
            "emacsPackages.magit": p("emacs-magit-4.3", none.clone()),
            "emacsPackages.magit-section": p("emacs-magit-section-4.3", none.clone()),
            "vimPlugins.telescope-nvim": p("vimplugin-telescope.nvim", none.clone()),
            "jq": p("jq-1.8", serde_json::json!({"mainProgram": "jq"})),
            "haskellPackages.jq": p("jq-0.1", none.clone()),
            "gimp": p("gimp-3.0", serde_json::json!({"apps": ["gimp.desktop"], "title": "GIMP"})),
        });
        std::fs::write(root.join("nix/var/nami/index.json"), index.to_string()).unwrap();
        let store = crate::store::Store::open_direct_readonly(&root).unwrap();
        let index = Index::load(&store).unwrap();
        let profiles = Profiles::system(&store);
        let names = |q: &str| search(&index, q, true, None, &profiles).unwrap().into_iter().map(|i| i.attr).collect::<Vec<_>>();
        assert_eq!(names("pandoc")[..2], ["pandoc", "haskellPackages.pandoc"]);
        assert_eq!(names("ggplot2")[..2], ["rPackages.ggplot2", "rPackages.ggplot2movies"]);
        assert_eq!(names("magit")[0], "emacsPackages.magit");
        assert_eq!(names("telescope")[0], "vimPlugins.telescope-nvim", "a name in a set starting with it");
        assert_eq!(names("jq"), ["jq", "haskellPackages.jq"], "short words find names in sets too, after their own");
        assert_eq!(names("gimp"), ["gimp"]);
        // Without --all, apps only, and a tool asked for by its very name: no set's libraries.
        let apps = |q: &str| search(&index, q, false, None, &profiles).unwrap().into_iter().map(|i| i.attr).collect::<Vec<_>>();
        assert_eq!(apps("pandoc"), ["pandoc"]);
        assert_eq!(apps("JQ"), ["jq"]);
        assert!(apps("pando").is_empty());
        assert_eq!(apps("gimp"), ["gimp"]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
