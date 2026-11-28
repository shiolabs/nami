//! `nami bundle`: a program, with everything it needs, as one file that
//! runs on any glibc Linux (x86_64, or ARM with `--arch aarch64`) with
//! nothing installed first.
//!
//! `--arch aarch64` bundles a catalog package for ARM computers from this
//! one: its aarch64 build (the catalog's index-aarch64-linux.json names
//! it; cache.nixos.org has it), nothing compiled here, and the aarch64
//! runtime in front (build.sh with ARM=1 carries it).
//!
//! The program's whole closure goes in, glibc and its loader too, as the
//! store has it; the runtime in front (src/bin/nami-bundle-runtime.rs)
//! unpacks it once into ~/.cache, with `/nix/store` replaced by a
//! same-length path of its own, and starts it. The file's layout is in
//! src/bundle_format.rs; why it's built this way is in
//! docs/guide/ship-your-app.md.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};

use crate::build::Builder;
use crate::bundle_format::{self as format, Desktop, Loader, Meta, PathEntry, Trailer, RUNTIME_MAGIC, STORE};
use crate::index::Index;
use crate::store::{base_name, Store};

/// `nami bundle .`: a project built and bundled with no recipe.
#[path = "bundle_project.rs"]
pub mod project;

pub struct Options {
    /// A recipe folder, a nixpkgs package, a store path, or a program.
    pub what: String,
    pub out: Option<PathBuf>,
    /// Which of the package's programs runs.
    pub program: Option<String>,
    /// nixpkgs' Mesa too, for computers without a driver the bundle can use.
    pub mesa: bool,
    /// The .desktop file and icon next to the bundle.
    pub desktop: bool,
    /// zstd level.
    pub level: i32,
    /// Store paths (by name, without hash) to leave out, with what only they need.
    pub exclude: Vec<String>,
    /// An AppImage of it too, beside the bundle.
    pub appimage: bool,
    /// Leave out what cache.nixos.org has: fetched (and checked) when the
    /// bundle first starts.
    pub thin: bool,
    /// For a project (`nami bundle .`): its name, version, icon, menu entry.
    pub project: project::Hints,
    /// Where new versions are published (`--update-url`): the bundle
    /// checks there and updates itself (src/bundle_update.rs).
    pub update_url: Option<String>,
    /// Also write the feed and parts to upload there.
    pub publish_dir: Option<PathBuf>,
    /// The release before, for patches (`--previous`; bundle_publish::Previous).
    pub previous: Option<String>,
    /// Who made it (`--developer`), shown with the key that signs it;
    /// else project.nami's `bundle: developer:`.
    pub developer: Option<String>,
    /// Run it in a sandbox, with these rights (`--sandbox`); else
    /// project.nami's `bundle: sandbox:` (src/bundle_sandbox.rs).
    pub sandbox: Option<format::Sandbox>,
    /// The computers it's for: x86_64, or aarch64 (`--arch`).
    pub arch: String,
}

/// project.nami's `bundle: developer:` and `bundle: sandbox:`, for a
/// project folder (the rest of `bundle:` is bundle_project.rs's).
fn developer_and_sandbox(what: &str) -> Result<(Option<String>, Option<format::Sandbox>)> {
    let dir = Path::new(what);
    let Some(file) = dir.is_dir().then(|| crate::project::file_in(dir)).flatten() else { return Ok((None, None)) };
    let Some(b) = crate::project::read(&file)?.bundle else { return Ok((None, None)) };
    let what = file.display().to_string();
    let developer = match b.get("developer") {
        None | Some(serde_yaml::Value::Null) => None,
        Some(serde_yaml::Value::String(d)) if !d.trim().is_empty() => Some(d.trim().to_string()),
        Some(_) => bail!("{what}: bundle: developer: your name, as people know you (`developer: Ada Lovelace`)"),
    };
    let sandbox = match b.get("sandbox") {
        Some(v) => crate::bundle_sandbox::from_value(&serde_json::to_value(v)?, &what).map_err(|e| anyhow::anyhow!(e))?,
        None => None,
    };
    Ok((developer, sandbox))
}

/// Where a thin bundle's parts come from.
pub const PUBLIC_CACHE: &str = crate::store::NIXOS_CACHE;

/// What goes in: the roots of the closure and what runs.
struct Plan {
    name: String,
    version: String,
    roots: Vec<String>,
    /// Where to look for programs and the app's .desktop file.
    tops: Vec<String>,
    /// A program from outside the store: (base name, its files).
    local: Option<(String, PathBuf)>,
    /// The store paths it refers to.
    local_refs: Vec<String>,
    entry: Option<String>,
    loader: Option<Loader>,
}

pub fn bundle(store: &Store, opts: &Options) -> Result<PathBuf> {
    ensure!(!(opts.thin && opts.appimage), "--thin and --appimage don't go together: an AppImage carries everything");
    ensure!(opts.publish_dir.is_none() || opts.update_url.is_some(), "--publish-dir needs --update-url: where it will be published goes in the bundle");
    let arch = opts.arch.as_str();
    ensure!(format::ARCHES.contains(&arch), "--arch {arch}: nami bundles for {}", format::ARCHES.join(" or "));
    // For this computer's kind (x86_64, or aarch64 on an ARM one): from its own catalog, and
    // what's built here. For the other: its catalog's packages (arch_plan).
    let native = arch == crate::arch::ARCH;
    if arch != "x86_64" {
        // An update feed is one file per folder, for one architecture (src/bundle_update.rs),
        // and its install script and download button are x86_64's (src/bundle_publish.rs).
        ensure!(opts.update_url.is_none(), "--update-url is for x86_64 bundles yet: a release's update.json is for its x86_64 file");
        ensure!(!opts.appimage, "--appimage is for x86_64 bundles yet: AppImage's runtime here is x86_64's");
    }
    let update = opts.update_url.as_deref().map(crate::bundle_publish::update_info).transpose()?;
    let (developer, sandbox) = {
        let (d, s) = developer_and_sandbox(&opts.what)?;
        (opts.developer.clone().filter(|d| !d.trim().is_empty()).or(d), opts.sandbox.clone().or(s))
    };
    // Updating needs the network: the runtime that fetches.
    let mut runtime = runtime_bytes(opts.thin || update.is_some(), arch)?;
    // (Its first bytes say it's a bundle, for desktops: application/x-tray-bundle. Not an
    // aarch64 one's: qemu-user's binfmt_misc entries, as distributions register them, want
    // those bytes zero, and an ARM bundle on an x86_64 computer runs only through them.)
    if arch == "x86_64" {
        format::mark_runtime(&mut runtime);
    }
    let index = if native { Index::load(store)? } else { arch_index(store, arch)? };
    let mut plan = if native { plan(store, &index, opts)? } else { arch_plan(store, &index, opts)? };
    if opts.mesa {
        let mesa = index.get("mesa")?;
        let out = mesa.outputs.get("out").cloned().flatten().context("the catalog's mesa has no out")?;
        store.realise(std::slice::from_ref(&out))?;
        plan.roots.push(out);
    }
    crate::log::status(&format!("Collecting what {} needs", plan.name));
    let (kept, dropped) = closure(store, &plan.roots, &opts.exclude)?;
    if !dropped.is_empty() {
        crate::say!("left out: {}", dropped.iter().map(|b| b.get(33..).unwrap_or(b)).collect::<Vec<_>>().join(", "));
    }
    let mut closure: Vec<String> = kept.into_iter().collect();
    // The program's own files from outside the store come too.
    if let Some((base, _)) = &plan.local {
        closure.push(base.clone());
    }
    closure.sort();
    let phys = |base: &str| -> PathBuf {
        match &plan.local {
            Some((b, dir)) if b == base => dir.clone(),
            _ => store.phys(&format!("{STORE}/{base}")),
        }
    };

    // What runs, and the other programs it could be.
    let mut programs = BTreeMap::new();
    for top in &plan.tops {
        let bin = phys(base_name(top)).join("bin");
        for e in fs::read_dir(&bin).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let ok = fs::metadata(e.path()).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false);
            if ok {
                programs.entry(name.clone()).or_insert(format!("{top}/bin/{name}"));
            }
        }
    }
    let entry = match (&opts.program, &plan.entry) {
        (Some(p), _) => programs.get(p).cloned().with_context(|| format!("{} has no program {p} (it has: {})", plan.name, list(&programs)))?,
        (None, Some(e)) => e.clone(),
        (None, None) => match programs.get(&plan.name) {
            Some(p) => p.clone(),
            None if programs.len() == 1 => programs.values().next().unwrap().clone(),
            None if programs.is_empty() => bail!("{} has no programs (nothing in bin/) to run", plan.name),
            None => bail!("{} has several programs: say which runs with --program (one of: {})", plan.name, list(&programs)),
        },
    };
    let entry_name = entry.rsplit('/').next().unwrap_or(&plan.name).to_string();

    // Libraries by soname, and glibc.
    let mut libs = BTreeMap::new();
    let mut glibc = String::new();
    for base in &closure {
        let dir = phys(base);
        if base.contains("-glibc-") && dir.join("lib").join(format::loader_name(arch)).exists() && glibc.is_empty() {
            glibc = format!("{STORE}/{base}");
        }
        sonames(&dir, &format!("{STORE}/{base}"), &mut libs);
    }
    if !dropped.is_empty() {
        check_left_out(&closure, &dropped, &libs, &plan.tops, &phys)?;
    }
    ensure!(!glibc.is_empty() || !closure.iter().any(|b| b.contains("-glibc-")), "the bundle's glibc isn't one for {arch} (no lib/{})", format::loader_name(arch));
    let out = opts.out.clone().unwrap_or_else(|| PathBuf::from(format!("{entry_name}-{arch}")));
    // What each part is (Nix's NAR hash) and refers to.
    let mut info: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for base in &closure {
        let i = match &plan.local {
            Some((b, dir)) if b == base => (crate::nar::hash_path(dir)?.0, plan.local_refs.clone()),
            _ => {
                let pi = store.path_info(base)?;
                (pi.nar_hash, pi.references.into_iter().filter(|r| r != base).collect())
            }
        };
        info.insert(base.clone(), i);
    }
    // Thin: what the public cache has, the same, is fetched instead.
    let fetched: BTreeSet<String> = if opts.thin { in_public_cache(store, &closure, &info) } else { BTreeSet::new() };
    let carried: Vec<String> = closure.iter().filter(|b| !fetched.contains(*b)).cloned().collect();
    let frames = compress(&carried, &phys, opts.level)?;
    // (Its icon, by its name, may be a theme's it carries: only one it carries.)
    let desktop = desktop(&plan, &phys, &entry_name, &carried);

    // runtime | frames | meta | trailer
    let mut meta = Meta {
        format: 1,
        name: plan.name.clone(),
        version: plan.version.clone(),
        entry,
        programs,
        loader: plan.loader.clone(),
        paths: Vec::new(),
        libs,
        glibc,
        desktop,
        env: BTreeMap::new(),
        made_by: format!("nami {}", env!("CARGO_PKG_VERSION")),
        runtime: format::sha256_hex(&runtime),
        substituters: if opts.thin { vec![format::Substituter { url: PUBLIC_CACHE.into(), key: crate::store::NIXOS_CACHE_KEY.into() }] } else { Vec::new() },
        update,
        developer,
        sandbox,
        arch: arch.to_string(),
    };
    crate::log::status(&format!("Writing {}", out.display()));
    let tmp =out.with_file_name(format!(".{}.tmp{}", out.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    let result = (|| -> Result<Trailer> {
        let mut f = std::io::BufWriter::new(fs::File::create(&tmp).with_context(|| format!("writing {}", tmp.display()))?);
        f.write_all(&runtime)?;
        let payload_offset = runtime.len() as u64;
        let mut at = payload_offset;
        let mut frames = carried.iter().zip(&frames);
        for base in &closure {
            let (nar_hash, references) = info[base].clone();
            if fetched.contains(base) {
                let nar_size = store.path_info(base)?.nar_size;
                meta.paths.push(PathEntry { name: base.clone(), offset: 0, size: 0, sha256: String::new(), nar_size, nar_hash, references, fetch: true });
                continue;
            }
            let (_, (data, nar_size)) = frames.next().context("a frame missing")?;
            f.write_all(data)?;
            meta.paths.push(PathEntry { name: base.clone(), offset: at, size: data.len() as u64, sha256: format::sha256_hex(data), nar_size: *nar_size, nar_hash, references, fetch: false });
            at += data.len() as u64;
        }
        let json = serde_json::to_vec(&meta)?;
        f.write_all(&json)?;
        let trailer = Trailer { payload_offset, meta_offset: at, meta_len: json.len() as u64, meta_sha256: sha2_256(&json) };
        f.write_all(&trailer.encode())?;
        f.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        Ok(trailer)
    })();
    let trailer = match result {
        Ok(t) => t,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    };
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
    fs::rename(&tmp, &out)?;
    if opts.desktop {
        match &meta.desktop {
            Some(d) => write_desktop(d, &out, &phys)?,
            None => eprintln!("{} has no .desktop file: no menu entry written", plan.name),
        }
    }
    let size = fs::metadata(&out)?.len();
    let unpacked: u64 = meta.paths.iter().map(|p| p.nar_size).sum();
    if let Some(dir) = &opts.publish_dir {
        crate::log::status("Signing the release");
        crate::bundle_publish::publish(&out, dir, opts.level, opts.previous.as_deref())?;
    }
    let appimage_file = if opts.appimage {
        crate::log::status("Writing the AppImage");
        Some(appimage(store, &index, &meta, &closure, &phys, &runtime, &out, &entry_name)?)
    } else {
        None
    };
    crate::log::settle();
    let card = Card {
        file: out.display().to_string(),
        size,
        unpacked,
        parts: carried.len(),
        fetched: fetched.len(),
        hash: trailer.hash(),
        arch: arch.to_string(),
        signed: meta.update.as_ref().map(|u| meta.developer.clone().unwrap_or_else(|| format!("your key ({})", crate::bundle_update::fingerprint(&u.key).unwrap_or_default()))),
        sandbox: meta.sandbox.as_ref().map(crate::bundle_sandbox::rights),
        what: opts.what.clone(),
        feed: meta.update.as_ref().map(|u| u.feed.clone()),
        publish_dir: opts.publish_dir.as_ref().map(|d| d.display().to_string()),
        appimage: appimage_file.as_ref().map(|f| (f.display().to_string(), fs::metadata(f).map(|m| m.len()).unwrap_or(0))),
    };
    for l in card.lines() {
        crate::say!("{l}");
    }
    Ok(out)
}

/// What `nami bundle` made, said at its end: `✓ ripgrep-x86_64 · 13.2 MB · runs on any
/// Linux distribution (x86_64, glibc) · signed by Ada Lovelace`, a line of detail, and
/// what to do next.
pub struct Card {
    pub file: String,
    pub size: u64,
    pub unpacked: u64,
    pub parts: usize,
    /// Parts a thin bundle fetches as it first starts.
    pub fetched: usize,
    pub hash: String,
    pub arch: String,
    /// Whose key signs its updates.
    pub signed: Option<String>,
    /// What its sandbox lets it use (empty: its window and its own files).
    pub sandbox: Option<String>,
    pub what: String,
    pub feed: Option<String>,
    pub publish_dir: Option<String>,
    pub appimage: Option<(String, u64)>,
}

impl Card {
    pub fn lines(&self) -> Vec<String> {
        use crate::tell::size;
        let mut first = vec![format!("✓ {}", self.file), size(self.size), format!("runs on any Linux distribution ({}, glibc)", self.arch)];
        first.extend(self.signed.as_ref().map(|s| format!("signed by {s}")));
        let mut out = vec![first.join(" · ")];
        let mut detail = vec![format!("{} unpacked", size(self.unpacked)), crate::tell::count(self.parts, "part", "parts")];
        if self.fetched > 0 {
            detail.push(format!("{} fetched from {PUBLIC_CACHE} as it first starts", self.fetched));
        }
        detail.push(format!("hash {}", self.hash));
        out.push(format!("  {}", detail.join(" · ")));
        if let Some(r) = &self.sandbox {
            out.push(format!("  In a sandbox where the computer has bubblewrap: {}", if r.is_empty() { "its window and its own files only".to_string() } else { format!("it can use {r}") }));
        }
        if let Some((f, n)) = &self.appimage {
            out.push(format!("✓ {f} · {} · the same, as an AppImage", size(*n)));
        }
        if let (Some(dir), Some(feed)) = (&self.publish_dir, &self.feed) {
            out.push(format!("✓ {dir} is ready to publish · upload every file in it to {feed}"));
            out.push(format!("  People install it with: {}", crate::bundle_publish::install_line(feed)));
            out.push(format!("  A Download for Linux button for your page: {}", Path::new(dir).join(crate::bundle_publish::BUTTON).display()));
        }
        let file = if self.file.contains('/') { self.file.clone() } else { format!("./{}", self.file) };
        out.push(String::new());
        out.push(format!("  Test it on {} distributions: nami bundle --test {file}", crate::bundle_test::distros_here().len()));
        if self.publish_dir.is_none() {
            out.push(format!("  Publish it, with updates: nami bundle {} --update-url <where> --publish-dir release/", self.what));
        }
        out
    }
}

/// `/nix/store/<base>/<rest>` -> (base, rest).
fn split_store(path: &str) -> (&str, &str) {
    let p = path.strip_prefix(STORE).unwrap_or(path).trim_start_matches('/');
    p.split_once('/').unwrap_or((p, ""))
}

/// Every path `roots` need, but not those named like `exclude` (nor what
/// only they need): (kept, left out).
fn closure(store: &Store, roots: &[String], exclude: &[String]) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let left_out = |base: &str| exclude.iter().any(|e| base.get(33..).is_some_and(|n| n.starts_with(e.as_str())));
    let (mut keep, mut dropped) = (BTreeSet::new(), BTreeSet::new());
    let mut stack: Vec<String> = roots.iter().map(|r| base_name(r).to_string()).collect();
    let root_set: BTreeSet<String> = stack.iter().cloned().collect();
    while let Some(b) = stack.pop() {
        if keep.contains(&b) || dropped.contains(&b) || !store.is_valid(&b) {
            continue;
        }
        if left_out(&b) && !root_set.contains(&b) {
            dropped.insert(b);
            continue;
        }
        stack.extend(store.path_info(&b)?.references);
        keep.insert(b);
    }
    Ok((keep, dropped))
}

/// What `--exclude` left out mustn't be a library that the app's programs
/// or a library that stays link to: it wouldn't start at all (what's loaded
/// later, a plugin, is the developer's to know).
fn check_left_out(kept: &[String], dropped: &BTreeSet<String>, libs: &BTreeMap<String, String>, tops: &[String], phys: &dyn Fn(&str) -> PathBuf) -> Result<()> {
    let mut gone = BTreeMap::new();
    for base in dropped {
        sonames(&phys(base), &format!("{STORE}/{base}"), &mut gone);
    }
    gone.retain(|soname, _| !libs.contains_key(soname));
    if gone.is_empty() {
        return Ok(());
    }
    let mut problems = Vec::new();
    for base in kept {
        let mut files = Vec::new();
        walk(&phys(base), &mut files);
        for f in files {
            let Ok(mut file) = fs::File::open(&f) else { continue };
            let mut head = [0u8; 4];
            if file.read_exact(&mut head).is_err() || &head != b"\x7fELF" {
                continue;
            }
            let Some(l) = fs::read(&f).ok().and_then(|d| crate::elf::linking(&d)) else { continue };
            // Plugins (in a lib/<something>/ folder) are loaded only when
            // used, and other packages' programs only run if something
            // runs them: those may lose theirs. The app's own programs and
            // every library may not.
            let rel = f.strip_prefix(phys(base)).unwrap_or(&f).to_string_lossy().into_owned();
            let own = tops.iter().any(|t| base_name(t) == base);
            let plugin = (rel.starts_with("lib/") && rel.matches('/').count() > 1) || (!own && !rel.starts_with("lib"));
            for n in l.needed.iter().filter(|n| gone.contains_key(*n)) {
                if !plugin {
                    problems.push(format!("{}/{rel} needs {n}, which only {} has", base.get(33..).unwrap_or(base), split_store(&gone[n]).0.get(33..).unwrap_or_default()));
                }
            }
        }
    }
    if !problems.is_empty() {
        problems.truncate(8);
        bail!("--exclude leaves out libraries that programs in the bundle link to (they wouldn't start):\n  {}", problems.join("\n  "));
    }
    Ok(())
}

fn list(programs: &BTreeMap<String, String>) -> String {
    programs.keys().cloned().collect::<Vec<_>>().join(", ")
}

fn sha2_256(data: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(data).into()
}

/// `jq-1.8.2` -> ("jq", "1.8.2").
fn split_name(name: &str) -> (String, String) {
    let version = crate::query::version_of(name);
    let pname = if version.is_empty() { name } else { name.strip_suffix(&format!("-{version}")).unwrap_or(name) };
    (pname.to_string(), version)
}

fn plan(store: &Store, index: &Index, opts: &Options) -> Result<Plan> {
    let what = opts.what.as_str();
    let path = Path::new(what);
    // A recipe.
    if crate::oldnames::is_file(&path.join(crate::build::RECIPE_FILE)) || path.file_name().is_some_and(|f| crate::oldnames::is_named(f, crate::build::RECIPE_FILE)) {
        let built = Builder::new(store, index).build(path)?;
        let (name, version) = split_name(&built.name);
        let tops = built.install_paths();
        let mut roots: Vec<String> = built.outputs.values().cloned().collect();
        roots.sort();
        return Ok(Plan { name, version, roots, tops, local: None, local_refs: Vec::new(), entry: None, loader: None });
    }
    // A project folder (Cargo.toml, go.mod, ...) or a release tarball.
    if let Some(plan) = project::plan(store, index, opts)? {
        return Ok(plan);
    }
    // A store path (or a file in one).
    if let Some(rest) = what.strip_prefix(&format!("{STORE}/")) {
        let base = rest.split('/').next().unwrap_or_default();
        let top = format!("{STORE}/{base}");
        store.realise(std::slice::from_ref(&top))?;
        let (name, version) = split_name(base.get(33..).unwrap_or(base));
        let entry = (rest.len() > base.len()).then(|| what.to_string());
        return Ok(Plan { name, version, roots: vec![top.clone()], tops: vec![top], local: None, local_refs: Vec::new(), entry, loader: None });
    }
    // A program (or a folder of one) from outside the store.
    if what.contains('/') || path.exists() {
        return local_program(store, index, path, opts.program.as_deref(), None);
    }
    // A package from the catalog.
    let p = index.get(what)?;
    let tops = p.install_paths();
    ensure!(!tops.is_empty(), "{what} has nothing ready-built in the catalog");
    store.realise(&tops).with_context(|| format!("getting {what}"))?;
    let (pname, version) = split_name(&p.name);
    let name = p.main_program.clone().filter(|m| opts.program.is_none() && !m.is_empty());
    let mut plan = Plan { name: pname, version, roots: tops.clone(), tops, local: None, local_refs: Vec::new(), entry: None, loader: None };
    if let Some(m) = name {
        let found = plan.tops.iter().map(|t| format!("{t}/bin/{m}")).find(|e| store.phys(e).exists());
        plan.entry = found;
    }
    Ok(plan)
}

/// `--arch aarch64`: a package from that architecture's catalog, or a store
/// path built for it elsewhere (CI on an ARM machine, cache.nixos.org).
/// Recipes and projects are built on this computer, for this one: to
/// bundle them for ARM, run `nami bundle` on an ARM computer.
fn arch_plan(store: &Store, index: &Index, opts: &Options) -> Result<Plan> {
    let what = opts.what.as_str();
    let arch = opts.arch.as_str();
    if let Some(rest) = what.strip_prefix(&format!("{STORE}/")) {
        let base = rest.split('/').next().unwrap_or_default();
        let top = format!("{STORE}/{base}");
        store.realise(std::slice::from_ref(&top))?;
        let (name, version) = split_name(base.get(33..).unwrap_or(base));
        let entry = (rest.len() > base.len()).then(|| what.to_string());
        return Ok(Plan { name, version, roots: vec![top.clone()], tops: vec![top], local: None, local_refs: Vec::new(), entry, loader: None });
    }
    if what.contains('/') || Path::new(what).exists() {
        let (computer, runner) = if arch == "aarch64" { ("an ARM", "an ARM runner (GitHub's ubuntu-24.04-arm)") } else { ("an x86_64", "an x86_64 runner (GitHub's ubuntu-24.04)") };
        bail!(
            "--arch {arch} bundles a package from the catalog (nami bundle jq --arch {arch}) or a store path built for {arch}, not {what}: \
             nami builds projects and recipes for the computer it runs on. Run `nami bundle {what}` on {computer} computer, or in CI on {runner}"
        );
    }
    let p = index.get(what)?;
    let tops = p.install_paths();
    ensure!(!tops.is_empty(), "{what} has nothing ready-built for {arch} in the catalog");
    store.realise(&tops).with_context(|| format!("getting {what} for {arch}"))?;
    let (pname, version) = split_name(&p.name);
    let mut plan = Plan { name: pname, version, roots: tops.clone(), tops, local: None, local_refs: Vec::new(), entry: None, loader: None };
    if let Some(m) = p.main_program.clone().filter(|m| opts.program.is_none() && !m.is_empty()) {
        plan.entry = plan.tops.iter().map(|t| format!("{t}/bin/{m}")).find(|e| store.phys(e).exists());
    }
    Ok(plan)
}

/// Another architecture's catalog: `index-<arch>-linux.json.xz` beside this
/// computer's (index-resolved), checked against its signature as `nami
/// update` checks the catalog, and kept in nami's cache;
/// NAMI_BUNDLE_INDEX_AARCH64 names one of your own (a file or a URL).
fn arch_index(store: &Store, arch: &str) -> Result<Index> {
    let system = format!("{arch}-linux");
    let var = format!("NAMI_BUNDLE_INDEX_{}", arch.to_uppercase());
    let (from, own) = match std::env::var(&var).ok().filter(|v| !v.is_empty()) {
        Some(v) => (v, true),
        None => {
            let resolved = fs::read_to_string(store.var_dir().join("index-resolved")).unwrap_or_default();
            let here = format!("index-{}", crate::index::SYSTEM);
            ensure!(resolved.contains(&here), "this computer's catalog ({}) has no {arch} one beside it: name one with {var}", resolved.trim());
            (resolved.trim().replace(&here, &format!("index-{system}")), false)
        }
    };
    let cache = store.cache_dir().join(format!("bundle-index-{system}-{}.json", &format::sha256_hex(from.as_bytes())[..16]));
    if !own {
        if let Ok(d) = fs::read(&cache) {
            return Index::from_json(store, &d);
        }
    }
    let get = |what: &str| -> Result<Option<Vec<u8>>> {
        if what.starts_with("https://") || what.starts_with("http://") {
            let mut d = Vec::new();
            match store.http().get(what).call() {
                Ok(r) => r.into_reader().read_to_end(&mut d)?,
                Err(ureq::Error::Status(404, _)) => return Ok(None),
                Err(e) => return Err(e).with_context(|| format!("fetching {what}")),
            };
            Ok(Some(d))
        } else {
            Ok(fs::read(what).ok())
        }
    };
    crate::say!("getting the catalog's {arch} packages: {from}");
    let raw = get(&from)?.with_context(|| format!("the catalog has no {arch} packages yet ({from} isn't there)"))?;
    if !own {
        let sig = get(&format!("{from}.sig"))?.with_context(|| format!("{from} isn't signed"))?;
        crate::catalog::verify(&raw, &String::from_utf8_lossy(&sig), &crate::catalog::trusted_keys(store)).with_context(|| format!("{from}: signature check failed"))?;
    }
    let mut data = Vec::new();
    if from.ends_with(".xz") {
        xz2::read::XzDecoder::new(&raw[..]).read_to_end(&mut data)?;
    } else {
        data = raw;
    }
    let index = Index::from_json(store, &data)?;
    if !own {
        let _ = crate::store::write_atomic(&cache, &data);
    }
    Ok(index)
}

/// A program built outside the store: one linked against the store's
/// libraries (in `nami shell`) goes in with its closure; one made for
/// other distributions (asking for /lib64's loader) gets nixpkgs' glibc and the
/// libraries it needs, and is started by that loader explicitly.
fn local_program(store: &Store, index: &Index, path: &Path, program: Option<&str>, base: Option<&str>) -> Result<Plan> {
    let path = path.canonicalize().with_context(|| format!("{} isn't there", path.display()))?;
    let (dir, entry_rel) = if path.is_dir() {
        let p = program.context("a folder: say which program in it runs with --program (its path inside the folder)")?;
        ensure!(path.join(p).is_file(), "{} has no {p}", path.display());
        (path.clone(), p.trim_start_matches("./").to_string())
    } else {
        (path.parent().unwrap().to_path_buf(), path.file_name().unwrap().to_string_lossy().into_owned())
    };
    let file_name = Path::new(&entry_rel).file_name().unwrap().to_string_lossy().into_owned();
    // A single file goes in alone (not its whole folder); a folder goes in whole.
    let staged = if path.is_dir() { dir.clone() } else { path.clone() };
    let mut files = Vec::new();
    walk(&staged, &mut files);
    let mut refs = BTreeSet::new();
    for f in &files {
        if let Ok(data) = fs::read(f) {
            refs.extend(store_refs(&data));
        }
    }
    let (hash, _) = crate::nar::hash_path(&staged)?;
    let label: String = file_name.chars().filter(|c| c.is_ascii_alphanumeric() || "-._+".contains(*c)).collect();
    // A store path's name: 32 letters of nix's alphabet (no e, o, u, t), then the name.
    let letters = crate::catalog::sha256_hex(hash.as_bytes())[..32].replace(['e', 't', 'u', 'o'], "0");
    let base = base.map(String::from).unwrap_or_else(|| format!("{letters}-{}", if label.is_empty() { "program" } else { label.as_str() }));
    // Its own path isn't one to get (a project's files name where they're installed).
    refs.remove(&base);
    let top = format!("{STORE}/{base}");
    let entry = if path.is_dir() { format!("{top}/{entry_rel}") } else { top.clone() };
    let local_refs: Vec<String> = refs.iter().cloned().collect();
    let mut roots: Vec<String> = refs.into_iter().map(|b| format!("{STORE}/{b}")).collect();
    let mut loader = None;
    let data = fs::read(if path.is_dir() { path.join(&entry_rel) } else { path.clone() })?;
    match crate::other_linux::kind_of(&data) {
        crate::other_linux::Kind::Foreign => {
            // What it needs that its own folder doesn't have, from the catalog.
            let own: BTreeSet<String> = files.iter().filter_map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
            let mut needed = BTreeSet::new();
            for f in &files {
                if let Some(l) = fs::read(f).ok().and_then(|d| crate::elf::linking(&d)) {
                    needed.extend(l.needed.into_iter().filter(|n| !own.contains(n)));
                }
            }
            let glibc_pkg = index.get("glibc")?.outputs.get("out").cloned().flatten().context("the catalog has no glibc")?;
            let mut pkgs = BTreeSet::new();
            let glibc_libs = ["libc.so.6", "libm.so.6", "libdl.so.2", "libpthread.so.0", "librt.so.1", crate::arch::LOADER_NAME, "libresolv.so.2", "libutil.so.1"];
            for n in needed.iter().filter(|n| !glibc_libs.contains(&n.as_str())) {
                match crate::recipe::package_with_library(store, index, n) {
                    Some(p) => {
                        pkgs.insert(p);
                    }
                    None => bail!("{} needs {n}, which no package in the catalog has", file_name),
                }
            }
            let mut lib_dirs = Vec::new();
            for p in &pkgs {
                let pkg = index.get(p)?;
                let out = pkg.outputs.get("lib").or_else(|| pkg.outputs.get("out")).cloned().flatten().with_context(|| format!("{p} has nothing ready-built"))?;
                roots.push(out.clone());
                lib_dirs.push(format!("{out}/lib"));
            }
            roots.push(glibc_pkg.clone());
            store.realise(&roots)?;
            // Its own libraries first (in its folder, or lib/ beside it).
            let mut library_path: Vec<String> = Vec::new();
            for f in &files {
                if f.file_name().is_some_and(|n| n.to_string_lossy().contains(".so")) {
                    if let Some(parent) = f.parent().and_then(|p| p.strip_prefix(&staged).ok()) {
                        let d = if parent.as_os_str().is_empty() { top.clone() } else { format!("{top}/{}", parent.display()) };
                        if !library_path.contains(&d) {
                            library_path.push(d);
                        }
                    }
                }
            }
            library_path.extend(lib_dirs);
            library_path.push(format!("{glibc_pkg}/lib"));
            loader = Some(Loader { path: format!("{glibc_pkg}/lib/{}", crate::arch::LOADER_NAME), library_path });
            crate::say!("{file_name} is built for other distributions: it gets glibc and {}", if pkgs.is_empty() { "nothing else".to_string() } else { pkgs.iter().cloned().collect::<Vec<_>>().join(", ") });
        }
        crate::other_linux::Kind::Native | crate::other_linux::Kind::Script => store.realise(&roots)?,
        k => bail!("{} can't be bundled ({k:?}): nami bundles 64-bit Linux programs", path.display()),
    }
    let (name, version) = (label, String::new());
    Ok(Plan { name, version, roots, tops: Vec::new(), local: Some((base, staged)), local_refs, entry: Some(entry), loader })
}

fn walk(p: &Path, out: &mut Vec<PathBuf>) {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => {
            for e in fs::read_dir(p).into_iter().flatten().flatten() {
                walk(&e.path(), out);
            }
        }
        Ok(m) if m.is_file() => out.push(p.to_path_buf()),
        _ => {}
    }
}

/// The store paths a file mentions (`/nix/store/<hash>-<name>`), as Nix
/// finds references.
fn store_refs(data: &[u8]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let pat = b"/nix/store/";
    let mut i = 0;
    while let Some(at) = data[i..].windows(pat.len()).position(|w| w == pat) {
        let start = i + at + pat.len();
        let end = data[start..].iter().position(|b| !(b.is_ascii_alphanumeric() || b"+-._?=".contains(b))).map_or(data.len(), |e| start + e);
        if let Ok(s) = std::str::from_utf8(&data[start..end]) {
            // (A hash Nix never makes, e.g. remove-references-to's eeee…, names nothing.)
            if format::valid_name(s) && s[..32].bytes().all(|c| b"0123456789abcdfghijklmnpqrsvwxyz".contains(&c)) {
                out.insert(s.to_string());
            }
        }
        i = end.max(start);
    }
    out
}

/// Every shared library under `dir` by its soname, at its store path.
fn sonames(dir: &Path, logical: &str, out: &mut BTreeMap<String, String>) {
    let mut files = Vec::new();
    for sub in ["lib", "lib64"] {
        walk(&dir.join(sub), &mut files);
    }
    for f in files {
        let name = f.file_name().unwrap_or_default().to_string_lossy();
        if !name.contains(".so") {
            continue;
        }
        let Ok(mut file) = fs::File::open(&f) else { continue };
        let mut head = [0u8; 4];
        if file.read_exact(&mut head).is_err() || &head != b"\x7fELF" {
            continue;
        }
        let Ok(data) = fs::read(&f) else { continue };
        if let Some(l) = crate::elf::linking(&data) {
            let soname = l.soname.unwrap_or_else(|| name.to_string());
            let rel = f.strip_prefix(dir).unwrap();
            out.entry(soname).or_insert_with(|| format!("{logical}/{}", rel.display()));
        }
    }
}

/// Each path's NAR, zstd-compressed: one frame each, in parallel.
/// A compressed frame, and the size of the NAR in it.
type Frame = (Vec<u8>, u64);

fn compress(closure: &[String], phys: &(dyn Fn(&str) -> PathBuf + Sync), level: i32) -> Result<Vec<Frame>> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<Result<Frame>>>> = closure.iter().map(|_| std::sync::Mutex::new(None)).collect();
    let jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    let done = std::sync::atomic::AtomicUsize::new(0);
    crate::log::status(&format!("Compressing {} parts", closure.len()));
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= closure.len() {
                    break;
                }
                let r = (|| -> Result<Frame> {
                    // Its size first (the encoder is told it), then the NAR, as
                    // a runtime makes it again from a patch (src/bundle_delta.rs).
                    let path = phys(&closure[i]);
                    let mut size = Counting { inner: std::io::sink(), n: 0 };
                    crate::nar::dump(&path, &mut size).with_context(|| format!("reading {}", closure[i]))?;
                    let mut enc = crate::bundle_delta::frame_encoder(Vec::new(), level, size.n)?;
                    let mut counting = Counting { inner: &mut enc, n: 0 };
                    crate::nar::dump(&path, &mut counting).with_context(|| format!("reading {}", closure[i]))?;
                    let n = counting.n;
                    ensure!(n == size.n, "{} changed while it was read", closure[i]);
                    Ok((enc.finish()?, n))
                })();
                *results[i].lock().unwrap() = Some(r);
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                crate::log::status(&format!("Compressing {n} of {} parts", closure.len()));
            });
        }
    });
    results.into_iter().map(|m| m.into_inner().unwrap().unwrap()).collect()
}

struct Counting<W> {
    inner: W,
    n: u64,
}

impl<W: Write> Write for Counting<W> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(b)?;
        self.n += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// The app's .desktop file (the one that runs `entry`, else the first) and
/// its icon: a file, by its name in the app's own icons, else in any icon
/// theme of the store paths it carries (`bases`), so wherever it's put in a
/// menu it has it.
fn desktop(plan: &Plan, phys: &dyn Fn(&str) -> PathBuf, entry: &str, bases: &[String]) -> Option<Desktop> {
    let mut found: Vec<(String, String, String)> = Vec::new(); // (top, id, text)
    for top in &plan.tops {
        let dir = phys(base_name(top)).join("share/applications");
        let mut here: Vec<_> = fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "desktop")).collect();
        here.sort();
        for f in here {
            if let Ok(text) = fs::read_to_string(&f) {
                found.push((top.clone(), f.file_name().unwrap().to_string_lossy().into_owned(), text));
            }
        }
    }
    let runs = |text: &str| {
        text.lines().filter_map(|l| l.strip_prefix("Exec=")).any(|e| {
            let prog = e.split_whitespace().next().unwrap_or("");
            prog.rsplit('/').next() == Some(entry)
        })
    };
    let shown = |text: &str| !text.lines().any(|l| l == "NoDisplay=true");
    let pick = found.iter().find(|(_, _, t)| runs(t) && shown(t)).or_else(|| found.iter().find(|(_, _, t)| shown(t)))?;
    let (top, id, text) = pick.clone();
    let icon_name = text.lines().find_map(|l| l.strip_prefix("Icon=")).map(str::trim).unwrap_or("").to_string();
    let icon = if icon_name.is_empty() {
        None
    } else if icon_name.starts_with('/') {
        Some(icon_name.clone()).filter(|i| i.starts_with(STORE))
    } else {
        let dir = phys(base_name(&top));
        let candidates = ["256x256", "512x512", "scalable", "128x128", "192x192", "96x96", "64x64", "48x48"]
            .iter()
            .flat_map(|size| ["png", "svg"].map(|ext| format!("share/icons/hicolor/{size}/apps/{icon_name}.{ext}")))
            .chain(["png", "svg"].map(|ext| format!("share/pixmaps/{icon_name}.{ext}")));
        candidates.into_iter().find(|rel| dir.join(rel).exists()).map(|rel| format!("{top}/{rel}")).or_else(|| theme_icon(&icon_name, base_name(&top), bases, phys))
    };
    Some(Desktop { id, file: text, icon })
}

/// An icon by its name, among the store paths `bases` (`own` first): the
/// best picture of it any of their icon themes has (bundle_menu::icon_score).
fn theme_icon(name: &str, own: &str, bases: &[String], phys: &dyn Fn(&str) -> PathBuf) -> Option<String> {
    let mut best: Option<(u32, String)> = None;
    let dirs = |d: &Path| -> Vec<String> { fs::read_dir(d).into_iter().flatten().flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect() };
    for base in std::iter::once(own).chain(bases.iter().map(String::as_str).filter(|b| *b != own)) {
        let root = phys(base);
        let mut look = |rel: String| {
            if let Some(score) = crate::bundle_menu::icon_score(&rel, name) {
                if best.as_ref().is_none_or(|b| score > b.0) && root.join(&rel).is_file() {
                    best = Some((score, format!("{STORE}/{base}/{rel}")));
                }
            }
        };
        let files = [format!("{name}.png"), format!("{name}.svg"), format!("{name}-symbolic.svg"), format!("{name}-symbolic.png")];
        for f in &files {
            look(format!("share/pixmaps/{f}"));
        }
        let icons = root.join("share/icons");
        for theme in dirs(&icons) {
            for a in dirs(&icons.join(&theme)) {
                for b in dirs(&icons.join(&theme).join(&a)) {
                    for f in &files {
                        look(format!("share/icons/{theme}/{a}/{b}/{f}"));
                    }
                }
            }
        }
    }
    best.map(|b| b.1)
}

/// `<bundle>.desktop` and its icon beside the bundle, for whoever packages it.
fn write_desktop(d: &Desktop, out: &Path, phys: &dyn Fn(&str) -> PathBuf) -> Result<()> {
    let abs = fs::canonicalize(out)?;
    let stem = out.file_name().unwrap().to_string_lossy().into_owned();
    let mut text = String::new();
    let mut icon_file = None;
    if let Some(icon) = &d.icon {
        let ext = Path::new(icon).extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "png".into());
        let dest = out.with_file_name(format!("{stem}.{ext}"));
        let (base, rest) = split_store(icon);
        let src = phys(base).join(rest);
        let _ = fs::remove_file(&dest);
        fs::copy(&src, &dest).with_context(|| format!("copying the icon {}", src.display()))?;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o644))?;
        icon_file = Some(fs::canonicalize(&dest)?);
    }
    for line in d.file.lines() {
        let line = match line.split_once('=') {
            Some(("Exec", v)) => format!("Exec={}{}", abs.display(), v.split_once(' ').map(|(_, r)| format!(" {r}")).unwrap_or_default()),
            Some(("TryExec", _)) => continue,
            Some(("DBusActivatable", _)) => "DBusActivatable=false".into(),
            Some(("Icon", _)) if icon_file.is_some() => format!("Icon={}", icon_file.as_ref().unwrap().display()),
            _ => line.to_string(),
        };
        text += &line;
        text.push('\n');
    }
    let file = out.with_file_name(format!("{stem}.desktop"));
    fs::write(&file, text)?;
    crate::say!("{} and its icon: the app menu entry (or run it with --nami-bundle-install)", file.display());
    Ok(())
}

/// AppImage's own runtime (type 2, static, FUSE 3 inside), pinned.
const APPIMAGE_RUNTIME: &str = "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64";
const APPIMAGE_RUNTIME_SHA256: &str = "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d";

/// The same program as an AppImage: AppImage's runtime and a squashfs of
/// an AppDir whose AppRun is the bundle runtime, with the store paths as
/// files in .tray-bundle/store (as tray named it). Started, AppRun copies them into the same
/// cache a bundle unpacks into, and runs the program the same way.
#[allow(clippy::too_many_arguments)]
fn appimage(store: &Store, index: &Index, meta: &Meta, closure: &[String], phys: &dyn Fn(&str) -> PathBuf, runtime: &[u8], out: &Path, entry_name: &str) -> Result<PathBuf> {
    let cached = store.cache_dir().join(format!("appimage-runtime-{}", &APPIMAGE_RUNTIME_SHA256[..16]));
    let ai_runtime = match fs::read(&cached) {
        Ok(d) if crate::catalog::sha256_hex(&d) == APPIMAGE_RUNTIME_SHA256 => d,
        _ => {
            crate::say!("fetching AppImage's runtime");
            let mut d = Vec::new();
            store.http().get(APPIMAGE_RUNTIME).call().with_context(|| format!("fetching {APPIMAGE_RUNTIME}"))?.into_reader().take(16 << 20).read_to_end(&mut d)?;
            ensure!(crate::catalog::sha256_hex(&d) == APPIMAGE_RUNTIME_SHA256, "{APPIMAGE_RUNTIME} isn't the file nami knows (its sha256 differs)");
            fs::create_dir_all(cached.parent().unwrap())?;
            let _ = crate::store::write_atomic(&cached, &d);
            d
        }
    };
    let tools = index.get("squashfs-tools")?.install_paths();
    store.realise(&tools)?;
    let mksquashfs = tools.iter().map(|t| store.phys(&format!("{t}/bin/mksquashfs"))).find(|p| p.exists()).context("squashfs-tools has no mksquashfs")?;

    let name = out.file_name().unwrap().to_string_lossy().into_owned();
    let file = out.with_file_name(format!("{name}.AppImage"));
    let dir = out.with_file_name(format!(".{name}.AppDir.tmp{}", std::process::id()));
    let squash = out.with_file_name(format!(".{name}.squashfs.tmp{}", std::process::id()));
    // Not store::remove_tree: it makes files writable, and these are the store's own (hard links).
    let _ = fs::remove_dir_all(&dir);
    let result = (|| -> Result<()> {
        fs::create_dir_all(dir.join(".tray-bundle/store"))?;
        fs::write(dir.join("AppRun"), runtime)?;
        fs::set_permissions(dir.join("AppRun"), fs::Permissions::from_mode(0o755))?;
        let mut m = meta.clone();
        m.runtime = String::new();
        for p in &mut m.paths {
            (p.offset, p.size, p.sha256) = (0, 0, String::new());
        }
        fs::write(dir.join(".tray-bundle/meta.json"), serde_json::to_vec(&m)?)?;
        for base in closure {
            link_tree(&phys(base), &dir.join(".tray-bundle/store").join(base))?;
        }
        // The .desktop file and icon AppImage tools look for at the top.
        let (desktop, icon) = match &meta.desktop {
            Some(d) => (d.file.clone(), d.icon.clone()),
            None => (format!("[Desktop Entry]\nType=Application\nName={}\nExec={entry_name}\nIcon={entry_name}\nTerminal=true\nCategories=Utility;\n", meta.name), None),
        };
        let id = meta.desktop.as_ref().map(|d| d.id.trim_end_matches(".desktop").to_string()).unwrap_or_else(|| entry_name.to_string());
        let mut text = String::new();
        for line in desktop.lines() {
            match line.split_once('=') {
                Some(("Exec", v)) => text += &format!("Exec={entry_name}{}\n", v.split_once(' ').map(|(_, r)| format!(" {r}")).unwrap_or_default()),
                Some(("Icon", _)) => text += &format!("Icon={id}\n"),
                Some(("TryExec", _)) => {}
                _ => text += &format!("{line}\n"),
            }
        }
        fs::write(dir.join(format!("{id}.desktop")), text)?;
        let icon_file = match icon {
            Some(i) => {
                let (base, rest) = split_store(&i);
                let ext = Path::new(&i).extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "png".into());
                fs::copy(phys(base).join(rest), dir.join(format!("{id}.{ext}")))?;
                format!("{id}.{ext}")
            }
            None => {
                fs::write(dir.join(format!("{id}.svg")), "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\"><rect x=\"6\" y=\"10\" width=\"52\" height=\"44\" rx=\"6\" fill=\"#334\"/><path d=\"M16 26l8 6-8 6M28 40h14\" stroke=\"#fff\" stroke-width=\"4\" fill=\"none\"/></svg>\n")?;
                format!("{id}.svg")
            }
        };
        std::os::unix::fs::symlink(&icon_file, dir.join(".DirIcon"))?;
        let status = std::process::Command::new(&mksquashfs)
            .arg(&dir)
            .arg(&squash)
            .args(["-root-owned", "-noappend", "-no-progress", "-quiet", "-comp", "zstd", "-Xcompression-level", "19", "-b", "1M"])
            .status()
            .context("running mksquashfs")?;
        ensure!(status.success(), "mksquashfs failed");
        let mut f = std::io::BufWriter::new(fs::File::create(&file)?);
        f.write_all(&ai_runtime)?;
        std::io::copy(&mut fs::File::open(&squash)?, &mut f)?;
        f.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755))?;
        Ok(())
    })();
    // Not store::remove_tree: it makes files writable, and these are the store's own (hard links).
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_file(&squash);
    result.map(|_| file)
}

/// A copy of `src` at `dest`, its files hard links where they can be.
fn link_tree(src: &Path, dest: &Path) -> Result<()> {
    let m = fs::symlink_metadata(src)?;
    if m.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(src)?, dest)?;
    } else if m.is_dir() {
        fs::create_dir(dest)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            link_tree(&e.path(), &dest.join(e.file_name()))?;
        }
    } else if fs::hard_link(src, dest).is_err() {
        fs::copy(src, dest)?;
    }
    Ok(())
}

/// The runtime a bundle starts with (`net`: the one that can fetch, for
/// thin bundles), for `arch`: what NAMI_BUNDLE_RUNTIME (or
/// NAMI_BUNDLE_RUNTIME_NET; for aarch64, NAMI_BUNDLE_RUNTIME_AARCH64 and
/// NAMI_BUNDLE_RUNTIME_NET_AARCH64) names, else the one appended to nami
/// itself (build.sh appends them), else nami-bundle-runtime(-net)(-aarch64)
/// beside nami. It must be static: it runs before anything of the bundle's
/// is there.
pub fn runtime_bytes(net: bool, arch: &str) -> Result<Vec<u8>> {
    let exe = std::env::current_exe()?;
    let arm = arch == "aarch64";
    let (var, file, magic) = match (net, arm) {
        (false, false) => ("NAMI_BUNDLE_RUNTIME", "nami-bundle-runtime", RUNTIME_MAGIC),
        (true, false) => ("NAMI_BUNDLE_RUNTIME_NET", "nami-bundle-runtime-net", RUNTIME_NET_MAGIC),
        (false, true) => ("NAMI_BUNDLE_RUNTIME_AARCH64", "nami-bundle-runtime-aarch64", RUNTIME_ARM_MAGIC),
        (true, true) => ("NAMI_BUNDLE_RUNTIME_NET_AARCH64", "nami-bundle-runtime-net-aarch64", RUNTIME_ARM_NET_MAGIC),
    };
    let data = if let Some(p) = std::env::var_os(var) {
        fs::read(&p).with_context(|| format!("reading {var} ({})", Path::new(&p).display()))?
    } else if let Some(d) = appended_runtimes(&exe)?.into_iter().find(|(m, _)| m == magic).map(|(_, d)| d) {
        d
    } else {
        // (Beside it, this computer's own are nami-bundle-runtime(-net): an ARM one's too.)
        let file = if arch == crate::arch::ARCH { file.trim_end_matches("-aarch64") } else { file };
        let beside = exe.with_file_name(file);
        let how = if arch == crate::arch::ARCH { "./build.sh makes them" } else if arm { "ARM=1 ./build.sh makes them" } else { "an x86_64 computer's ./build.sh makes them" };
        fs::read(&beside).with_context(|| format!("this nami carries no {file}, and there's none at {} ({how})", beside.display()))?
    };
    ensure!(data.starts_with(b"\x7fELF"), "the bundle runtime isn't a program");
    ensure!(format::elf_arch(&data) == Some(arch), "the bundle runtime {file} is for {}, not {arch}", format::elf_arch(&data).unwrap_or("another architecture"));
    if crate::elf::linking(&data).is_some_and(|l| l.program) {
        bail!("the bundle runtime is linked to this computer's libraries: it must be static (./build.sh builds it with musl)");
    }
    Ok(data)
}

/// The fetching runtime's mark, after the plain one's.
const RUNTIME_NET_MAGIC: &[u8; 8] = b"NAMIRTN1";
/// The aarch64 runtimes' marks (`--arch aarch64`).
const RUNTIME_ARM_MAGIC: &[u8; 8] = b"NAMIRTA1";
const RUNTIME_ARM_NET_MAGIC: &[u8; 8] = b"NAMIRTB1";
const MAGICS: [&[u8; 8]; 4] = [RUNTIME_MAGIC, RUNTIME_NET_MAGIC, RUNTIME_ARM_MAGIC, RUNTIME_ARM_NET_MAGIC];

/// Append the runtimes to the nami at `nami` (replacing those it carries),
/// so one file has them all: build.sh does this after building (with
/// ARM=1, the aarch64 ones too). Each is marked for what it is: the plain
/// or the fetching one (its place), for x86_64 or aarch64 (its ELF header),
/// so an ARM computer's nami, whose own runtimes come first, has them as
/// the aarch64 ones.
pub fn attach_runtime(nami: &Path, runtime: &Path, net: Option<&Path>, arm: Option<&Path>, arm_net: Option<&Path>) -> Result<()> {
    let mut data = fs::read(nami).with_context(|| format!("reading {}", nami.display()))?;
    let carried: usize = appended_runtimes(nami)?.iter().map(|(_, d)| d.len() + 16).sum();
    data.truncate(data.len() - carried);
    for (file, fetches) in [(Some(runtime), false), (net, true), (arm, false), (arm_net, true)] {
        let Some(file) = file else { continue };
        let rt = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
        ensure!(rt.starts_with(b"\x7fELF") && !crate::elf::linking(&rt).is_some_and(|l| l.program), "{} isn't a static program", file.display());
        let magic = match (format::elf_arch(&rt), fetches) {
            (Some("aarch64"), false) => RUNTIME_ARM_MAGIC,
            (Some("aarch64"), true) => RUNTIME_ARM_NET_MAGIC,
            (Some("x86_64"), false) => RUNTIME_MAGIC,
            (Some("x86_64"), true) => RUNTIME_NET_MAGIC,
            _ => bail!("{} is a program for neither x86_64 nor aarch64", file.display()),
        };
        data.extend(&rt);
        data.extend((rt.len() as u64).to_le_bytes());
        data.extend(magic);
    }
    let tmp = nami.with_extension("attach.tmp");
    fs::write(&tmp, &data)?;
    fs::set_permissions(&tmp, fs::metadata(nami)?.permissions())?;
    fs::rename(&tmp, nami)?;
    Ok(())
}

/// `[runtime][length: u64][mark]`, each runtime so, at the end of nami
/// (last first).
fn appended_runtimes(exe: &Path) -> Result<Vec<([u8; 8], Vec<u8>)>> {
    let mut f = fs::File::open(exe)?;
    let mut end = f.seek(SeekFrom::End(0))?;
    let mut out = Vec::new();
    while end >= 16 {
        f.seek(SeekFrom::Start(end - 16))?;
        let mut tail = [0u8; 16];
        f.read_exact(&mut tail)?;
        let magic: [u8; 8] = tail[8..].try_into().unwrap();
        if !MAGICS.contains(&&magic) {
            break;
        }
        let n = u64::from_le_bytes(tail[..8].try_into().unwrap());
        ensure!(n + 16 <= end, "nami's appended bundle runtime is damaged");
        f.seek(SeekFrom::Start(end - 16 - n))?;
        let mut data = vec![0u8; n as usize];
        f.read_exact(&mut data)?;
        out.push((magic, data));
        end -= n + 16;
    }
    Ok(out)
}

/// The parts cache.nixos.org has, the very same (its narinfo, signed,
/// says the NAR hash the store has): what a thin bundle leaves out.
fn in_public_cache(store: &Store, closure: &[String], info: &BTreeMap<String, (String, Vec<String>)>) -> BTreeSet<String> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let found = std::sync::Mutex::new(BTreeSet::new());
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(base) = closure.get(i) else { break };
                let Some(hash) = base.get(..32) else { continue };
                let Ok(resp) = store.http().get(&format!("{PUBLIC_CACHE}/{hash}.narinfo")).call() else { continue };
                let Ok(text) = resp.into_string() else { continue };
                let Ok(n) = crate::narinfo::NarInfo::parse(&text) else { continue };
                if n.base_name() == base && n.nar_hash == info[base].0 && n.verify(&[crate::store::NIXOS_CACHE_KEY]).is_ok() {
                    found.lock().unwrap().insert(base.clone());
                }
            });
        }
    });
    found.into_inner().unwrap()
}

/// `nami bundle --verify`: the file against its own hash.
pub fn verify(path: &Path) -> Result<(Meta, String)> {
    let mut f = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let (trailer, meta) = format::read(&mut f).with_context(|| format!("{}", path.display()))?;
    format::verify(&mut f, &trailer, &meta).with_context(|| format!("{}", path.display()))?;
    Ok((meta, trailer.hash()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_store_references() {
        let data = b"\0/nix/store/0123456789abcdfghijklmnpqrsvwxyz-glibc-2.44/lib/ld-linux.so\0 /nix/store/short /nix/store/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-zig-0.15/lib /nix/store/0123456789abcdfghijklmnpqrsvwxyz-jq-1.8:x";
        let refs = store_refs(data);
        assert_eq!(refs.into_iter().collect::<Vec<_>>(), vec!["0123456789abcdfghijklmnpqrsvwxyz-glibc-2.44", "0123456789abcdfghijklmnpqrsvwxyz-jq-1.8"]);
    }

    #[test]
    fn the_runtimes_nar_hash_is_nixs() {
        // The runtime tells a host's copy of a part by this: it must be the
        // hash nami (and Nix) record for it.
        let dir = std::env::temp_dir().join(format!("nami-bundle-narhash-{}", std::process::id()));
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join("bin/x"), "#!/bin/sh\necho x\n").unwrap();
        fs::set_permissions(dir.join("bin/x"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(dir.join("b"), "seven b").unwrap();
        std::os::unix::fs::symlink("bin/x", dir.join("a")).unwrap();
        assert_eq!(format::nar_hash(&dir).unwrap(), crate::nar::hash_path(&dir).unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn carries_two_runtimes() {
        let dir = std::env::temp_dir().join(format!("nami-bundle-attach-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // Static "programs": an ELF header (x86_64's, or aarch64's) with no program headers.
        let elf = |tag: u8, machine: u8| {
            let mut d = b"\x7fELF\x02\x01\x01".to_vec();
            d.resize(64, 0);
            d[18] = machine;
            d.push(tag);
            d
        };
        fs::write(dir.join("nami"), b"nami itself").unwrap();
        fs::write(dir.join("rt"), elf(1, 0x3e)).unwrap();
        fs::write(dir.join("net"), elf(2, 0x3e)).unwrap();
        for _ in 0..2 {
            // Again replaces them, rather than adding.
            attach_runtime(&dir.join("nami"), &dir.join("rt"), Some(&dir.join("net")), None, None).unwrap();
        }
        let got = appended_runtimes(&dir.join("nami")).unwrap();
        assert_eq!(got, vec![(*RUNTIME_NET_MAGIC, elf(2, 0x3e)), (*RUNTIME_MAGIC, elf(1, 0x3e))]);
        let all = fs::read(dir.join("nami")).unwrap();
        assert!(all.starts_with(b"nami itself") && all.len() == 11 + 2 * (65 + 16));
        // An ARM computer's nami: its own runtimes first, aarch64's, marked as the ARM ones.
        fs::write(dir.join("rt"), elf(3, 0xb7)).unwrap();
        fs::write(dir.join("net"), elf(4, 0xb7)).unwrap();
        attach_runtime(&dir.join("nami"), &dir.join("rt"), Some(&dir.join("net")), None, None).unwrap();
        assert_eq!(appended_runtimes(&dir.join("nami")).unwrap(), vec![(*RUNTIME_ARM_NET_MAGIC, elf(4, 0xb7)), (*RUNTIME_ARM_MAGIC, elf(3, 0xb7))]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn splits_store_paths() {
        assert_eq!(split_store("/nix/store/0123456789abcdfghijklmnpqrsvwxyz-a-1/share/x.svg"), ("0123456789abcdfghijklmnpqrsvwxyz-a-1", "share/x.svg"));
        assert_eq!(split_store("/nix/store/0123456789abcdfghijklmnpqrsvwxyz-a-1"), ("0123456789abcdfghijklmnpqrsvwxyz-a-1", ""));
    }

    #[test]
    fn names_and_versions() {
        assert_eq!(split_name("jq-1.8.2"), ("jq".into(), "1.8.2".into()));
        assert_eq!(split_name("gnome-text-editor-50.1"), ("gnome-text-editor".into(), "50.1".into()));
        assert_eq!(split_name("hello"), ("hello".into(), String::new()));
    }

    #[test]
    fn picks_the_apps_desktop_file_and_icon() {
        let dir = std::env::temp_dir().join(format!("nami-bundle-desktop-{}", std::process::id()));
        let top = "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-app-1";
        let root = dir.join("0123456789abcdfghijklmnpqrsvwxyz-app-1");
        fs::create_dir_all(root.join("share/applications")).unwrap();
        fs::create_dir_all(root.join("share/icons/hicolor/scalable/apps")).unwrap();
        fs::write(root.join("share/applications/a-helper.desktop"), "[Desktop Entry]\nExec=helper\nNoDisplay=true\n").unwrap();
        fs::write(root.join("share/applications/org.x.App.desktop"), "[Desktop Entry]\nName=App\nExec=app %U\nIcon=org.x.App\n").unwrap();
        fs::write(root.join("share/icons/hicolor/scalable/apps/org.x.App.svg"), "<svg/>").unwrap();
        let plan = Plan { name: "app".into(), version: "1".into(), roots: vec![], tops: vec![top.into()], local: None, local_refs: Vec::new(), entry: None, loader: None };
        let phys = |b: &str| dir.join(b);
        let d = desktop(&plan, &phys, "app", &[]).unwrap();
        assert_eq!(d.id, "org.x.App.desktop");
        assert_eq!(d.icon.as_deref(), Some(format!("{top}/share/icons/hicolor/scalable/apps/org.x.App.svg").as_str()));
        // Written beside a bundle: Exec runs the bundle, Icon is the copied file.
        let out = dir.join("app-x86_64");
        fs::write(&out, "").unwrap();
        write_desktop(&d, &out, &phys).unwrap();
        let text = fs::read_to_string(dir.join("app-x86_64.desktop")).unwrap();
        assert!(text.contains(&format!("Exec={} %U", fs::canonicalize(&out).unwrap().display())), "{text}");
        assert!(dir.join("app-x86_64.svg").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_theme_icon_by_its_name_from_what_it_carries() {
        let dir = std::env::temp_dir().join(format!("nami-bundle-theme-{}", std::process::id()));
        let (app, theme) = ("0123456789abcdfghijklmnpqrsvwxyz-game-1", "1123456789abcdfghijklmnpqrsvwxyz-adwaita-icon-theme-50");
        fs::create_dir_all(dir.join(app).join("share/applications")).unwrap();
        fs::write(dir.join(app).join("share/applications/game.desktop"), "[Desktop Entry]\nName=Game\nExec=game\nIcon=applications-games\n").unwrap();
        let t = dir.join(theme).join("share/icons/Adwaita");
        for d in ["symbolic/categories", "48x48/apps", "256x256/categories"] {
            fs::create_dir_all(t.join(d)).unwrap();
        }
        fs::write(t.join("symbolic/categories/applications-games-symbolic.svg"), "<svg/>").unwrap();
        let plan = Plan { name: "game".into(), version: "1".into(), roots: vec![], tops: vec![format!("{STORE}/{app}")], local: None, local_refs: Vec::new(), entry: None, loader: None };
        let phys = |b: &str| dir.join(b);
        let bases = vec![app.to_string(), theme.to_string()];
        // Only symbolic there: that one.
        let d = desktop(&plan, &phys, "game", &bases).unwrap();
        assert_eq!(d.icon, Some(format!("{STORE}/{theme}/share/icons/Adwaita/symbolic/categories/applications-games-symbolic.svg")));
        // A full-colour one, the biggest, over it.
        fs::write(t.join("48x48/apps/applications-games.png"), "png").unwrap();
        fs::write(t.join("256x256/categories/applications-games.png"), "png").unwrap();
        assert_eq!(desktop(&plan, &phys, "game", &bases).unwrap().icon, Some(format!("{STORE}/{theme}/share/icons/Adwaita/256x256/categories/applications-games.png")));
        // Not one it doesn't carry.
        assert_eq!(desktop(&plan, &phys, "game", &bases[..1]).unwrap().icon, None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
