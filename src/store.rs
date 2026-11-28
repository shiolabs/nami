//! The store: `/nix/store`, filled from binary caches without Nix.
//!
//! nami keeps its own database of the store paths it installed in
//! `/nami/db`. Paths in `/nix/store` that nami did not register are
//! never touched, so a pre-existing Nix installation is left alone.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::nar;
use crate::narinfo::NarInfo;

/// Logical store directory. Nixpkgs binaries hardcode it, so it can't move.
pub const STORE_DIR: &str = "/nix/store";
pub const NIXOS_CACHE: &str = "https://cache.nixos.org";
pub const NIXOS_CACHE_KEY: &str = "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=";

const JOBS: usize = 8;

#[derive(Serialize, Deserialize)]
pub struct PathInfo {
    pub references: Vec<String>,
    pub nar_hash: String,
    pub nar_size: u64,
    /// Built on this machine, rather than downloaded (what `nami push` uploads).
    #[serde(default)]
    pub built: bool,
}

/// How `Store::gc_with` goes.
#[derive(Debug, Default, Clone, Copy)]
pub struct Sweep {
    /// Paths registered less than this many seconds ago stay (and what they use): one a
    /// person's install has just fetched may not be in their step yet (the cleanup that
    /// runs by itself; `nami gc` takes what nothing uses at once).
    pub spare_newer_than: Option<u64>,
    /// Say what goes as it goes (`removing 480 paths (2300.0 MiB)`, then each, as
    /// fetching says them: a terminal's live line, Settings' bar). Not the daily cleanup,
    /// whose log has its one line.
    pub progress: bool,
}

/// A binary cache: an HTTP(S) URL or a local directory, and the public key
/// its narinfos must be signed with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cache {
    pub url: String,
    pub key: String,
}

pub struct Store {
    /// Filesystem prefix; `/` normally. Lets tests unpack into a scratch dir.
    root: PathBuf,
    http: ureq::Agent,
    /// cache.nixos.org first, then root's caches (`caches.json`), then the
    /// ones the trusted catalog names (`catalog-caches.json`).
    caches: std::sync::RwLock<Vec<Cache>>,
    /// Held for the whole run by root's nami; per request by the daemon.
    _lock: Option<File>,
    /// Not root: the store is changed through the daemon at this socket.
    daemon: Option<PathBuf>,
    /// Nix is installed: its store, so changes go through it (nix/store.rs).
    nix: Option<crate::nix::store::Nix>,
}

/// The caches a signed catalog names, for the packages it adds to nixpkgs
/// (written by `Index::update`, only when the signature checked out).
pub const CATALOG_CACHES: &str = "catalog-caches.json";

/// The daemon's socket, in nami's folder (`/nami/daemon.sock`; [`Store::var_dir`]).
pub const DAEMON_SOCKET: &str = "daemon.sock";

/// The daemon's socket on the computer at `root`.
pub fn daemon_socket(root: &Path) -> PathBuf {
    crate::migrate::var_dir(root).join(DAEMON_SOCKET)
}

impl Store {
    /// Root gets direct access, holding the state lock; everyone else talks
    /// to the daemon.
    pub fn open(root: &Path) -> Result<Self> {
        if crate::profile::is_root() {
            Self::open_direct(root, true)
        } else {
            // (tray's place, while root hasn't run nami to move it.)
            let socket = daemon_socket(root);
            if !socket.exists() {
                bail!("the nami daemon isn't running ({} is missing); run nami as root, or start it (install.sh sets it up)", socket.display());
            }
            let mut store = Self::open_direct_readonly(root)?;
            store.daemon = Some(socket);
            Ok(store)
        }
    }

    /// For root. `lock: false` is for the daemon, which locks per request.
    pub fn open_direct(root: &Path, lock: bool) -> Result<Self> {
        let store = root.join("nix/store");
        // (On a new computer: /nami, or /nix/var/nami where `/` can't hold it; migrate.rs.)
        let var = crate::migrate::make(root).with_context(|| format!("can't create {} (nami needs root to manage /nix)", crate::migrate::var_dir(root).display()))?;
        for dir in [&store, &var.join("db"), &var.join("profiles/per-user")] {
            fs::create_dir_all(dir).with_context(|| {
                format!("can't create {} (nami needs root to manage /nix)", dir.display())
            })?;
        }
        // Users create their own profile directory in here, like /tmp.
        fs::set_permissions(var.join("profiles/per-user"), fs::Permissions::from_mode(0o1777))?;
        let mut s = Self::open_direct_readonly(root)?;
        if lock {
            s._lock = Some(s.lock()?);
        }
        Ok(s)
    }

    pub fn open_direct_readonly(root: &Path) -> Result<Self> {
        let http = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(60))
            .user_agent(concat!("nami/", env!("CARGO_PKG_VERSION")))
            .build();
        let nix = crate::nix::store::Nix::detect(root);
        let store = Store { root: root.to_path_buf(), http, caches: std::sync::RwLock::new(Vec::new()), _lock: None, daemon: None, nix };
        store.reload_caches()?;
        Ok(store)
    }

    /// Re-read `caches.json` and `catalog-caches.json` (the daemon does
    /// this per request).
    pub fn reload_caches(&self) -> Result<()> {
        // A mirror of cache.nixos.org (a company's proxy, a server's own cache for the computers
        // on it), named by its URL in `nixos-mirror`: fetched from in its place, its paths still
        // checked against cache.nixos.org's key, so a mirror can pass them on but never forge one.
        let nixos = fs::read_to_string(self.var_dir().join("nixos-mirror")).ok().map(|s| s.trim().trim_end_matches('/').to_string()).filter(|s| ["http://", "https://", "file://"].iter().any(|p| s.starts_with(p)));
        let mut caches = vec![Cache { url: nixos.unwrap_or_else(|| NIXOS_CACHE.into()), key: NIXOS_CACHE_KEY.into() }];
        for file in ["caches.json", CATALOG_CACHES] {
            if let Ok(data) = fs::read(self.var_dir().join(file)) {
                for c in serde_json::from_slice::<Vec<Cache>>(&data).with_context(|| format!("reading {file}"))? {
                    if !caches.contains(&c) {
                        caches.push(c);
                    }
                }
            }
        }
        *self.caches.write().unwrap() = caches;
        Ok(())
    }

    /// The caches every download may come from, cache.nixos.org first.
    pub fn caches(&self) -> Vec<Cache> {
        self.caches.read().unwrap().clone()
    }

    /// Done changing the store: other nami commands may go on (`nami try-os`,
    /// while its VM runs on the store).
    pub fn unlock(&mut self) {
        self._lock = None;
    }

    /// The same while the store's still in use here: what's done next only reads it, and
    /// waits on nami's that change it (the daily update's run as each person, whose changes
    /// the daemon makes under the lock).
    pub fn release(&self) {
        if let Some(l) = &self._lock {
            let _ = l.unlock();
        }
    }

    /// The exclusive state lock, released when the file is dropped.
    pub fn lock(&self) -> Result<File> {
        let lock = File::create(self.var_dir().join("lock"))?;
        if lock.try_lock().is_err() {
            // (Said, not printed: in the daemon it goes to whoever waits, not to its log.)
            crate::say!("waiting for another nami to finish (one that's installing, building or trying a computer)...");
            lock.lock().context("locking nami state")?;
        }
        Ok(lock)
    }

    pub fn daemon(&self) -> Option<&Path> {
        self.daemon.as_deref()
    }

    /// The installed Nix whose store this is, if any.
    pub fn nix(&self) -> Option<&crate::nix::store::Nix> {
        self.nix.as_ref()
    }

    /// Where finished trees wait before entering the store: inside it when
    /// the store is ours (so entering is a rename), elsewhere under Nix.
    pub fn staging_dir(&self) -> Result<PathBuf> {
        let dir = match self.nix {
            Some(_) => self.var_dir().join("staging"),
            None => self.phys(STORE_DIR),
        };
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Put a finished tree (`tmp`) into the store at `logical`, with these
    /// references (basenames), and record it as built here.
    pub fn commit_path(&self, tmp: &Path, logical: &str, references: Vec<String>) -> Result<()> {
        let Some(nix) = &self.nix else {
            let dest = self.phys(logical);
            if dest.symlink_metadata().is_ok() {
                remove_tree(&dest)?; // left over from an interrupted run, never registered
            }
            put_in_place(tmp, &dest).with_context(|| format!("moving {} to {}", tmp.display(), dest.display()))?;
            return self.register(logical, references).with_context(|| format!("registering {logical}"));
        };
        let (nar_hash, nar_size) = nar::hash_path(tmp)?;
        if !self.phys(logical).exists() {
            nix.import(&[(logical.to_string(), tmp.to_path_buf(), references.clone())])
                .with_context(|| format!("adding {logical} to Nix's store"))?;
        }
        remove_tree(tmp)?;
        nix.add_root(base_name(logical))?;
        let entry = PathInfo { references, nar_hash, nar_size, built: true };
        write_atomic(&self.db_entry(base_name(logical)), &serde_json::to_vec(&entry)?)
    }

    /// Where this user may keep downloads and cached build definitions.
    pub fn cache_dir(&self) -> PathBuf {
        if self.daemon.is_none() {
            return self.var_dir();
        }
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        base.join("nami")
    }

    /// Users may only use caches root trusts: the daemon downloads for them,
    /// and accepting a key is a decision for the whole machine.
    pub fn check_user_caches(&self, wanted: &[Cache]) -> Result<()> {
        let trusted = self.caches();
        for c in wanted {
            if !trusted.contains(c) {
                bail!("cache {} isn't trusted on this machine; root can add it with `caches:` in the system file", c.url);
            }
        }
        Ok(())
    }

    /// Have the system profile keep GPU drivers installed (for a user's apps).
    pub fn ensure_graphics(&self) -> Result<()> {
        match &self.daemon {
            Some(socket) => crate::daemon::client::ensure_graphics(socket),
            None => Ok(()), // root: the system profile handles its own runtime
        }
    }

    /// Replace the user's extra caches (cache.nixos.org always stays first).
    pub fn set_caches(&self, extra: &[Cache]) -> Result<()> {
        ensure!(self.daemon.is_none(), "only root can change the machine's caches");
        for c in extra {
            ensure!(c.key.split_once(':').is_some_and(|(n, k)| !n.is_empty() && k.len() == 44), "cache {}: key must look like name:base64", c.url);
        }
        write_atomic(&self.var_dir().join("caches.json"), &serde_json::to_vec_pretty(extra)?)?;
        self.reload_caches()
    }

    fn trusted_keys(&self) -> Vec<String> {
        self.caches.read().unwrap().iter().map(|c| c.key.clone()).collect()
    }

    /// GET `rel` from a cache: `Ok(None)` if it isn't there.
    fn cache_get(&self, cache_url: &str, rel: &str) -> Result<Option<Box<dyn Read + Send + Sync>>> {
        if cache_url.starts_with("http://") || cache_url.starts_with("https://") {
            let url = format!("{}/{rel}", cache_url.trim_end_matches('/'));
            // A dropped connection or a busy server: try again, a few times.
            let mut attempt = 0;
            loop {
                match self.http.get(&url).call() {
                    Ok(resp) => return Ok(Some(resp.into_reader())),
                    Err(ureq::Error::Status(404 | 403, _)) => return Ok(None),
                    // Out of room where it's going (a cloud computer's disk, full): what the
                    // server says, once; trying again won't make room.
                    Err(ureq::Error::Status(507, resp)) => {
                        let mut why = String::new();
                        let _ = resp.into_reader().take(500).read_to_string(&mut why);
                        let why = why.trim();
                        bail!("fetching {url}: {}", if why.is_empty() { "the server has no room for it (507)" } else { why });
                    }
                    Err(e @ (ureq::Error::Transport(_) | ureq::Error::Status(429 | 500..=599, _))) if attempt < 3 => {
                        attempt += 1;
                        crate::say!("{}", retry_said(&url, &e, 1 << attempt));
                        std::thread::sleep(Duration::from_secs(1 << attempt));
                    }
                    Err(e) => return Err(e).with_context(|| format!("fetching {url}")),
                }
            }
        } else {
            let dir = cache_url.strip_prefix("file://").unwrap_or(cache_url);
            match File::open(Path::new(dir).join(rel)) {
                Ok(f) => Ok(Some(Box::new(f))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).with_context(|| format!("reading {dir}/{rel}")),
            }
        }
    }

    pub fn path_info(&self, base: &str) -> Result<PathInfo> {
        Ok(serde_json::from_slice(&fs::read(self.db_entry(base))?)?)
    }

    /// Can every one of these paths be downloaded from a user cache?
    /// (cache.nixos.org never has paths nami builds, so it isn't asked.)
    pub fn substitutable(&self, paths: &[String]) -> bool {
        let caches: Vec<Cache> = self.caches.read().unwrap()[1..].to_vec();
        !caches.is_empty()
            && paths.iter().all(|p| {
                let hash = base_name(p).split('-').next().unwrap_or_default();
                caches.iter().any(|c| matches!(self.cache_get(&c.url, &format!("{hash}.narinfo")), Ok(Some(_))))
            })
    }

    pub fn var_dir(&self) -> PathBuf {
        crate::migrate::var_dir(&self.root)
    }

    /// Where a logical path like `/nix/store/abc-foo` lives on disk.
    pub fn phys(&self, logical: &str) -> PathBuf {
        self.root.join(logical.trim_start_matches('/'))
    }

    fn db_entry(&self, base: &str) -> PathBuf {
        self.var_dir().join("db").join(format!("{base}.json"))
    }

    pub fn is_valid(&self, base: &str) -> bool {
        // Under Nix, also check it's there: Nix's GC may have taken a path
        // nami no longer roots.
        self.db_entry(base).exists() && (self.nix.is_none() || self.phys(&format!("{STORE_DIR}/{base}")).exists())
    }

    fn references(&self, base: &str) -> Result<Vec<String>> {
        Ok(self.path_info(base)?.references)
    }

    pub fn http(&self) -> &ureq::Agent {
        &self.http
    }

    /// Make the given store paths and their full dependency closures present.
    pub fn realise(&self, paths: &[String]) -> Result<()> {
        if let Some(socket) = &self.daemon {
            if paths.iter().all(|p| self.is_valid(base_name(p))) {
                return Ok(());
            }
            return crate::daemon::client::realise(socket, paths);
        }
        if let Some(nix) = &self.nix {
            return self.realise_with_nix(nix, paths);
        }
        // 1. Walk the closure via narinfo References, stopping at valid paths
        //    (a valid path's closure is always valid).
        let mut seen: HashSet<String> = HashSet::new();
        let mut frontier: Vec<String> = paths.iter().map(|p| base_name(p).to_string()).collect();
        let mut missing: BTreeMap<String, NarInfo> = BTreeMap::new();
        while !frontier.is_empty() {
            frontier.retain(|b| !self.is_valid(b) && seen.insert(b.clone()));
            let infos = par_map(&frontier, |b| self.fetch_narinfo(b))?;
            frontier = infos.iter().flat_map(|i| i.references.clone()).collect();
            missing.extend(infos.into_iter().map(|i| (i.base_name().to_string(), i)));
        }
        if missing.is_empty() {
            return Ok(());
        }

        // 2. Download, verify and unpack everything missing.
        let total: u64 = missing.values().map(|i| i.nar_size).sum();
        crate::say!("fetching {} paths ({:.1} MiB unpacked)", missing.len(), total as f64 / 1048576.0);
        let infos: Vec<&NarInfo> = missing.values().collect();
        par_map(&infos, |info| self.fetch_nar(info))?;

        // 3. Register, so they count as valid only once the whole closure is there.
        for info in missing.values() {
            let entry = PathInfo {
                references: info.references.clone(),
                nar_hash: info.nar_hash.clone(),
                nar_size: info.nar_size,
                built: false,
            };
            write_atomic(&self.db_entry(info.base_name()), &serde_json::to_vec(&entry)?)?;
        }
        Ok(())
    }

    /// Nix downloads (checking signatures itself); nami records what came.
    fn realise_with_nix(&self, nix: &crate::nix::store::Nix, paths: &[String]) -> Result<()> {
        let missing: Vec<String> = paths.iter().filter(|p| !self.is_valid(base_name(p))).cloned().collect();
        if missing.is_empty() {
            return Ok(());
        }
        let caches: Vec<(String, String)> = self.caches.read().unwrap()[1..].iter().map(|c| (c.url.clone(), c.key.clone())).collect();
        let have = nix.valid(&missing)?;
        let fetch: Vec<String> = missing.iter().filter(|p| !have.contains(*p)).cloned().collect();
        if !fetch.is_empty() {
            crate::say!("fetching {} paths through Nix", fetch.len());
            nix.realise(&fetch, &caches)?;
        }
        for p in &missing {
            nix.add_root(base_name(p))?;
        }
        let closure: Vec<String> = nix.requisites(&missing)?.into_iter().filter(|p| !self.db_entry(base_name(p)).exists()).collect();
        for i in nix.info(&closure)? {
            let entry = PathInfo { references: i.references, nar_hash: i.nar_hash, nar_size: i.nar_size, built: false };
            write_atomic(&self.db_entry(base_name(&i.path)), &serde_json::to_vec(&entry)?)?;
        }
        Ok(())
    }

    /// Can `base` be downloaded (a trusted cache has it, signed)? An error when that can't be
    /// told: a cache that can't be reached, or one whose answer is badly signed (never "no
    /// cache has it", which would leave the package out quietly).
    pub fn in_a_cache(&self, base: &str) -> Result<bool> {
        Ok(self.find_narinfo(base)?.is_some())
    }

    fn fetch_narinfo(&self, base: &str) -> Result<NarInfo> {
        self.find_narinfo(base)?.ok_or_else(|| {
            if crate::index::MAC {
                anyhow::anyhow!("{base} isn't ready-built for Mac (no binary cache has it for {}), and nami doesn't build packages on a Mac yet", crate::index::SYSTEM)
            } else {
                anyhow::anyhow!("{base} is not in any binary cache (unfree, or not built by Hydra?)")
            }
        })
    }

    /// `base`'s narinfo from the first cache that has it, its signature checked; None when
    /// none has it.
    fn find_narinfo(&self, base: &str) -> Result<Option<NarInfo>> {
        let hash = base.split('-').next().unwrap_or(base);
        let caches = self.caches();
        let keys = self.trusted_keys();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        for cache in &caches {
            let Some(mut body) = self.cache_get(&cache.url, &format!("{hash}.narinfo"))? else { continue };
            let mut text = String::new();
            body.read_to_string(&mut text)?;
            let mut info = NarInfo::parse(&text)?;
            if info.base_name() != base {
                bail!("{} returned narinfo for {} instead of {base}", cache.url, info.store_path);
            }
            info.verify(&keys).with_context(|| format!("{base} from {}: its signature doesn't check out, so it isn't used", cache.url))?;
            info.cache = cache.url.clone();
            return Ok(Some(info));
        }
        Ok(None)
    }

    fn fetch_nar(&self, info: &NarInfo) -> Result<()> {
        let dest = self.phys(&info.store_path);
        if dest.symlink_metadata().is_ok() {
            // Left over from an interrupted run: never registered, so redo it.
            remove_tree(&dest)?;
        }
        let tmp = self.root.join("nix/store").join(format!(".nami-tmp-{}", info.base_name()));
        self.unpack_nar(info, &tmp)?;
        put_in_place(&tmp, &dest).with_context(|| format!("moving {} to {}", tmp.display(), dest.display()))?;
        crate::say!("  {}", info.base_name());
        Ok(())
    }

    /// One store path's files, without what it depends on, in `dest`
    /// (not the store): to look at a path that isn't there.
    pub fn peek(&self, path: &str, dest: &Path) -> Result<()> {
        let info = self.fetch_narinfo(base_name(path))?;
        self.unpack_nar(&info, dest)
    }

    /// A path's NAR from its cache, checked, unpacked at `tmp`.
    fn unpack_nar(&self, info: &NarInfo, tmp: &Path) -> Result<()> {
        if tmp.symlink_metadata().is_ok() {
            remove_tree(tmp)?;
        }
        let body = self
            .cache_get(&info.cache, &info.url)?
            .with_context(|| format!("{}/{} is missing", info.cache, info.url))?;
        let decompressed: Box<dyn Read> = match info.compression.as_str() {
            "zstd" => Box::new(zstd::stream::read::Decoder::new(body)?),
            "xz" => Box::new(xz2::read::XzDecoder::new(body)),
            "none" => Box::new(body),
            other => bail!("unsupported NAR compression {other:?}"),
        };
        let mut hashing = nar::HashingReader::new(decompressed);
        nar::unpack(&mut hashing, tmp).with_context(|| format!("unpacking {}", info.store_path))?;
        std::io::copy(&mut hashing, &mut std::io::sink())?;

        let (size, hash) = (hashing.count, hashing.finish());
        if size != info.nar_size || hash != info.nar_hash {
            remove_tree(tmp)?;
            bail!("{}: NAR hash mismatch (got {hash}, {size} bytes)", info.store_path);
        }
        Ok(())
    }

    /// Record a path nami put into the store itself (a build output or source).
    pub fn register(&self, logical: &str, references: Vec<String>) -> Result<()> {
        let (nar_hash, nar_size) = nar::hash_path(&self.phys(logical))?;
        let entry = PathInfo { references, nar_hash, nar_size, built: true };
        write_atomic(&self.db_entry(base_name(logical)), &serde_json::to_vec(&entry)?)
    }

    /// Copy a local file or directory into the store, content-addressed:
    /// the same content and name always give the same store path.
    pub fn add_path(&self, src: &Path, name: &str) -> Result<String> {
        self.add_path_referring(src, name, &[])
    }

    /// [`Store::add_path`] for a tree that points into other store paths (a
    /// Python with its packages, python_env.rs): those of `candidates`
    /// (basenames) it mentions are its references, so gc keeps them while
    /// it's used.
    pub fn add_path_referring(&self, src: &Path, name: &str, candidates: &[String]) -> Result<String> {
        let dir = match &self.daemon {
            Some(_) => self.cache_dir(),
            None => self.staging_dir()?,
        };
        fs::create_dir_all(&dir).with_context(|| format!("making {}", dir.display()))?;
        let tmp = dir.join(format!(".nami-add-{}.nar", std::process::id()));
        let mut w = nar::HashingWriter::new(std::io::BufWriter::new(File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?));
        nar::dump(src, &mut w).with_context(|| format!("archiving {}", src.display()))?;
        std::io::Write::flush(&mut w)?;
        let nar_hash = w.finish();
        let result = match &self.daemon {
            Some(socket) => crate::daemon::client::add_nar(socket, &tmp, name, candidates),
            None => self.add_nar_file(&tmp, &nar_hash, name, candidates),
        };
        fs::remove_file(&tmp)?;
        result
    }

    /// Add an archive (already hashed: `nar_hash`) under a content-addressed
    /// path. Its references are those of `candidates` (basenames of valid
    /// paths) it mentions, read from what it unpacks to, and they're part of
    /// its path: the same tree with other references is another path, never
    /// one with references missing (the daemon takes them from anyone).
    pub fn add_nar_file(&self, nar_file: &Path, nar_hash: &str, name: &str, candidates: &[String]) -> Result<String> {
        ensure!(valid_store_name(name), "bad store path name {name:?}");
        let candidates: Vec<String> = candidates.iter().filter(|c| c.len() > 33 && valid_store_name(c) && self.is_valid(c)).cloned().collect();
        let plain = store_path(&format!("tray-source:{nar_hash}:{name}"), name);
        if candidates.is_empty() && self.is_valid(base_name(&plain)) {
            return Ok(plain);
        }
        let tmp = self.staging_dir()?.join(format!(".nami-add-{}-{name}", std::process::id()));
        if tmp.symlink_metadata().is_ok() {
            remove_tree(&tmp)?;
        }
        nar::unpack(&mut std::io::BufReader::new(File::open(nar_file)?), &tmp).with_context(|| format!("unpacking into {}", tmp.display()))?;
        let refs = if candidates.is_empty() { Vec::new() } else { crate::build::scan_references(&tmp, &candidates).with_context(|| format!("looking for references in {}", tmp.display()))? };
        let logical = if refs.is_empty() { plain } else { store_path(&format!("nami-tree:{nar_hash}:{}:{name}", refs.join(":")), name) };
        if self.is_valid(base_name(&logical)) {
            remove_tree(&tmp)?;
        } else {
            self.commit_path(&tmp, &logical, refs)?;
        }
        Ok(logical)
    }

    /// Basenames of every registered path reachable from `roots`.
    pub fn closure(&self, roots: &[String]) -> Result<BTreeSet<String>> {
        let mut live: BTreeSet<String> = BTreeSet::new();
        let mut stack: Vec<String> = roots.iter().map(|p| base_name(p).to_string()).collect();
        while let Some(b) = stack.pop() {
            if self.is_valid(&b) && live.insert(b.clone()) {
                stack.extend(self.references(&b)?);
            }
        }
        Ok(live)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Delete nami-registered paths not reachable from `roots`. Returns how
    /// many, and how many bytes that freed.
    pub fn gc(&self, roots: &[String]) -> Result<(usize, u64)> {
        self.gc_with(roots, &Sweep { progress: true, ..Default::default() })
    }

    /// Paths registered within the last `secs` seconds.
    fn registered_within(&self, secs: u64) -> Vec<String> {
        // (As the cleanup's clock has it: NAMI_NOW, in tests.)
        let since = std::time::UNIX_EPOCH + Duration::from_secs(crate::cleanup::now().saturating_sub(secs));
        fs::read_dir(self.var_dir().join("db"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t >= since))
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").map(|b| format!("{STORE_DIR}/{b}")))
            .collect()
    }

    /// `gc`, as `sweep` says.
    pub fn gc_with(&self, roots: &[String], sweep: &Sweep) -> Result<(usize, u64)> {
        let mut roots = roots.to_vec();
        if let Some(secs) = sweep.spare_newer_than {
            roots.extend(self.registered_within(secs));
        }
        let live = self.closure(&roots)?;
        let (deleted, freed) = match &self.nix {
            Some(nix) => self.gc_with_nix(nix, &live)?,
            None => self.gc_paths(&live, sweep)?,
        };
        // A Mac's casks' apps, kept beside the store (casks.rs): those no store path left links to.
        let (apps, size) = crate::casks::sweep(self);
        if sweep.progress && apps > 0 {
            crate::say!("removed {apps} {} from Homebrew's casks ({:.1} MiB)", if apps == 1 { "app" } else { "apps" }, size as f64 / 1048576.0);
        }
        Ok((deleted + apps, freed + size))
    }

    /// `gc_with`'s store paths, nami's own store's: what isn't `live` goes.
    fn gc_paths(&self, live: &BTreeSet<String>, sweep: &Sweep) -> Result<(usize, u64)> {
        let dead: Vec<String> = fs::read_dir(self.var_dir().join("db"))?
            .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".json").map(String::from))
            .filter(|b| !live.contains(b))
            .collect();
        if sweep.progress && !dead.is_empty() {
            let size: u64 = dead.iter().filter_map(|b| self.path_info(b).ok()).map(|i| i.nar_size).sum();
            crate::say!("removing {} paths ({:.1} MiB)", dead.len(), size as f64 / 1048576.0);
        }
        let (mut deleted, mut freed) = (0, 0);
        for base in &dead {
            let base = base.as_str();
            if sweep.progress {
                crate::say!("  {base}");
            }
            freed += self.path_info(base).map(|i| i.nar_size).unwrap_or(0);
            // Unregister first: a half-deleted path must never look valid.
            fs::remove_file(self.db_entry(base))?;
            let path = self.phys(&format!("{STORE_DIR}/{base}"));
            if path.symlink_metadata().is_ok() {
                remove_tree(&path)?;
            }
            deleted += 1;
        }
        Ok((deleted, freed))
    }

    /// Under Nix: root exactly what's live, then ask Nix to delete the rest
    /// of nami's paths (it keeps whatever Nix itself, or anything else, uses).
    fn gc_with_nix(&self, nix: &crate::nix::store::Nix, live: &BTreeSet<String>) -> Result<(usize, u64)> {
        nix.sync_roots(live)?;
        let mut dead = Vec::new();
        for entry in fs::read_dir(self.var_dir().join("db"))? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if let Some(base) = name.strip_suffix(".json") {
                if !live.contains(base) {
                    dead.push(base.to_string());
                }
            }
        }
        let paths: Vec<String> = dead.iter().map(|b| format!("{STORE_DIR}/{b}")).collect();
        nix.delete(&paths)?;
        let (mut deleted, mut freed) = (0, 0);
        for b in &dead {
            if !self.phys(&format!("{STORE_DIR}/{b}")).exists() {
                freed += self.path_info(b).map(|i| i.nar_size).unwrap_or(0);
                deleted += 1;
            }
            // Forget it either way: if Nix kept it, it's Nix's now.
            fs::remove_file(self.db_entry(b))?;
        }
        Ok((deleted, freed))
    }
}

/// `/nix/store/<hash>-<name>` where the hash part is derived from `fingerprint`
/// the way Nix does it: sha256, XOR-folded to 160 bits, nix-base32.
pub fn store_path(fingerprint: &str, name: &str) -> String {
    use sha2::{Digest, Sha256};
    let full = Sha256::digest(fingerprint.as_bytes());
    let mut short = [0u8; 20];
    for (i, b) in full.iter().enumerate() {
        short[i % 20] ^= b;
    }
    format!("{STORE_DIR}/{}-{name}", crate::nix::base32::encode(&short))
}

/// Characters Nix allows in store path names.
pub fn valid_store_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 211 && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "+-._?=".contains(c))
}

pub fn base_name(path: &str) -> &str {
    path.trim_start_matches(STORE_DIR).trim_start_matches('/')
}

/// Store paths are read-only; restore write permission so they can be removed.
pub fn make_writable(path: &Path) -> Result<()> {
    let meta = path.symlink_metadata()?;
    if meta.is_dir() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
        for entry in fs::read_dir(path)? {
            make_writable(&entry?.path())?;
        }
    }
    Ok(())
}

pub fn remove_tree(path: &Path) -> Result<()> {
    if path.symlink_metadata()?.is_dir() {
        make_writable(path)?;
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// A finished tree, `tmp`, renamed to `dest` beside it. macOS renames a read-only folder only
/// for root, even within one parent: anyone else's (a store of their own, a test's) is
/// writable for the moment, and read-only again once it's in place.
fn put_in_place(tmp: &Path, dest: &Path) -> std::io::Result<()> {
    match fs::rename(tmp, dest) {
        Err(e) if cfg!(target_os = "macos") && e.kind() == std::io::ErrorKind::PermissionDenied && tmp.symlink_metadata().is_ok_and(|m| m.is_dir()) => {
            let mode = tmp.symlink_metadata()?.permissions();
            fs::set_permissions(tmp, fs::Permissions::from_mode(mode.mode() | 0o200))?;
            if let Err(e) = fs::rename(tmp, dest) {
                let _ = fs::set_permissions(tmp, mode);
                return Err(e);
            }
            fs::set_permissions(dest, mode)
        }
        r => r,
    }
}

pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {

    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    File::open(&tmp)?.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Run `f` over `items` on a small thread pool, failing on the first error.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> Result<R> + Sync) -> Result<Vec<R>> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Result<R>>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..JOBS.min(items.len()) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= items.len() {
                    break;
                }
                let r = f(&items[i]);
                let failed = r.is_err();
                results.lock().unwrap()[i] = Some(r);
                if failed {
                    next.store(items.len(), Ordering::Relaxed);
                }
            });
        }
    });
    let mut out = Vec::with_capacity(items.len());
    for r in results.into_inner().unwrap().into_iter().flatten() {
        out.push(r?);
    }
    Ok(out)
}

/// A download tried again, in a few words: the server, why, and when
/// (`cache.nixos.org: no connection to it; trying again in 2 s`).
fn retry_said(url: &str, e: &ureq::Error, secs: u64) -> String {
    let host = url.split_once("://").map_or(url, |(_, r)| r.split('/').next().unwrap_or(r));
    let why = match e {
        ureq::Error::Status(429, _) => "it's busy".to_string(),
        ureq::Error::Status(code, _) => format!("it answered {code}"),
        ureq::Error::Transport(t) => match t.kind() {
            ureq::ErrorKind::Dns => "its name can't be looked up (is this computer online?)".to_string(),
            ureq::ErrorKind::ConnectionFailed => "no connection to it".to_string(),
            ureq::ErrorKind::Io => "the connection dropped".to_string(),
            k => k.to_string(),
        },
    };
    format!("{host}: {why}; trying again in {secs} s")
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_retry_says_the_server_and_why_in_words() {
        // Nothing listens on port 9 here: the connection is refused.
        let e = ureq::get("http://127.0.0.1:9/abc.narinfo").call().unwrap_err();
        assert_eq!(super::retry_said("http://127.0.0.1:9/abc.narinfo", &e, 2), "127.0.0.1:9: no connection to it; trying again in 2 s");
    }
}
