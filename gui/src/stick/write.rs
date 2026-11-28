//! `namios-installer write`: the part that runs with admin rights (pkexec, UAC,
//! macOS's password prompt). Downloads the image, unpacks it as it comes
//! (xz) and writes it straight onto the USB drive, so no image lands on the
//! disk; checks the download against the release's checksum, and reads
//! the USB drive back to compare. It says how it's going in `progress`, a file
//! the window made (`{"step":"write","done":..,"total":..}` and so on);
//! `progress`.stop there stops it.

use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

/// What's written at a time: whole sectors, as Windows and macOS want.
const CHUNK: usize = 4 << 20;

pub struct Job {
    pub url: String,
    pub sha256: String,
    pub disk: String,
    pub progress: std::path::PathBuf,
    /// Your files, for namiOS's boot partition (seed.rs), once the image is checked.
    pub seed: Option<crate::seed::Seed>,
}

/// Say how it's going: the file's whole content, one JSON object.
pub fn say(progress: &Path, v: serde_json::Value) {
    // The window made it, in the temp folder. Opened to create, root would be
    // refused someone else's file there (fs.protected_regular, most Linuxes),
    // and the window would never hear a word: so only if it's gone.
    let open = |create| std::fs::OpenOptions::new().write(true).truncate(true).create(create).open(progress);
    if let Ok(mut f) = open(false).or_else(|_| open(true)) {
        let _ = f.write_all(format!("{v}\n").as_bytes());
    }
}

pub fn stopped(progress: &Path) -> bool {
    progress.with_extension("stop").exists()
}

pub fn run(job: &Job) -> i32 {
    match write(job) {
        Ok(()) => {
            say(&job.progress, serde_json::json!({ "step": "done" }));
            0
        }
        Err(e) => {
            say(&job.progress, serde_json::json!({ "step": "error", "message": e }));
            1
        }
    }
}

/// The download as it comes: how much, and its SHA-256.
pub struct Counted<R> {
    pub inner: R,
    pub read: Arc<AtomicU64>,
    pub hash: Sha256,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hash.update(&buf[..n]);
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// The USB drive, written a chunk at a time (the last one filled out with
/// zeros to a whole chunk); what's written is hashed, to compare after.
/// The first chunk, with the partition table, is kept and written last:
/// Windows finds new partitions as soon as their table is there, mounts
/// one, and then won't let the rest be written.
struct Stick<'a> {
    dev: std::fs::File,
    buf: Vec<u8>,
    written: u64,
    /// The first chunk, until it's written (at the end).
    first: Option<Vec<u8>>,
    /// What's written after the first chunk.
    hash: Sha256,
    job: &'a Job,
    downloaded: Arc<AtomicU64>,
    total: u64,
}

impl Stick<'_> {
    fn flush_chunk(&mut self, pad: bool) -> std::io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        if pad {
            self.buf.resize(self.buf.len().div_ceil(4096) * 4096, 0);
        }
        if self.written == 0 {
            // The first: kept, and its place left for now.
            self.dev.seek(SeekFrom::Start(self.buf.len() as u64))?;
            self.first = Some(std::mem::take(&mut self.buf));
            self.buf.reserve(CHUNK);
            self.written = self.first.as_ref().unwrap().len() as u64;
        } else {
            self.dev.write_all(&self.buf)?;
            self.hash.update(&self.buf);
            self.written += self.buf.len() as u64;
            self.buf.clear();
        }
        if stopped(&self.job.progress) {
            return Err(std::io::Error::other("stopped"));
        }
        say(&self.job.progress, serde_json::json!({ "step": "write", "done": self.downloaded.load(Ordering::Relaxed), "total": self.total, "written": self.written }));
        Ok(())
    }
}

impl Write for Stick<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let room = CHUNK - self.buf.len();
        let n = data.len().min(room);
        self.buf.extend_from_slice(&data[..n]);
        if self.buf.len() == CHUNK {
            self.flush_chunk(false)?;
        }
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn write(job: &Job) -> Result<(), String> {
    say(&job.progress, serde_json::json!({ "step": "prepare" }));
    let raw = prepare(&job.disk)?;
    let result = (|| -> Result<(), String> {
        let response = ureq::get(&job.url).call().map_err(|e| format!("Couldn't download namiOS: {e}"))?;
        let total = response.header("content-length").and_then(|s| s.parse().ok()).unwrap_or(0);
        let downloaded = Arc::new(AtomicU64::new(0));
        let mut input = BufReader::with_capacity(1 << 20, Counted { inner: response.into_reader(), read: downloaded.clone(), hash: Sha256::new() });
        let dev = open(&raw, true).map_err(|e| format!("Couldn't open the USB drive ({raw}): {e}"))?;
        let mut stick = Stick { dev, buf: Vec::with_capacity(CHUNK), written: 0, first: None, hash: Sha256::new(), job, downloaded, total };
        lzma_rs::xz_decompress(&mut input, &mut stick).map_err(|e| if stopped(&job.progress) { "stopped".to_string() } else { format!("Couldn't write to the USB drive: {e}") })?;
        stick.flush_chunk(true).map_err(|e| format!("Couldn't write to the USB drive: {e}"))?;
        // The rest is there: now the partition table.
        stick.dev.sync_all().map_err(|e| format!("Couldn't write to the USB drive: {e}"))?;
        let first = stick.first.take().unwrap_or_default();
        stick.dev.seek(SeekFrom::Start(0)).and_then(|_| stick.dev.write_all(&first)).and_then(|_| stick.dev.sync_all()).map_err(|e| format!("Couldn't write to the USB drive: {e}"))?;
        // The rest of the download (xz's index), for its checksum.
        std::io::copy(&mut input, &mut std::io::sink()).map_err(|e| format!("Couldn't download namiOS: {e}"))?;
        let got = crate::release::hex(&input.get_ref().hash.clone().finalize());
        if !got.eq_ignore_ascii_case(job.sha256.trim()) {
            return Err("The download is damaged (its checksum doesn't match). Try again.".into());
        }
        let (written, want) = (stick.written, stick.hash.finalize());
        drop(stick.dev);
        check(&raw, &first, written, &want, job)?;
        match &job.seed {
            Some(seed) => add_seed(&raw, &first, seed, job),
            None => Ok(()),
        }
    })();
    finish(&job.disk);
    result
}

/// Read the USB drive back: what's on it is what was written.
fn check(raw: &str, first: &[u8], written: u64, want: &[u8], job: &Job) -> Result<(), String> {
    let mut dev = open(raw, false).map_err(|e| format!("Couldn't verify the USB drive: {e}"))?;
    forget_cache(&dev);
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let bad = || "Verification failed. The USB drive may be faulty. Try a different USB drive.".to_string();
    dev.read_exact(&mut buf[..first.len()]).map_err(|e| format!("Couldn't verify the USB drive: {e}"))?;
    if buf[..first.len()] != *first {
        return Err(bad());
    }
    let mut done = first.len() as u64;
    while done < written {
        let n = ((written - done) as usize).min(CHUNK);
        dev.read_exact(&mut buf[..n]).map_err(|e| format!("Couldn't verify the USB drive: {e}"))?;
        hash.update(&buf[..n]);
        done += n as u64;
        if stopped(&job.progress) {
            return Err("stopped".into());
        }
        say(&job.progress, serde_json::json!({ "step": "check", "done": done, "total": written }));
    }
    if hash.finalize()[..] != *want {
        return Err(bad());
    }
    Ok(())
}

/// Your files onto the USB drive's EFI partition (where its partition table says it is),
/// read back from the drive itself.
fn add_seed(raw: &str, first: &[u8], seed: &crate::seed::Seed, job: &Job) -> Result<(), String> {
    say(&job.progress, serde_json::json!({ "step": "seed" }));
    let (esp, _) = crate::gpt::image_parts(first)?;
    let dev = open(raw, true).map_err(|e| format!("Couldn't add your files to the USB drive: {e}"))?;
    let dev = crate::seed::put(dev, esp.start, esp.size, seed)?;
    dev.sync_all().map_err(|e| format!("Couldn't add your files to the USB drive: {e}"))?;
    drop(dev);
    let mut dev = open(raw, false).map_err(|e| format!("Couldn't verify the USB drive: {e}"))?;
    forget_cache(&dev);
    let back = crate::seed::read(ReadOnly(&mut dev), esp.start, esp.size)?;
    if back != *seed {
        return Err("Verification failed. Your files didn't read back as they were written. Try a different USB drive.".into());
    }
    Ok(())
}

/// A disk opened to read, for what wants to be able to write (and won't, reading).
struct ReadOnly<'a>(&'a mut std::fs::File);

impl Read for ReadOnly<'_> {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(b)
    }
}
impl Write for ReadOnly<'_> {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("read-only"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Seek for ReadOnly<'_> {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(p)
    }
}

fn open(path: &str, write: bool) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().read(true).write(write).open(path)
}

/// Linux and macOS: read the USB drive, not what's left of it in memory.
#[cfg(unix)]
fn forget_cache(f: &std::fs::File) {
    use std::os::fd::AsRawFd;
    #[cfg(target_os = "linux")]
    unsafe {
        libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
    }
    #[cfg(target_os = "macos")]
    unsafe {
        libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1);
    }
}
#[cfg(not(unix))]
fn forget_cache(_: &std::fs::File) {}

/// The USB drive, ready to be written: nothing on it in use. What to open.
#[cfg(target_os = "linux")]
fn prepare(disk: &str) -> Result<String, String> {
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    for l in mounts.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() > 1 && f[0].starts_with(disk) {
            let at = f[1].replace("\\040", " ");
            let ok = std::process::Command::new("umount").arg(&at).status().is_ok_and(|s| s.success());
            if !ok {
                return Err(format!("The USB drive is in use ({at}). Close any apps using it, then try again."));
            }
        }
    }
    Ok(disk.to_string())
}

#[cfg(target_os = "windows")]
fn prepare(disk: &str) -> Result<String, String> {
    // Windows lets a disk be written where nothing on it is mounted: an
    // offline disk; or, as removable ones can't go offline, one without
    // partitions (it's being erased anyway).
    if let Some(n) = disk.strip_prefix(r"\\.\PhysicalDrive") {
        powershell(&format!(
            "$ErrorActionPreference = 'Stop'; $d = {n}; \
             try {{ Set-Disk -Number $d -IsReadOnly $false }} catch {{ }}; \
             try {{ Set-Disk -Number $d -IsOffline $true }} catch {{ \
               try {{ Clear-Disk -Number $d -RemoveData -RemoveOEM -Confirm:$false }} catch {{ }}; \
               if (@(Get-Partition -DiskNumber $d -ErrorAction SilentlyContinue | Get-Volume -ErrorAction SilentlyContinue).Count -gt 0) {{ throw 'something on it is still in use' }} \
             }}"
        ))
        .map_err(|e| format!("Windows couldn't prepare the USB drive: {e}"))?;
    }
    Ok(disk.to_string())
}

#[cfg(target_os = "macos")]
fn prepare(disk: &str) -> Result<String, String> {
    if disk.starts_with("/dev/disk") {
        let ok = std::process::Command::new("diskutil").args(["unmountDisk", "force", disk]).status().is_ok_and(|s| s.success());
        if !ok {
            return Err("The USB drive is in use. Close any apps using it, then try again.".into());
        }
        // The raw device: much faster.
        return Ok(disk.replacen("/dev/disk", "/dev/rdisk", 1));
    }
    Ok(disk.to_string())
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn prepare(disk: &str) -> Result<String, String> {
    Ok(disk.to_string())
}

/// Back in use: Windows reads the USB drive again; macOS lets it go.
fn finish(disk: &str) {
    #[cfg(target_os = "windows")]
    if let Some(n) = disk.strip_prefix(r"\\.\PhysicalDrive") {
        let _ = powershell(&format!("Update-Disk -Number {n}; Set-Disk -Number {n} -IsOffline $false"));
    }
    #[cfg(target_os = "macos")]
    if disk.starts_with("/dev/disk") {
        let _ = std::process::Command::new("diskutil").args(["eject", disk]).status();
    }
    let _ = disk;
}

#[cfg(target_os = "windows")]
fn powershell(script: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", script]).creation_flags(0x0800_0000).output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}
