//! Your files, handed to namiOS: the folder `namios` on the drive, as anyone puts theirs there
//! by hand, with `system.nami`, `user.nami` (the first person's) or
//! `source` (one line: the git address they're at). The Installer writes it onto namiOS's boot
//! partition (the USB drive's EFI partition, or the boot partition next to Windows); namiOS
//! takes it from there at its first start. Written after the image is written and checked,
//! then read back. namiOS as it comes needs none: then nothing's written. (Installers before
//! the folder wrote `nami-seed/`, which namiOS still takes.)
//!
//! The window hands the writer the same files in a folder of its own
//! (`--seed <folder>`), as they'll be on the partition.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The folder on the boot partition.
pub const DIR: &str = "namios";
const FILES: [&str; 3] = ["system.nami", "user.nami", "source"];
/// system.nami's name until 0.19: read too, and replaced when the files are written.
const OLD_SYSTEM_FILE: &str = "computer.nami";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seed {
    pub system: Option<String>,
    pub user: Option<String>,
    /// A git address (`github:you/config/laptop`).
    pub source: Option<String>,
}

impl Seed {
    pub fn is_empty(&self) -> bool {
        self.system.is_none() && self.user.is_none() && self.source.is_none()
    }

    fn files(&self) -> Vec<(&'static str, String)> {
        let source = self.source.as_ref().map(|s| format!("{}\n", s.trim()));
        FILES.into_iter().zip([self.system.clone(), self.user.clone(), source]).filter_map(|(n, t)| Some((n, t?))).collect()
    }

    fn from_files(mut get: impl FnMut(&str) -> Option<String>) -> Seed {
        // (The system's file by its name before 0.19 too.)
        let system = get(FILES[0]).or_else(|| get(OLD_SYSTEM_FILE));
        Seed { system, user: get(FILES[1]), source: get(FILES[2]).map(|s| s.trim().to_string()) }
    }

    /// Into `dir`, for the writer.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        for n in FILES.iter().chain([&OLD_SYSTEM_FILE]) {
            let _ = std::fs::remove_file(dir.join(n));
        }
        for (n, t) in self.files() {
            std::fs::write(dir.join(n), t)?;
        }
        Ok(())
    }

    pub fn load(dir: &Path) -> Result<Seed, String> {
        let s = Seed::from_files(|n| std::fs::read(dir.join(n)).ok().map(|b| String::from_utf8_lossy(&b).into_owned()));
        if s.is_empty() {
            return Err(format!("No files to add in {}.", dir.display()));
        }
        Ok(s)
    }
}

/// What's written at a time on the disk: whole blocks from block boundaries, as Windows and
/// macOS's raw disks take them.
const BLOCK: u64 = 64 << 10;

/// A partition, `size` bytes at `base` of `inner`, to read and write a file system on: a
/// block at a time, read whole, changed, and written back whole (`flush`).
pub struct Part<D: Read + Write + Seek> {
    inner: D,
    base: u64,
    size: u64,
    pos: u64,
    /// The block in hand, and whether it's changed.
    block: Option<(u64, Vec<u8>, bool)>,
}

impl<D: Read + Write + Seek> Part<D> {
    pub fn new(inner: D, base: u64, size: u64) -> Self {
        Part { inner, base, size, pos: 0, block: None }
    }

    fn write_back(&mut self) -> std::io::Result<()> {
        if let Some((b, data, dirty)) = &mut self.block {
            if *dirty {
                self.inner.seek(SeekFrom::Start(self.base + *b * BLOCK))?;
                self.inner.write_all(data)?;
                *dirty = false;
            }
        }
        Ok(())
    }

    fn load(&mut self, b: u64) -> std::io::Result<()> {
        if self.block.as_ref().is_some_and(|(at, _, _)| *at == b) {
            return Ok(());
        }
        self.write_back()?;
        let mut data = vec![0u8; BLOCK.min(self.size - b * BLOCK) as usize];
        self.inner.seek(SeekFrom::Start(self.base + b * BLOCK))?;
        self.inner.read_exact(&mut data)?;
        self.block = Some((b, data, false));
        Ok(())
    }

    /// What's changed, onto the disk; the disk itself.
    pub fn into_inner(mut self) -> std::io::Result<D> {
        self.write_back()?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<D: Read + Write + Seek> Read for Part<D> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let b = self.pos / BLOCK;
        self.load(b)?;
        let (_, data, _) = self.block.as_ref().unwrap();
        let off = (self.pos - b * BLOCK) as usize;
        let n = buf.len().min(data.len() - off);
        buf[..n].copy_from_slice(&data[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl<D: Read + Write + Seek> Write for Part<D> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.pos >= self.size {
            return Err(std::io::Error::other("past the partition's end"));
        }
        let b = self.pos / BLOCK;
        self.load(b)?;
        let (_, data, dirty) = self.block.as_mut().unwrap();
        let off = (self.pos - b * BLOCK) as usize;
        let n = buf.len().min(data.len() - off);
        data[off..off + n].copy_from_slice(&buf[..n]);
        *dirty = true;
        self.pos += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.write_back()?;
        self.inner.flush()
    }
}

impl<D: Read + Write + Seek> Seek for Part<D> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
            SeekFrom::End(d) => self.size as i128 + d as i128,
        };
        if p < 0 {
            return Err(std::io::Error::other("seek before the start"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

/// The seed onto the FAT partition `size` bytes at `base` of `disk`: `namios/`, with only
/// what the seed has (what was there before goes). The disk, back.
pub fn put<D: Read + Write + Seek>(disk: D, base: u64, size: u64, seed: &Seed) -> Result<D, String> {
    let bad = |e: std::io::Error| format!("Couldn't add your files to namiOS's boot partition: {e}");
    let mut part = Part::new(disk, base, size);
    {
        let fs = fatfs::FileSystem::new(&mut part, fatfs::FsOptions::new()).map_err(bad)?;
        {
            let root = fs.root_dir();
            let dir = match root.open_dir(DIR) {
                Ok(d) => d,
                Err(_) => root.create_dir(DIR).map_err(bad)?,
            };
            for n in FILES.iter().chain([&OLD_SYSTEM_FILE]) {
                let _ = dir.remove(n);
            }
            for (n, t) in seed.files() {
                let mut f = dir.create_file(n).map_err(bad)?;
                f.truncate().map_err(bad)?;
                f.write_all(t.as_bytes()).map_err(bad)?;
                f.flush().map_err(bad)?;
            }
        }
        fs.unmount().map_err(bad)?;
    }
    part.into_inner().map_err(bad)
}

/// What `namios/` on the FAT partition has.
pub fn read<D: Read + Write + Seek>(disk: D, base: u64, size: u64) -> Result<Seed, String> {
    let bad = |e: std::io::Error| format!("Couldn't read your files back from namiOS's boot partition: {e}");
    let mut part = Part::new(disk, base, size);
    let fs = fatfs::FileSystem::new(&mut part, fatfs::FsOptions::new()).map_err(bad)?;
    let dir = fs.root_dir().open_dir(DIR).map_err(bad)?;
    Ok(Seed::from_files(|n| {
        let mut f = dir.open_file(n).ok()?;
        let mut v = Vec::new();
        f.read_to_end(&mut v).ok()?;
        String::from_utf8(v).ok()
    }))
}

/// Put, then read back: what's there is what was meant.
pub fn put_checked<D: Read + Write + Seek>(disk: D, base: u64, size: u64, seed: &Seed) -> Result<D, String> {
    let mut disk = put(disk, base, size, seed)?;
    let back = read(&mut disk, base, size)?;
    if back != *seed {
        return Err("Your files didn't read back as they were written to namiOS's boot partition. Try a different USB drive.".into());
    }
    Ok(disk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const MIB: u64 = 1 << 20;

    /// A "disk" with a FAT32 partition of 40 MiB at 1 MiB, with a file of the image's on it.
    fn disk() -> Vec<u8> {
        let mut part = Cursor::new(vec![0u8; 40 * MIB as usize]);
        fatfs::format_volume(&mut part, fatfs::FormatVolumeOptions::new().fat_type(fatfs::FatType::Fat32).volume_label(*b"ESP        ")).unwrap();
        {
            let fs = fatfs::FileSystem::new(&mut part, fatfs::FsOptions::new()).unwrap();
            fs.root_dir().create_dir("EFI").unwrap().create_dir("BOOT").unwrap().create_file("BOOTX64.EFI").unwrap().write_all(b"shim").unwrap();
        }
        let mut d = vec![0x5au8; MIB as usize];
        d.extend(part.into_inner());
        d.extend(vec![0xa5u8; MIB as usize]);
        d
    }

    #[test]
    fn the_seed_goes_on_the_boot_partition() {
        let before = disk();
        let system = "# mine\nhostname: laptop\npackages:\n  - namios\n  - vlc\nunfree: true\n".to_string();
        let seed = Seed { system: Some(system.clone()), user: Some("packages: [ripgrep]\n".into()), source: None };
        let after = put_checked(Cursor::new(before.clone()), MIB, 40 * MIB, &seed).unwrap().into_inner();
        assert_eq!(after.len(), before.len());
        // Nothing outside the partition changed.
        assert_eq!(after[..MIB as usize], before[..MIB as usize]);
        assert_eq!(after[41 * MIB as usize..], before[41 * MIB as usize..]);
        // What's there: the image's own files, and the seed's.
        let mut part = Cursor::new(after[MIB as usize..41 * MIB as usize].to_vec());
        let fs = fatfs::FileSystem::new(&mut part, fatfs::FsOptions::new()).unwrap();
        let mut shim = String::new();
        fs.root_dir().open_file("EFI/BOOT/BOOTX64.EFI").unwrap().read_to_string(&mut shim).unwrap();
        assert_eq!(shim, "shim");
        let mut got = String::new();
        fs.root_dir().open_file("namios/system.nami").unwrap().read_to_string(&mut got).unwrap();
        assert_eq!(got, system);
        let names: Vec<String> = fs.root_dir().open_dir("namios").unwrap().iter().map(|e| e.unwrap().file_name()).filter(|n| !n.starts_with('.')).collect();
        assert_eq!(names, ["system.nami", "user.nami"]);
    }

    #[test]
    fn a_git_address_alone_and_written_again() {
        let d = put_checked(Cursor::new(disk()), MIB, 40 * MIB, &Seed { system: Some("x: 1\n".into()), ..Default::default() }).unwrap();
        // Again, with a git address only: system.nami goes.
        let seed = Seed { source: Some("github:you/config/laptop".into()), ..Default::default() };
        let mut d = put_checked(d, MIB, 40 * MIB, &seed).unwrap();
        assert_eq!(read(&mut d, MIB, 40 * MIB).unwrap(), seed);
        let mut part = Cursor::new(d.into_inner()[MIB as usize..41 * MIB as usize].to_vec());
        let fs = fatfs::FileSystem::new(&mut part, fatfs::FsOptions::new()).unwrap();
        let mut line = String::new();
        fs.root_dir().open_file("namios/source").unwrap().read_to_string(&mut line).unwrap();
        assert_eq!(line, "github:you/config/laptop\n");
        assert!(fs.root_dir().open_file("namios/system.nami").is_err());
    }

    #[test]
    fn handed_over_in_a_folder() {
        let dir = std::env::temp_dir().join(format!("namios-seed-test-{}", std::process::id()));
        let seed = Seed { system: Some("hostname: a\n".into()), user: None, source: Some(" github:a/b ".into()) };
        seed.save(&dir).unwrap();
        let back = Seed::load(&dir).unwrap();
        assert_eq!(back, Seed { source: Some("github:a/b".into()), ..seed });
        Seed::default().save(&dir).unwrap();
        assert!(Seed::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn written_in_whole_blocks() {
        /// A disk that takes only whole blocks from block boundaries, as a raw one does.
        struct Raw(Cursor<Vec<u8>>);
        impl Read for Raw {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                assert!(self.0.position() % 4096 == 0 && b.len() % 4096 == 0, "read {} at {}", b.len(), self.0.position());
                self.0.read(b)
            }
        }
        impl Write for Raw {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                assert!(self.0.position() % 4096 == 0 && b.len() % 4096 == 0, "write {} at {}", b.len(), self.0.position());
                self.0.write(b)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl Seek for Raw {
            fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
                self.0.seek(p)
            }
        }
        let seed = Seed { system: Some("hostname: raw\n".into()), ..Default::default() };
        put_checked(Raw(Cursor::new(disk())), MIB, 40 * MIB, &seed).unwrap();
    }

    /// A directory fatfs makes (EFI/, namios/) starts as every FAT directory must, for
    /// fsck.fat and chkdsk: "." then "..", short names alone, ".." naming the root as cluster 0
    /// (vendor/fatfs, "namiOS:").
    #[test]
    fn a_directory_as_fat_has_them() {
        let seed = Seed { system: Some("hostname: x\n".into()), ..Default::default() };
        let d = put(Cursor::new(disk()), MIB, 40 * MIB, &seed).unwrap().into_inner();
        let dot = d.windows(12).position(|w| w == b".          \x10").expect("a \".\" entry");
        assert_eq!(dot % 512, 0, "\".\" is the directory's first entry");
        assert_eq!(&d[dot + 32..dot + 44], b"..         \x10", "\"..\" its second");
        assert_eq!(&d[dot + 32 + 20..dot + 32 + 22], [0, 0]);
        assert_eq!(&d[dot + 32 + 26..dot + 32 + 28], [0, 0], "\"..\" names the root as cluster 0");
    }

    /// The image's EFI partition, found from its partition table, as the USB drive's writer
    /// finds it.
    #[test]
    fn found_from_the_gpt() {
        let mut d = disk();
        // A GPT naming the EFI partition (1 MiB, 40 MiB) and a root after it.
        d.resize(50 * MIB as usize, 0);
        d[..MIB as usize].fill(0);
        d[512..520].copy_from_slice(b"EFI PART");
        d[512 + 72..512 + 80].copy_from_slice(&2u64.to_le_bytes());
        d[512 + 80..512 + 84].copy_from_slice(&128u32.to_le_bytes());
        d[512 + 84..512 + 88].copy_from_slice(&128u32.to_le_bytes());
        let put_entry = |d: &mut Vec<u8>, i: usize, kind: [u8; 16], start: u64, end: u64| {
            let e = 1024 + i * 128;
            d[e..e + 16].copy_from_slice(&kind);
            d[e + 32..e + 40].copy_from_slice(&(start / 512).to_le_bytes());
            d[e + 40..e + 48].copy_from_slice(&(end / 512 - 1).to_le_bytes());
        };
        put_entry(&mut d, 0, [0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b], MIB, 41 * MIB);
        put_entry(&mut d, 1, [0xe3, 0xbc, 0x68, 0x4f, 0xcd, 0xe8, 0xb1, 0x4d, 0x96, 0xe7, 0xfb, 0xca, 0xf9, 0x84, 0xb7, 0x09], 42 * MIB, 50 * MIB);
        let (esp, _) = crate::gpt::image_parts(&d[..MIB as usize]).unwrap();
        assert_eq!((esp.start, esp.size), (MIB, 40 * MIB));
        let seed = Seed { system: Some("hostname: gpt\n".into()), ..Default::default() };
        let mut c = put_checked(Cursor::new(d), esp.start, esp.size, &seed).unwrap();
        assert_eq!(read(&mut c, esp.start, esp.size).unwrap(), seed);
    }
}
